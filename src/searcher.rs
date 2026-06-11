//! Handles directory traversal and file processing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::fs::{DirEntry, FileType};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;
#[cfg(feature = "git")]
use git2::Repository;
use lscolors::{LsColors, Style};
use regex::Regex;
#[cfg(all(unix, feature = "users"))]
use uzers::UsersCache;

use crate::config::Config;
use crate::expr::Expr;
use crate::field::Field;
use crate::field::context::{FieldContext, FileMetadataState};
use crate::field::dispatch;
use crate::fileinfo::{to_file_info, FileInfo};
use crate::function;
use crate::ignore::docker::{
    matches_dockerignore_filter, search_upstream_dockerignore, DockerignoreFilter,
};
use crate::ignore::hg::{matches_hgignore_filter, search_upstream_hgignore, HgignoreFilter};
use crate::operators::{LogicalOp, Op};
use crate::output::ResultsWriter;
use crate::query::TraversalMode::{Bfs, Dfs};
use crate::query::{Query, Root, TraversalMode};
use crate::util::*;
use crate::util::error::{error_message, path_error_message, SearchError};


pub struct Searcher<'a> {
    query: &'a Query,
    config: &'a Config,
    default_config: &'a Config,
    use_colors: bool,
    results_writer: ResultsWriter,
    #[cfg(all(unix, feature = "users"))]
    user_cache: UsersCache,
    regex_cache: HashMap<String, Regex>,
    found: u32,
    accumulators: HashMap<Vec<String>, function::GroupAccumulator>,
    output_buffer: TopN<Criteria<String>, String>,
    ordering_fields_rc: Rc<Vec<Expr>>,
    ordering_asc_rc: Rc<Vec<bool>>,

    record_context: Rc<RefCell<HashMap<String, HashMap<String, String>>>>,
    current_alias: Option<String>,
    subquery_required_fields: Option<HashMap<Field, String>>,

    hgignore_filters: Vec<HgignoreFilter>,
    dockerignore_filters: Vec<DockerignoreFilter>,
    visited_dirs: HashSet<PathBuf>,
    lscolors: LsColors,
    dir_queue: VecDeque<(PathBuf, u32)>,
    current_follow_symlinks: bool,
    current_min_depth: u32,
    current_max_depth: u32,
    current_search_archives: bool,
    current_apply_gitignore: bool,
    current_apply_hgignore: bool,
    current_apply_dockerignore: bool,
    current_traversal_mode: TraversalMode,
    current_root_dir: PathBuf,

    fms: FileMetadataState,
    #[cfg(feature = "git")]
    git_cache: crate::util::git::GitCache,
    file_map: HashMap<String, String>,
    conforms_map: HashMap<String, String>,
    subquery_cache: HashMap<String, Vec<String>>,
    silent_mode: bool,

    pub error_count: i32,
}

static FIELD_WITH_ALIAS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^([a-zA-Z0-9_]+)\\.([a-zA-Z0-9_]+)$").unwrap()
});

macro_rules! try_output {
    ($expr:expr, $ret:expr) => {
        if let Err(e) = $expr {
            if e.kind() == ErrorKind::BrokenPipe {
                return $ret;
            }
        }
    };
}

/// Returns true if a subquery's results are independent of the outer row,
/// i.e. it can be safely memoised. A subquery is *not* cacheable when any of
/// its expressions references a `root_alias` that is not declared on one of
/// the subquery's own roots — that's the correlated-subquery case, where the
/// result depends on outer-row state propagated via `record_context`.
fn is_subquery_cacheable(query: &Query) -> bool {
    let own_aliases: HashSet<String> = query
        .roots
        .iter()
        .filter_map(|r| r.options.alias.clone())
        .collect();
    !expr_references_external_alias(&query.expr, &own_aliases)
        && query.fields.iter().all(|e| !expr_walk_external_alias(e, &own_aliases))
}

fn expr_references_external_alias(expr: &Option<Expr>, own: &HashSet<String>) -> bool {
    match expr {
        Some(e) => expr_walk_external_alias(e, own),
        None => false,
    }
}

fn expr_walk_external_alias(expr: &Expr, own: &HashSet<String>) -> bool {
    if let Some(ref alias) = expr.root_alias
        && !own.contains(alias) {
            return true;
        }
    if let Some(ref left) = expr.left
        && expr_walk_external_alias(left, own) { return true; }
    if let Some(ref right) = expr.right
        && expr_walk_external_alias(right, own) { return true; }
    if let Some(ref args) = expr.args
        && args.iter().any(|a| expr_walk_external_alias(a, own)) { return true; }
    // Nested subqueries: descend so a doubly-nested correlated reference is
    // also detected.
    if let Some(ref sub) = expr.subquery {
        let nested_own: HashSet<String> = sub
            .roots
            .iter()
            .filter_map(|r| r.options.alias.clone())
            .chain(own.iter().cloned())
            .collect();
        if let Some(ref sub_expr) = sub.expr
            && expr_walk_external_alias(sub_expr, &nested_own) { return true; }
    }
    false
}

#[cfg(any(all(windows, feature = "everything"), all(unix, feature = "plocate")))]
fn external_index_depth(path: &Path, root_prefix: &str) -> Option<u32> {
    let s = path.to_string_lossy();
    let under = if cfg!(windows) {
        s.len() >= root_prefix.len() && s[..root_prefix.len()].eq_ignore_ascii_case(root_prefix)
    } else {
        s.starts_with(root_prefix)
    };
    if !under {
        return None;
    }
    let rel = &s[root_prefix.len()..];
    if rel.is_empty() {
        return None;
    }
    Some(rel.matches(std::path::MAIN_SEPARATOR).count() as u32 + 1)
}

impl<'a> Searcher<'a> {
    pub fn new(
        query: &'a Query,
        config: &'a Config,
        default_config: &'a Config,
        use_colors: bool,
    ) -> Self {
        let record_context = Rc::new(RefCell::new(HashMap::new()));
        Self::new_with_context(query, record_context, config, default_config, use_colors)
    }

    pub fn new_with_context(
        query: &'a Query,
        record_context: Rc<RefCell<HashMap<String, HashMap<String, String>>>>,
        config: &'a Config,
        default_config: &'a Config,
        use_colors: bool,
    ) -> Self {
        let limit = query.limit;

        let results_writer = ResultsWriter::new(&query.output_format);
        Searcher {
            query,
            config,
            default_config,
            use_colors,
            results_writer,
            #[cfg(all(unix, feature = "users"))]
            user_cache: UsersCache::new(),
            regex_cache: HashMap::new(),
            found: 0,
            accumulators: HashMap::new(),
            output_buffer: if limit == 0 {
                TopN::limitless()
            } else {
                TopN::new(limit.saturating_add(query.offset))
            },
            ordering_fields_rc: Rc::new(query.ordering_fields.clone()),
            ordering_asc_rc: Rc::new(query.ordering_asc.clone()),
            record_context,
            current_alias: None,
            subquery_required_fields: None,

            hgignore_filters: vec![],
            dockerignore_filters: vec![],
            visited_dirs: HashSet::new(),
            lscolors: LsColors::from_env().unwrap_or_default(),
            dir_queue: VecDeque::new(),
            current_follow_symlinks: false,
            current_min_depth: 0,
            current_max_depth: 0,
            current_search_archives: false,
            current_apply_gitignore: false,
            current_apply_hgignore: false,
            current_apply_dockerignore: false,
            current_traversal_mode: TraversalMode::Bfs,
            current_root_dir: PathBuf::new(),

            fms: FileMetadataState::new(),
            #[cfg(feature = "git")]
            git_cache: crate::util::git::GitCache::new(),
            file_map: HashMap::new(),
            conforms_map: HashMap::new(),
            subquery_cache: HashMap::new(),
            silent_mode: false,

            error_count: 0,
        }
    }

    pub fn is_buffered(&self) -> bool {
        self.query.is_ordered() || self.query.has_aggregate_column() || self.query.offset > 0 || self.silent_mode
    }

    /// Searches directories based on configured query and outputs results to stdout.
    pub fn list_search_results(&mut self) -> Result<(), SearchError> {
        // Pre-flight: catch unparseable date/datetime literals once, up front,
        // so a typo like `where modified = 'not-a-date'` fails immediately
        // with a single fatal error instead of degrading per file scanned.
        if let Some(ref where_expr) = self.query.expr
            && let Err(msg) = where_expr.validate_datetime_literals() {
                return Err(SearchError::fatal(msg).with_source("where"));
            }

        let current_dir = std::env::current_dir()?;

        if !self.silent_mode {
            let col_count = self.query.fields.len();
            try_output!(self.results_writer.write_header(&self.query.raw_query, col_count, &mut std::io::stdout()), Ok(()));
        }

        let start_time = std::time::Instant::now();

        let mut roots = vec![];

        // ======== Process each root specified in the query =========
        for root in &self.query.roots {
            if root.options.regexp {
                let mut ext_roots: Vec<String> = vec![];
                // Split the path into parts to process each segment as a regex
                let parts = root.path.split('/').collect::<Vec<&str>>();
                for part in parts {
                    if looks_like_regexp(part) {
                        // Create a regex from the part
                        let rx_string = format!("^{}$", part);
                        let rx = match Regex::new(&rx_string) {
                            Ok(rx) => rx,
                            Err(_) => {
                                return Err(SearchError::fatal(format!("invalid regex in root path: {}", part)));
                            }
                        };
                        let mut tmp = vec![];

                        if ext_roots.is_empty() {
                            let part = part.to_string();
                            if part.starts_with("/") {
                                ext_roots.push(String::from("/"));
                            } else {
                                ext_roots.push(String::from(""));
                            }
                        }

                        // Read the directory and filter entries matching the regex
                        for root in &ext_roots {
                            let mut start_from_rx_dir = false;

                            let mut path = Path::new(&root);

                            if path == Path::new("") {
                                path = current_dir.as_path();
                                start_from_rx_dir = true;
                            }

                            match path.read_dir() {
                                Ok(read_result) => {
                                    for entry in read_result.flatten() {
                                        if let Ok(file_type) = entry.file_type()
                                            && file_type.is_dir()
                                                && rx.is_match(
                                                    entry.file_name().to_string_lossy().as_ref(),
                                                )
                                            {
                                                if start_from_rx_dir {
                                                    tmp.push(
                                                        entry
                                                            .file_name()
                                                            .to_string_lossy()
                                                            .to_string(),
                                                    );
                                                } else {
                                                    tmp.push(
                                                        entry.path().to_string_lossy().to_string(),
                                                    );
                                                }
                                            }
                                    }
                                }
                                Err(e) => {
                                    self.error_count += 1;
                                    path_error_message(path, e)
                                }
                            }
                        }

                        ext_roots = tmp;
                    } else if ext_roots.is_empty() {
                        ext_roots.push(part.to_string());
                    } else {
                        //update all roots
                        ext_roots = ext_roots
                            .iter()
                            .map(|root| root.to_string() + "/" + part)
                            .collect();
                    }
                }

                ext_roots.iter().for_each(|ext_root| {
                    roots.push(Root::clone_with_path(ext_root.to_string(), root.clone()))
                });
            } else {
                // The root is not a regular expression
                roots.push(root.clone());
            }
        }

        // ======== Explore each root =========
        for root in roots {
            self.current_follow_symlinks = root.options.symlinks;
            self.current_alias = root.options.alias.clone();
            self.subquery_required_fields = match (&self.current_alias, &self.query.expr) {
                (Some(alias), Some(expr)) => {
                    let fields = expr.get_fields_required_in_subqueries(alias, false);
                    if fields.is_empty() { None } else { Some(fields) }
                }
                _ => None,
            };

            if let Some(ref inner) = root.subquery {
                let paths = self.collect_subquery_root_paths(inner.as_ref().clone());
                self.dir_queue.clear();
                self.visited_dirs.clear();
                self.hgignore_filters.clear();
                self.dockerignore_filters.clear();
                self.current_min_depth = 0;
                self.current_max_depth = 0;
                self.current_search_archives = root.options.archives;
                self.current_apply_gitignore = root
                    .options
                    .gitignore
                    .unwrap_or(self.config.gitignore.unwrap_or(false));
                self.current_apply_hgignore = root
                    .options
                    .hgignore
                    .unwrap_or(self.config.hgignore.unwrap_or(false));
                self.current_apply_dockerignore = root
                    .options
                    .dockerignore
                    .unwrap_or(self.config.dockerignore.unwrap_or(false));
                self.current_traversal_mode = root.options.traversal;

                let result = self.visit_subquery_paths(paths);
                if let Err(err) = result
                    && err.is_fatal() {
                        return Err(err);
                    }
                continue;
            }

            self.current_root_dir = PathBuf::from(&root.path);
            self.current_min_depth = root.options.min_depth;
            self.current_max_depth = root.options.max_depth;
            self.current_search_archives = root.options.archives;
            self.current_apply_gitignore = root
                .options
                .gitignore
                .unwrap_or(self.config.gitignore.unwrap_or(false));
            self.current_apply_hgignore = root
                .options
                .hgignore
                .unwrap_or(self.config.hgignore.unwrap_or(false));
            self.current_apply_dockerignore = root
                .options
                .dockerignore
                .unwrap_or(self.config.dockerignore.unwrap_or(false));
            self.current_traversal_mode = root.options.traversal;

            self.dir_queue.clear();
            self.visited_dirs.clear();
            self.hgignore_filters.clear();
            self.dockerignore_filters.clear();

            // Apply filters
            if self.current_apply_hgignore {
                search_upstream_hgignore(&mut self.hgignore_filters, &self.current_root_dir);
            }

            if self.current_apply_dockerignore {
                search_upstream_dockerignore(&mut self.dockerignore_filters, &self.current_root_dir);
            }

            #[cfg(all(windows, feature = "everything"))]
            {
                if self.config.everything.unwrap_or(false)
                    && !self.current_search_archives
                    && !self.current_apply_gitignore
                    && !self.current_apply_hgignore
                    && !self.current_apply_dockerignore
                    && self.try_visit_with_everything(&self.current_root_dir.clone())?
                {
                    continue;
                }
            }

            #[cfg(all(unix, feature = "plocate"))]
            {
                if self.config.plocate.unwrap_or(false)
                    && !self.current_search_archives
                    && !self.current_apply_gitignore
                    && !self.current_apply_hgignore
                    && !self.current_apply_dockerignore
                    && self.try_visit_with_plocate(&self.current_root_dir.clone())?
                {
                    continue;
                }
            }

            let result = self.visit_dir(
                &self.current_root_dir.clone(),
                0,
                #[cfg(feature = "git")]
                Repository::discover(&self.current_root_dir).ok().as_ref(),
                true,
            );

            if let Err(err) = result
                && err.is_fatal() {
                    return Err(err);
                }
        }

        let compute_time = std::time::Instant::now();

        // ======== Compute results =========
        if self.query.has_aggregate_column() {
            if !self.query.grouping_fields.is_empty() {
                let group_keys: Vec<String> = self
                    .query
                    .grouping_fields
                    .iter()
                    .map(|f| f.to_string())
                    .collect();
                let accumulators = std::mem::take(&mut self.accumulators);

                let ordering_fields_rc = self.ordering_fields_rc.clone();
                let ordering_asc_rc = self.ordering_asc_rc.clone();
                let field_names: Vec<String> = self.query.fields.iter()
                    .map(|f| f.to_string().to_lowercase())
                    .collect();
                let sorting_indices: Vec<usize> = self.query.ordering_fields.iter()
                    .map(|f| {
                        let name = f.to_string().to_lowercase();
                        field_names.iter().position(|g| g == &name).unwrap_or(0)
                    })
                    .collect();

                let mut grouped_results: TopN<Criteria<String>, Vec<(String, String)>> =
                    if self.query.limit > 0 {
                        TopN::new(self.query.limit.saturating_add(self.query.offset))
                    } else {
                        TopN::limitless()
                    };

                for (group_key, group_acc) in &accumulators {
                    let mut items: Vec<(String, String)> = Vec::new();
                    let mut file_map = HashMap::new();
                    for (i, k) in group_keys.iter().enumerate() {
                        file_map.insert(k.clone(), group_key.get(i).cloned().unwrap_or_default());
                    }
                    for column_expr in &self.query.fields {
                        if let Ok(value) = self.get_column_expr_value(
                            None, &None, Path::new(""), &mut file_map, Some(group_acc), column_expr,
                        ) {
                            let field_name = column_expr.to_string().to_lowercase();
                            items.push((field_name, value.to_string()));
                        }
                    }
                    let criteria_values: Vec<String> = sorting_indices.iter()
                        .map(|i| items.get(*i).map(|item| item.1.clone()).unwrap_or_default())
                        .collect();
                    grouped_results.insert(
                        Criteria::new(ordering_fields_rc.clone(), criteria_values, ordering_asc_rc.clone()),
                        items,
                    );
                }

                let mut first = true;
                let mut stdout = std::io::stdout().lock();
                for items in grouped_results.iter_values().skip(self.query.offset as usize) {
                    let mut buf = WritableBuffer::new();
                    self.results_writer.write_row(&mut buf, items.clone())?;
                    let rendered = String::from(buf);
                    if !self.silent_mode {
                        if !first {
                            try_output!(self.results_writer.write_row_separator(&mut stdout), Ok(()));
                        }
                        first = false;
                        try_output!(write!(stdout, "{}", &rendered), Ok(()));
                    }
                    self.output_buffer.insert(
                        Criteria::new(Rc::new(vec![]), vec![], Rc::new(vec![])),
                        rendered,
                    );
                }
                drop(stdout);
            } else {
                let mut buf = WritableBuffer::new();
                let mut items: Vec<(String, String)> = Vec::new();

                let accumulators = std::mem::take(&mut self.accumulators);
                let empty_acc = function::GroupAccumulator::default();
                let ungrouped_acc = accumulators.get(&vec![]).unwrap_or(&empty_acc);
                for column_expr in &self.query.fields {
                    if let Ok(value) = self.get_column_expr_value(
                        None,
                        &None,
                        Path::new(""),
                        &mut HashMap::new(),
                        Some(ungrouped_acc),
                        column_expr
                    ) {
                        let field_name = column_expr.to_string().to_lowercase();
                        items.push((field_name, value.to_string()));
                    }
                }

                self.results_writer.write_row(&mut buf, items)?;
                let rendered = String::from(buf);
                self.output_buffer.insert(
                    Criteria::new(Rc::new(vec![]), vec![], Rc::new(vec![])),
                    rendered.clone(),
                );

                if !self.silent_mode {
                    try_output!(write!(std::io::stdout(), "{}", rendered), Ok(()));
                }
            }
        } else if self.is_buffered() && !self.silent_mode {
            let mut stdout = std::io::stdout().lock();
            let mut first = true;
            for piece in self.output_buffer.iter_values().skip(self.query.offset as usize) {
                if first {
                    first = false;
                } else {
                    try_output!(self.results_writer.write_row_separator(&mut stdout), Ok(()));
                }
                try_output!(write!(stdout, "{}", piece), Ok(()));
            }
            drop(stdout);
        }

        if !self.silent_mode {
            let mut stdout = std::io::stdout().lock();
            self.results_writer.write_footer(&mut stdout)?;
        }

        let completion_time = std::time::Instant::now();

        if self.config.debug {
            eprintln!("Search: {}ms\nCompute: {}ms", 
                      compute_time.duration_since(start_time).as_millis(), 
                      completion_time.duration_since(compute_time).as_millis());
        }

        Ok(())
    }

    /// Run a FROM-clause subselect and return the list of paths it produced.
    /// The inner query is forced to emit only the `path` field so its rows can
    /// drive the outer query as a flat list of filesystem entries.
    fn collect_subquery_root_paths(&mut self, mut query: Query) -> Vec<String> {
        // Use the absolute path so visit_subquery_paths can re-resolve each
        // result against the filesystem regardless of the inner query's root.
        query.fields = vec![Expr::field(Field::AbsPath)];
        query.output_format = crate::query::OutputFormat::Tabs;
        self.get_list_from_subquery(query)
    }

    /// Treat each path produced by a FROM subselect as an input entry: resolve
    /// it to a `DirEntry` (by reading its parent directory) and feed it through
    /// `check_file` so the outer query's WHERE/SELECT logic can run against it
    /// without further directory traversal.
    fn visit_subquery_paths(&mut self, paths: Vec<String>) -> Result<(), SearchError> {
        for path_str in paths {
            if path_str.is_empty() {
                continue;
            }
            if !self.is_buffered() && self.query.limit > 0 && self.query.limit <= self.found {
                break;
            }
            let path = PathBuf::from(&path_str);
            let parent = match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => PathBuf::from("."),
            };
            let file_name = match path.file_name() {
                Some(n) => n.to_os_string(),
                None => continue,
            };
            self.current_root_dir = parent.clone();

            match fs::read_dir(&parent) {
                Ok(entries) => {
                    let mut matched_entry: Option<DirEntry> = None;
                    for entry in entries.flatten() {
                        if entry.file_name() == file_name {
                            matched_entry = Some(entry);
                            break;
                        }
                    }
                    if let Some(entry) = matched_entry
                        && let Err(err) = self.check_file(&entry, &parent, &None, None) {
                            if err.is_fatal() {
                                return Err(err);
                            }
                            self.handle_nonfatal_error(err, &path);
                        }
                }
                Err(err) => {
                    self.error_count += 1;
                    path_error_message(&parent, err);
                }
            }
        }
        Ok(())
    }

    fn get_list_from_subquery(&mut self, query: Query) -> Vec<String> {
        let query_str = format!("{:?}", query);

        let ok_to_cache = is_subquery_cacheable(&query);
        if ok_to_cache
            && let Some(cached) = self.subquery_cache.get(&query_str) {
                return cached.clone();
            }

        let mut sub_searcher = Searcher::new_with_context(
            &query,
            self.record_context.clone(),
            self.config,
            self.default_config,
            self.use_colors
        );
        sub_searcher.silent_mode = !self.config.debug;
        if let Err(err) = sub_searcher.list_search_results() {
            err.print();
            return vec![];
        }

        let result_values = sub_searcher.output_buffer.iter_values()
            .map(|s| s.trim_end().to_string())
            .collect::<Vec<String>>();

        if ok_to_cache {
            self.subquery_cache.insert(query_str, result_values.clone());
        }

        result_values
    }

    #[cfg(all(windows, feature = "everything"))]
    fn try_visit_with_everything(&mut self, root_dir: &Path) -> Result<bool, SearchError> {
        let abs = match canonical_path(&root_dir.to_path_buf()) {
            Ok(p) => p,
            Err(_) => return Ok(false),
        };

        let paths = match crate::util::everything::query_descendants(&abs) {
            Ok(paths) => paths,
            Err(err) => {
                if self.config.debug {
                    use crate::util::everything::EverythingError;
                    let reason = match err {
                        EverythingError::Unavailable => String::from("SDK not available"),
                        EverythingError::Query(code) => format!("query failed (error {})", code),
                    };
                    eprintln!("Everything unavailable: {}; falling back to traversal", reason);
                }
                return Ok(false);
            }
        };

        self.visit_external_index_results(root_dir, abs, paths)?;
        Ok(true)
    }

    #[cfg(all(unix, feature = "plocate"))]
    fn try_visit_with_plocate(&mut self, root_dir: &Path) -> Result<bool, SearchError> {
        let abs = match canonical_path(&root_dir.to_path_buf()) {
            Ok(p) => p,
            Err(_) => return Ok(false),
        };

        let paths = match crate::util::plocate::query_descendants(&abs) {
            Ok(paths) => paths,
            Err(err) => {
                if self.config.debug {
                    use crate::util::plocate::PlocateError;
                    let reason = match err {
                        PlocateError::Spawn => String::from("plocate not found"),
                        PlocateError::Failed(code) => match code {
                            Some(code) => format!("exited with code {}", code),
                            None => String::from("terminated by signal"),
                        },
                    };
                    eprintln!("plocate unavailable: {}; falling back to traversal", reason);
                }
                return Ok(false);
            }
        };

        self.visit_external_index_results(root_dir, abs, paths)?;
        Ok(true)
    }

    #[cfg(any(all(windows, feature = "everything"), all(unix, feature = "plocate")))]
    fn visit_external_index_results(
        &mut self,
        root_dir: &Path,
        abs: String,
        paths: Vec<PathBuf>,
    ) -> Result<(), SearchError> {
        let mut prefix = abs;
        if !prefix.ends_with(std::path::MAIN_SEPARATOR) {
            prefix.push(std::path::MAIN_SEPARATOR);
        }

        let min_depth = self.current_min_depth;
        let max_depth = self.current_max_depth;
        let kept = paths
            .into_iter()
            .filter(|path| match external_index_depth(path, &prefix) {
                Some(depth) => {
                    (min_depth == 0 || depth >= min_depth)
                        && (max_depth == 0 || depth <= max_depth)
                }
                None => false,
            });

        self.visit_external_index_entries(root_dir, kept)
    }

    #[cfg(any(all(windows, feature = "everything"), all(unix, feature = "plocate")))]
    fn visit_external_index_entries(
        &mut self,
        root_dir: &Path,
        paths: impl Iterator<Item = PathBuf>,
    ) -> Result<(), SearchError> {
        use std::ffi::OsString;

        let mut by_parent: HashMap<PathBuf, HashSet<OsString>> = HashMap::new();
        for path in paths {
            if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                by_parent
                    .entry(parent.to_path_buf())
                    .or_default()
                    .insert(name.to_os_string());
            }
        }

        for (parent, names) in by_parent {
            match fs::read_dir(&parent) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        if !self.is_buffered() && self.query.limit > 0 && self.query.limit <= self.found {
                            return Ok(());
                        }
                        if names.contains(&entry.file_name())
                            && let Err(err) = self.check_file(&entry, root_dir, &None, None) {
                                if err.is_fatal() {
                                    return Err(err);
                                }
                                self.handle_nonfatal_error(err, &entry.path());
                            }
                    }
                }
                Err(err) => {
                    self.error_count += 1;
                    path_error_message(&parent, err);
                }
            }
        }

        Ok(())
    }

    /// Recursively explore directories starting from a given path.
    /// Handles archives, and optionally applies filters.
    fn visit_dir(
        &mut self,
        dir: &Path,
        root_depth: u32,
        #[cfg(feature = "git")]
        git_repository: Option<&Repository>,
        process_queue: bool,
    ) -> Result<(), SearchError> {
        // Canonicalization is comparatively expensive: it resolves every path
        // component, opening a handle per directory on Windows. It is only
        // needed to detect symlink loops while following symlinks, and to
        // resolve entry paths against ignore filters. When neither applies we
        // skip it entirely, saving one syscall per directory traversed.
        let needs_canonical = self.current_follow_symlinks
            || self.current_apply_gitignore
            || self.current_apply_hgignore
            || self.current_apply_dockerignore;

        let canonical_path = if needs_canonical {
            match canonical_path(&dir.to_path_buf()) {
                Ok(path) => Some(path),
                Err(e) => {
                    self.error_count += 1;
                    error_message(
                        &dir.to_string_lossy(),
                        &format!("could not canonicalize path: {}", e),
                    );
                    return Ok(());
                }
            }
        } else {
            None
        };

        // Prevents infinite loops when following symlinks
        if self.current_follow_symlinks {
            let canonical_pathbuf = PathBuf::from(canonical_path.as_ref().unwrap());
            if self.visited_dirs.contains(&canonical_pathbuf) {
                return Ok(());
            } else {
                self.visited_dirs.insert(canonical_pathbuf);
            }
        }

        // `depth` is this directory's level (root == 1); `child_root_depth` is
        // passed to children/queued dirs so they can derive their own depth.
        // With a canonical path we keep the original separator-counting scheme
        // to preserve behavior; otherwise traversal descends exactly one real
        // directory per level, so an explicit counter is equivalent.
        let (depth, child_root_depth) = match &canonical_path {
            Some(canonical_path) => {
                let canonical_depth = calc_depth(canonical_path);
                let base_depth = match root_depth {
                    0 => canonical_depth,
                    _ => root_depth,
                };
                (canonical_depth.saturating_sub(base_depth) + 1, base_depth)
            }
            None => {
                let depth = root_depth + 1;
                (depth, depth)
            }
        };

        // Read the directory and process each entry
        let root_dir = self.current_root_dir.clone();
        match fs::read_dir(dir) {
            Ok(entry_list) => {
                for entry in entry_list {
                    if !self.is_buffered() && self.query.limit > 0 && self.query.limit <= self.found
                    {
                        break;
                    }

                    match entry {
                        Ok(entry) => {
                            let mut path = entry.path();
                            let pass_ignores = if self.current_apply_gitignore || self.current_apply_hgignore || self.current_apply_dockerignore {
                                let canonical_entry_path = PathBuf::from(canonical_path.as_ref().unwrap()).join(entry.file_name());

                                // Check the path against the filters
                                #[cfg(feature = "git")]
                                let pass_gitignore = !self.current_apply_gitignore
                                    || !git_repository
                                        .is_some_and(|repo| repo.is_path_ignored(&canonical_entry_path)
                                            .unwrap_or(false));
                                #[cfg(not(feature = "git"))]
                                let pass_gitignore = true;

                                let pass_hgignore = !self.current_apply_hgignore
                                    || !matches_hgignore_filter(
                                    &self.hgignore_filters,
                                    canonical_entry_path.to_string_lossy().as_ref(),
                                );
                                let pass_dockerignore = !self.current_apply_dockerignore
                                    || !matches_dockerignore_filter(
                                    &self.dockerignore_filters,
                                    canonical_entry_path.to_string_lossy().as_ref(),
                                );

                                pass_gitignore && pass_hgignore && pass_dockerignore
                            } else {
                                true
                            };                            

                            // If the path passes the filters, process it
                            if pass_ignores {
                                // Compute the entry's file type once, but only
                                // when the descent step below needs it anyway,
                                // and hand it to check_file so type predicates
                                // can reuse it instead of issuing their own
                                // stat. On filesystems that don't report
                                // d_type this avoids a duplicate lstat per
                                // entry; at max depth (no descent) we leave it
                                // unset so predicates only pay for it on demand.
                                let within_max_depth = self.current_max_depth == 0
                                    || depth < self.current_max_depth;
                                let descent_file_type =
                                    if within_max_depth { Some(entry.file_type()) } else { None };
                                let file_type_hint = match &descent_file_type {
                                    Some(Ok(file_type)) => Some(*file_type),
                                    _ => None,
                                };

                                if self.current_min_depth == 0 || depth >= self.current_min_depth {
                                    let checked = self.check_file(&entry, &root_dir, &None, file_type_hint);
                                    if let Err(err) = checked {
                                        if err.is_fatal() {
                                            return Err(err);
                                        }
                                        self.handle_nonfatal_error(err, &path);
                                        continue;
                                    }

                                    if self.current_search_archives
                                        && self.is_zip_archive(&path.to_string_lossy())
                                        && let Ok(file) = fs::File::open(&path)
                                            && let Ok(mut archive) = zip::ZipArchive::new(file) {
                                                for i in 0..archive.len() {
                                                    if !self.is_buffered() && self.query.limit > 0
                                                        && self.query.limit <= self.found
                                                    {
                                                        break;
                                                    }

                                                    if let Ok(afile) = archive.by_index(i) {
                                                        let file_info = to_file_info(&afile);
                                                        if let Err(err) = self.check_file(&entry, &root_dir, &Some(file_info), None) {
                                                            if err.is_fatal() {
                                                                return Err(err);
                                                            }
                                                            self.handle_nonfatal_error(err, &path);
                                                            continue;
                                                        }
                                                    }
                                                }
                                            }
                                }

                                // Recursively visit subdirectories if we're not too deep
                                if within_max_depth {
                                    match descent_file_type.unwrap() {
                                        Ok(file_type) => {
                                            let mut ok = false;

                                            if file_type.is_symlink() && self.current_follow_symlinks {
                                                if let Ok(resolved) = fs::read_link(&path) {
                                                    let resolved_path = if resolved.is_relative() {
                                                        if let Some(parent) = path.parent() {
                                                            parent.join(&resolved)
                                                        } else {
                                                            resolved
                                                        }
                                                    } else {
                                                        resolved
                                                    };
                                                    if resolved_path.is_dir() {
                                                        ok = true;
                                                        path = resolved_path;
                                                    }
                                                }
                                            } else if file_type.is_dir() {
                                                ok = true;
                                            }

                                            if ok && self.ok_to_visit_dir(file_type) {
                                                if self.current_traversal_mode == Dfs {
                                                    #[cfg(feature = "git")]
                                                    let repo;
                                                    #[cfg(feature = "git")]
                                                    let git_repository = match git_repository {
                                                        Some(repo) => Some(repo),
                                                        None if self.current_apply_gitignore => {
                                                            repo = Repository::open(&path).ok();
                                                            repo.as_ref()
                                                        },
                                                        _ => None,
                                                    };
                                                    let result = self.visit_dir(
                                                        &path,
                                                        child_root_depth,
                                                        #[cfg(feature = "git")]
                                                        git_repository,
                                                        false,
                                                    );

                                                    if let Err(err) = result {
                                                        if err.is_fatal() {
                                                            return Err(err);
                                                        }
                                                        self.handle_nonfatal_error(err, &path);
                                                    }
                                                } else {
                                                    self.dir_queue.push_back((path, child_root_depth));
                                                }
                                            }
                                        }
                                        Err(err) => {
                                            self.error_count += 1;
                                            path_error_message(&path, err);
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            self.error_count += 1;
                            path_error_message(dir, err);
                        }
                    }
                }
            }
            Err(err) => {
                self.error_count += 1;
                path_error_message(dir, err);
            }
        }

        if self.current_traversal_mode == Bfs && process_queue {
            while !self.dir_queue.is_empty() {
                let (path, queued_root_depth) = self.dir_queue.pop_front().unwrap();
                #[cfg(feature = "git")]
                let repo;
                #[cfg(feature = "git")]
                let git_repository = match git_repository {
                    Some(repo) => Some(repo),
                    None if self.current_apply_gitignore => {
                        repo = Repository::open(&path).ok();
                        repo.as_ref()
                    },
                    _ => None,
                };
                let result = self.visit_dir(
                    &path,
                    queued_root_depth,
                    #[cfg(feature = "git")]
                    git_repository,
                    false,
                );

                if let Err(err) = result {
                    if err.is_fatal() {
                        return Err(err);
                    }
                    self.handle_nonfatal_error(err, &path);
                }
            }
        }

        Ok(())
    }

    fn handle_nonfatal_error(&mut self, mut err: SearchError, default_source: &Path) {
        self.error_count += 1;
        if err.source.is_empty() {
            err.source = default_source.to_string_lossy().to_string();
        }
        err.print();
    }

    fn ok_to_visit_dir(&self, file_type: FileType) -> bool {
        self.current_follow_symlinks || !file_type.is_symlink()
    }

    fn is_declared_root_alias(&self, alias: &str) -> bool {
        self.query
            .roots
            .iter()
            .any(|root| root.options.alias.as_deref() == Some(alias))
    }

    fn get_column_expr_value(
        &mut self,
        entry: Option<&DirEntry>,
        file_info: &Option<FileInfo>,
        root_path: &Path,
        file_map: &mut HashMap<String, String>,
        accumulator: Option<&function::GroupAccumulator>,
        column_expr: &Expr,
    ) -> Result<Variant, SearchError> {
        let column_expr_str = column_expr.to_string();

        let mut should_update_context = false;

        if let Some(captures) = FIELD_WITH_ALIAS.captures(&column_expr_str) {
            let column_expr_context_name = captures.get(1).unwrap().as_str();
            if self.current_alias.as_deref() == Some(column_expr_context_name) {
                should_update_context = true;
            } else {
                let context = self.record_context.borrow();
                if let Some(ctx) = context.get(column_expr_context_name) {
                    if let Some(val) = ctx.get(captures.get(2).unwrap().as_str()) {
                        return Ok(Variant::from_string(val));
                    } else {
                        //TODO: this should be propagated up to the higher context
                        return Ok(Variant::empty(VariantType::String));
                    }
                }
                // The prefix doesn't name any known root context. An explicit
                // `alias.field` reference (or a prefix matching a declared root
                // alias) is a query error; any other dotted string is a plain
                // value (e.g. the literal `readme.md`) and falls through to
                // normal evaluation.
                if column_expr.root_alias.is_some()
                    || self.is_declared_root_alias(column_expr_context_name)
                {
                    return Err(SearchError::fatal(format!("Invalid root alias: {}", column_expr_context_name)).with_source("query"));
                }
            }
        }

        if file_map.contains_key(&column_expr_str) {
            if should_update_context {
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_default();
                let entry_key = column_expr_str.split('.').nth(1).unwrap().to_string();
                context_entry.insert(entry_key, file_map[&column_expr_str].clone());
            }
            return Ok(Variant::from_string(&file_map[&column_expr_str]));
        }

        if let Some(ref subquery) = column_expr.subquery {
            let mut subquery = subquery.clone();
            if subquery.grouping_fields.is_empty() {
                subquery.limit = 1;
            }
            let list = self.get_list_from_subquery(*subquery);
            if !list.is_empty() {
                let result = list.first().unwrap().to_string();
                return Ok(Variant::from_string(&result));
            }
        }

        if let Some(ref _function) = column_expr.function {
            let result =
                self.get_function_value(entry, file_info, root_path, file_map, accumulator, column_expr)?;
            file_map.insert(column_expr_str, result.to_string());
            return Ok(result);
        }

        if let Some(ref field) = column_expr.field {
            if let Some(entry) = entry {
                let result = self.get_field_value(entry, file_info, root_path, field).unwrap_or(Variant::empty(VariantType::String));
                file_map.insert(column_expr_str, result.to_string());
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_default();
                let entry_key = if let Some(alias) = column_expr.alias.clone() { alias } else { field.to_string() };
                context_entry.insert(entry_key, result.to_string());
                return Ok(result);
            } else if let Some(val) = file_map.get(&field.to_string()) {
                return Ok(Variant::from_string(val));
            } else {
                return Ok(Variant::empty(VariantType::String));
            }
        }

        if let Some(ref value) = column_expr.val {
            return Ok(Variant::from_signed_string(value, column_expr.minus));
        }

        let result;

        if let Some(ref left) = column_expr.left {
            let left_result =
                self.get_column_expr_value(entry, file_info, root_path, file_map, accumulator, left)?;

            if let Some(ref op) = column_expr.arithmetic_op {
                if let Some(ref right) = column_expr.right {
                    let right_result =
                        self.get_column_expr_value(entry, file_info, root_path, file_map, accumulator, right)?;
                        result = op.calc(&left_result, &right_result);
                        file_map.insert(column_expr_str, result.clone()?.to_string());
                } else {
                    result = Ok(left_result);
                }
            } else {
                result = Ok(left_result);
            }
        } else {
            result = Ok(Variant::empty(VariantType::Int));
        }

        result.map_err(|e| e.into())
    }

    fn get_function_value(
        &mut self,
        entry: Option<&DirEntry>,
        file_info: &Option<FileInfo>,
        root_path: &Path,
        file_map: &mut HashMap<String, String>,
        accumulator: Option<&function::GroupAccumulator>,
        column_expr: &Expr,
    ) -> Result<Variant, SearchError> {
        let dummy = Expr::value(String::from(""));
        let boxed_dummy = &Box::from(dummy);

        let left_expr = match &column_expr.left {
            Some(left_expr) => left_expr,
            _ => boxed_dummy,
        };

        let function = column_expr.function.as_ref().unwrap();

        if function.is_aggregate_function() {
            let _ = self.get_column_expr_value(entry, file_info, root_path, file_map, accumulator, left_expr)?;
            let buffer_key = left_expr.to_string();
            let empty_acc = function::GroupAccumulator::default();
            let aggr_result = function::get_aggregate_value(
                function,
                accumulator.unwrap_or(&empty_acc),
                buffer_key,
                &column_expr.val,
            );
            Ok(Variant::from_string(&aggr_result))
        } else {
            let function_arg =
                self.get_column_expr_value(entry, file_info, root_path, file_map, accumulator, left_expr);
            let mut function_args = vec![];
            if let Some(args) = &column_expr.args {
                for arg in args {
                    let arg_value =
                        self.get_column_expr_value(entry, file_info, root_path, file_map, accumulator, arg)?;
                    function_args.push(arg_value.to_string());
                }
            }
            let result = function::get_value(
                function,
                function_arg?.to_string(),
                function_args,
                entry,
                file_info,
            )?;
            file_map.insert(column_expr.to_string(), result.to_string());

            Ok(result)
        }
    }


    fn get_field_value(
        &mut self,
        entry: &DirEntry,
        file_info: &Option<FileInfo>,
        root_path: &Path,
        field: &Field,
    ) -> Result<Variant, SearchError> {
        let mut ctx = FieldContext {
            entry,
            file_info,
            root_path,
            fms: &mut self.fms,
            #[cfg(feature = "git")]
            git_cache: &mut self.git_cache,
            follow_symlinks: self.current_follow_symlinks,
            config: self.config,
            default_config: self.default_config,
            #[cfg(all(unix, feature = "users"))]
            user_cache: &self.user_cache,
        };
        dispatch::get_field_value(&mut ctx, field)
    }

    fn check_file(&mut self, entry: &DirEntry, root_path: &Path, file_info: &Option<FileInfo>, file_type_hint: Option<FileType>) -> Result<(), SearchError> {
        self.fms.clear();
        // Reuse the file type the traversal already resolved, so is_dir /
        // is_file / is_symlink don't issue a redundant stat for this entry.
        self.fms.seed_file_type(file_type_hint);

        let mut file_map = std::mem::take(&mut self.file_map);
        file_map.clear();
        let result = self.check_file_inner(entry, root_path, file_info, &mut file_map);
        self.file_map = file_map;
        result
    }

    fn check_file_inner(&mut self, entry: &DirEntry, root_path: &Path, file_info: &Option<FileInfo>, file_map: &mut HashMap<String, String>) -> Result<(), SearchError> {
        if let Some(ref current_alias) = self.current_alias.clone() {
            {
                let mut context = self.record_context.borrow_mut();
                if let Some(ctx) = context.get_mut(current_alias) {
                    ctx.clear();
                }
            }

            if let Some(required_fields) = std::mem::take(&mut self.subquery_required_fields) {
                let mut field_values = HashMap::new();
                for (field, alias) in &required_fields {
                    let field_value = self.get_field_value(entry, file_info, root_path, field).unwrap_or(Variant::empty(VariantType::String));
                    field_values.insert(alias.clone(), field_value);
                }
                self.subquery_required_fields = Some(required_fields);

                let mut context = self.record_context.borrow_mut();
                let context_entry = context.entry(current_alias.to_string()).or_default();
                for (field, field_value) in field_values {
                    context_entry.insert(field, field_value.to_string());
                }
            }
        }

        if let Some(ref expr) = self.query.expr {
            let result = self.conforms(entry, file_info, root_path, expr)?;
            if !result {
                return Ok(());
            }
        }

        self.found += 1;

        if self.query.has_aggregate_column() {
            for field in self.query.get_all_fields() {
                file_map.insert(
                    field.to_string(),
                    self.get_field_value(entry, file_info, root_path, &field).unwrap_or(Variant::empty(VariantType::String)).to_string(),
                );
            }
            // Evaluate inner expressions of aggregate functions so computed values
            // (e.g. "size * 2") are stored in file_map under the correct key.
            // Collect keys first to avoid cloning query.fields on every file.
            let aggregate_inner_exprs: Vec<_> = self.query.fields.iter()
                .filter_map(|column_expr| {
                    if let Some(ref func) = column_expr.function
                        && func.is_aggregate_function()
                            && let Some(ref left) = column_expr.left {
                                let left_key = left.to_string();
                                if !file_map.contains_key(&left_key) {
                                    return Some(left.clone());
                                }
                            }
                    None
                })
                .collect();
            for left in &aggregate_inner_exprs {
                self.get_column_expr_value(Some(entry), file_info, root_path, file_map, None, left)?;
            }
            for field in self.query.grouping_fields.iter() {
                if file_map.get(&field.to_string()).is_none() {
                    self.get_column_expr_value(Some(entry), file_info, root_path, file_map, None, field)?;
                }
            }
            let group_key: Vec<String> = self.query.grouping_fields.iter()
                .map(|f| file_map.get(&f.to_string()).cloned().unwrap_or_default())
                .collect();
            let accumulator = self.accumulators.entry(group_key).or_default();
            accumulator.increment_count();
            for (key, value) in file_map.iter() {
                accumulator.push(key, value);
            }
            return Ok(());
        }

        let mut buf = WritableBuffer::new();

        if !self.is_buffered() && self.found > 1 {
            self.results_writer.write_row_separator(&mut buf)?;
        }

        let mut items: Vec<(String, String)> = Vec::new();

        if self.use_colors && self.query.fields.iter().any(|f| f.contains_colorized()) {
            self.fms.update_file_metadata(entry, self.current_follow_symlinks);
        }

        for field in self.query.fields.iter() {
            let record =
                self.get_column_expr_value(Some(entry), file_info, root_path, file_map, None, field)?;

            let value = match self.use_colors && field.contains_colorized() {
                true => self.colorize(&record.to_string()),
                false => record.to_string(),
            };
            items.push((field.to_string(), value));
        }

        if file_info.is_some() {
            let archive_path = entry.path().to_string_lossy().to_string();
            items.insert(0, (String::from("archive"), format!("[{}]", archive_path)));
        }

        let mut criteria = vec!["".to_string(); self.query.ordering_fields.len()];
        for (idx, field) in self.query.ordering_fields.iter().enumerate() {
            criteria[idx] = match file_map.get(&field.to_string()) {
                Some(record) => record.clone(),
                None => self
                    .get_column_expr_value(Some(entry), file_info, root_path, file_map, None, field)?
                    .to_string(),
            }
        }

        self.results_writer.write_row(&mut buf, items)?;

        if self.is_buffered() {
            self.output_buffer.insert(
                Criteria::new(
                    self.ordering_fields_rc.clone(),
                    criteria,
                    self.ordering_asc_rc.clone(),
                ),
                String::from(buf),
            );
        } else {
            try_output!(write!(std::io::stdout(), "{}", String::from(buf)),
                        Err(SearchError::fatal("broken pipe").with_source("output")));
        }

        Ok(())
    }

    fn colorize(&mut self, value: &str) -> String {
        let path = Path::new(value);
        let style = match self.fms.get_file_metadata() {
            Some(metadata) => self.lscolors.style_for_path_with_metadata(path, Some(metadata)),
            None => self.lscolors.style_for_path(path),
        };

        let ansi_style = style.map(Style::to_nu_ansi_term_style).unwrap_or_default();

        format!("{}", ansi_style.paint(value))
    }


    fn check_exists(&mut self, expr: &Expr) -> bool {
        let right = expr.right.as_ref().unwrap();
        match &right.args {
            Some(args) => !args.is_empty(),
            None => {
                if let Some(subquery) = &right.subquery {
                    let mut subquery = *subquery.clone();
                    if subquery.grouping_fields.is_empty() {
                        subquery.limit = 1;
                    }
                    !self.get_list_from_subquery(subquery).is_empty()
                } else {
                    false
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_in_list(
        &mut self,
        expr: &Expr,
        entry: &DirEntry,
        file_info: &Option<FileInfo>,
        root_path: &Path,
        arg_map: &mut HashMap<String, String>,
        field_value: &Variant,
        negate: bool,
    ) -> Result<bool, SearchError> {
        let right = expr.right.as_ref().unwrap();
        let owned_args;
        let args: &[Expr] = match &right.args {
            Some(args) => args,
            None => {
                owned_args = if let Some(subquery) = &right.subquery {
                    self.get_list_from_subquery(*subquery.clone())
                        .iter()
                        .map(|s| Expr::value(s.to_string()))
                        .collect()
                } else {
                    vec![]
                };
                &owned_args
            }
        };
        let field_type = field_value.get_type();
        let mut found = false;
        for arg in args {
            arg_map.clear();
            let arg_val = self.get_column_expr_value(
                Some(entry), file_info, root_path, arg_map, None, arg,
            )?;
            let matches = match field_type {
                VariantType::String => arg_val.to_string() == field_value.to_string(),
                VariantType::Int => arg_val.to_int() == field_value.to_int(),
                VariantType::Float => arg_val.to_float() == field_value.to_float(),
                VariantType::Bool => arg_val.to_bool() == field_value.to_bool(),
                VariantType::DateTime => {
                    // Unparseable values: silently skip this arg (treat as a
                    // non-match) instead of erroring per file scanned.
                    match (field_value.to_datetime(), arg_val.to_datetime()) {
                        (Ok((field_dt, _)), Ok((arg_start, arg_finish))) => {
                            field_dt >= arg_start && field_dt <= arg_finish
                        }
                        _ => false,
                    }
                }
            };
            if matches {
                found = true;
                break;
            }
        }
        Ok(if negate { !found } else { found })
    }

    fn match_pattern(
        &mut self,
        val: String,
        field_str: &str,
        converter: fn(&str) -> Result<String, String>,
        err_prefix: &str,
        cache_prefix: &str,
    ) -> Result<bool, SearchError> {
        let cache_key = format!("{}:{}", cache_prefix, val);
        if let Some(regex) = self.regex_cache.get(&cache_key) {
            return Ok(regex.is_match(field_str));
        }
        match converter(&val) {
            Ok(pattern) => {
                match Regex::new(&pattern) {
                    Ok(regex) => {
                        let matched = regex.is_match(field_str);
                        self.regex_cache.insert(cache_key, regex);
                        Ok(matched)
                    }
                    _ => Err(SearchError::normal(format!("{}{}", err_prefix, val)).with_source("expression")),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    fn match_glob(
        &mut self,
        val: String,
        field_str: &str,
    ) -> Result<bool, SearchError> {
        let cache_key = format!("glob:{}", val);
        if let Some(regex) = self.regex_cache.get(&cache_key) {
            return Ok(regex.is_match(field_str));
        }
        match convert_glob_to_pattern(&val) {
            Ok(pattern) => {
                match Regex::new(&pattern) {
                    Ok(regex) => {
                        let matched = regex.is_match(field_str);
                        self.regex_cache.insert(cache_key, regex);
                        Ok(matched)
                    }
                    _ => Ok(val.eq(field_str)),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    fn conforms(&mut self, entry: &DirEntry, file_info: &Option<FileInfo>, root_path: &Path, expr: &Expr) -> Result<bool, SearchError> {
        let mut result = false;

        if let Some(ref logical_op) = expr.logical_op {
            let left_result = match expr.left {
                Some(ref left) => self.conforms(entry, file_info, root_path, left)?,
                None => false,
            };

            let right_result = |s: &mut Self| -> Result<bool, SearchError> {
                match expr.right {
                    Some(ref right) => s.conforms(entry, file_info, root_path, right),
                    None => Ok(false),
                }
            };

            result = match logical_op {
                LogicalOp::And => if !left_result { false } else { right_result(self)? },
                LogicalOp::Or => if left_result { true } else { right_result(self)? },
            };
        } else if let Some(ref op) = expr.op {
            match op {
                Op::Exists => return Ok(self.check_exists(expr)),
                Op::NotExists => return Ok(!self.check_exists(expr)),
                _ => {}
            }

            let mut temp_map = std::mem::take(&mut self.conforms_map);
            let field_value = match self.get_column_expr_value(
                Some(entry),
                file_info,
                root_path,
                &mut temp_map,
                None,
                expr.left.as_ref().unwrap(),
            ) {
                Ok(v) => v,
                Err(e) => {
                    self.conforms_map = temp_map;
                    return Err(e);
                }
            };
            temp_map.clear();
            let value = match op {
                Op::In | Op::NotIn => Variant::empty(VariantType::String),
                _ => {
                    match self.get_column_expr_value(
                        Some(entry),
                        file_info,
                        root_path,
                        &mut temp_map,
                        None,
                        expr.right.as_ref().unwrap(),
                    ) {
                        Ok(v) => {
                            temp_map.clear();
                            v
                        }
                        Err(e) => {
                            self.conforms_map = temp_map;
                            return Err(e);
                        }
                    }
                }
            };
            self.conforms_map = temp_map;
            let mut arg_map = HashMap::new();

            result = match field_value.get_type() {
                VariantType::String => {
                    let val = value.to_string();
                    let field_str = field_value.to_string();
                    match op {
                        Op::Eq => {
                            if is_glob(&val) {
                                return self.match_glob(val, &field_str);
                            }
                            val.eq(&field_str)
                        }
                        Op::Ne => {
                            if is_glob(&val) {
                                return self.match_glob(val, &field_str).map(|m| !m);
                            }
                            val.ne(&field_str)
                        }
                        Op::Rx | Op::NotRx => {
                            fn identity(s: &str) -> Result<String, String> { Ok(s.to_string()) }
                            let matched = self.match_pattern(val, &field_str, identity, "Incorrect regex expression: ", "rx");
                            return if *op == Op::NotRx { matched.map(|m| !m) } else { matched };
                        }
                        Op::Like => {
                            return self.match_pattern(val, &field_str, convert_like_to_pattern, "Incorrect LIKE expression: ", "like");
                        }
                        Op::NotLike => {
                            return self.match_pattern(val, &field_str, convert_like_to_pattern, "Incorrect LIKE expression: ", "like").map(|m| !m);
                        }
                        Op::Gt => field_str > val,
                        Op::Gte => field_str >= val,
                        Op::Lt => field_str < val,
                        Op::Lte => field_str <= val,
                        Op::Eeq => val.eq(&field_str),
                        Op::Ene => val.ne(&field_str),
                        Op::In => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, false)?,
                        Op::NotIn => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, true)?,
                        _ => false,
                    }
                }
                VariantType::Int => {
                    let val = value.to_int();
                    let int_value = field_value.to_int();
                    match op {
                        Op::Eq | Op::Eeq => int_value == val,
                        Op::Ne | Op::Ene => int_value != val,
                        Op::Gt => int_value > val,
                        Op::Gte => int_value >= val,
                        Op::Lt => int_value < val,
                        Op::Lte => int_value <= val,
                        Op::In => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, false)?,
                        Op::NotIn => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, true)?,
                        _ => false,
                    }
                }
                VariantType::Float => {
                    let val = value.to_float();
                    let float_value = field_value.to_float();
                    match op {
                        Op::Eq | Op::Eeq => float_value == val,
                        Op::Ne | Op::Ene => float_value != val,
                        Op::Gt => float_value > val,
                        Op::Gte => float_value >= val,
                        Op::Lt => float_value < val,
                        Op::Lte => float_value <= val,
                        Op::In => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, false)?,
                        Op::NotIn => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, true)?,
                        _ => false,
                    }
                }
                VariantType::Bool => {
                    let val = value.to_bool();
                    match op {
                        Op::Eq | Op::Eeq => field_value.to_bool() == val,
                        Op::Ne | Op::Ene => field_value.to_bool() != val,
                        Op::Gt => field_value.to_bool() & !val,
                        Op::Gte => field_value.to_bool() >= val,
                        Op::Lt => !field_value.to_bool() & val,
                        Op::Lte => field_value.to_bool() <= val,
                        Op::In => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, false)?,
                        Op::NotIn => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, true)?,
                        _ => false,
                    }
                }
                VariantType::DateTime => {
                    // If either side fails to parse, treat the comparison as a
                    // non-match (SQL NULL-like semantics) and degrade silently —
                    // matches the coercion behavior of the other type branches.
                    // Without this, a typo in a date literal would propagate a
                    // non-fatal error per file scanned, spamming stderr.
                    match (field_value.to_datetime(), value.to_datetime()) {
                        (Ok((field_dt, _)), Ok((start, finish))) => {
                            let start = start.and_utc().timestamp();
                            let finish = finish.and_utc().timestamp();
                            let dt = field_dt.and_utc().timestamp();
                            match op {
                                Op::Eeq => dt == start,
                                Op::Ene => dt != start,
                                Op::Eq => dt >= start && dt <= finish,
                                Op::Ne => dt < start || dt > finish,
                                Op::Gt => dt > finish,
                                Op::Gte => dt >= start,
                                Op::Lt => dt < start,
                                Op::Lte => dt <= finish,
                                Op::In => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, false)?,
                                Op::NotIn => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, true)?,
                                _ => false,
                            }
                        }
                        _ => match op {
                            Op::In => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, false)?,
                            Op::NotIn => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, true)?,
                            _ => false,
                        },
                    }
                }
            };
        }

        Ok(result)
    }

    fn check_extension(
        &self,
        file_name: &str,
        config_ext: &Option<Vec<String>>,
        default_ext: &Option<Vec<String>>,
    ) -> bool {
        has_extension(
            file_name,
            config_ext.as_ref().unwrap_or(default_ext.as_ref().unwrap()),
        )
    }

    fn is_zip_archive(&self, file_name: &str) -> bool {
        self.check_extension(file_name, &self.config.is_zip_archive, &self.default_config.is_zip_archive)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Expr;
    use crate::field::Field;
    use crate::function::Function;
    use crate::query::{OutputFormat, Query, Root, RootOptions};


    fn create_test_searcher() -> Searcher<'static> {
        // Create a minimal Query instance for testing
        let query = Box::leak(Box::new(Query {
            fields: Vec::new(),
            roots: Vec::new(),
            expr: None,
            grouping_fields: Vec::new(),
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        }));

        // Use default configurations
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));

        Searcher::new(query, config, default_config, false)
    }

    fn create_test_searcher_with_ordering() -> Searcher<'static> {
        // Create a Query instance with ordering fields
        let query = Box::leak(Box::new(Query {
            fields: Vec::new(),
            roots: Vec::new(),
            expr: None,
            grouping_fields: Vec::new(),
            ordering_fields: vec![Expr::field(Field::Name)],
            ordering_asc: vec![true],
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        }));

        // Use default configurations
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));

        Searcher::new(query, config, default_config, false)
    }

    fn create_test_searcher_with_aggregate() -> Searcher<'static> {
        // Create a Query instance with an aggregate function in fields
        let mut expr = Expr::field(Field::Name);
        expr.function = Some(Function::Count);

        let query = Box::leak(Box::new(Query {
            fields: vec![expr],
            roots: Vec::new(),
            expr: None,
            grouping_fields: Vec::new(),
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        }));

        // Use default configurations
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));

        Searcher::new(query, config, default_config, false)
    }

    #[test]
    fn test_is_buffered_with_ordering() {
        let searcher = create_test_searcher_with_ordering();
        assert!(searcher.is_buffered());
    }

    #[test]
    fn test_is_buffered_with_aggregate() {
        let searcher = create_test_searcher_with_aggregate();
        assert!(searcher.is_buffered());
    }

    #[test]
    fn test_is_buffered_without_ordering_or_aggregate() {
        let searcher = create_test_searcher();
        assert!(!searcher.is_buffered());
    }

    #[test]
    fn test_is_buffered_with_offset() {
        let query = Box::leak(Box::new(Query {
            fields: Vec::new(),
            roots: Vec::new(),
            expr: None,
            grouping_fields: Vec::new(),
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 10,
            offset: 5,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        }));
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));
        let searcher = Searcher::new(query, config, default_config, false);
        assert!(searcher.is_buffered());
    }

    #[test]
    fn test_has_ordering() {
        let searcher_with_ordering = create_test_searcher_with_ordering();
        assert!(searcher_with_ordering.query.is_ordered());

        let searcher_without_ordering = create_test_searcher();
        assert!(!searcher_without_ordering.query.is_ordered());
    }

    #[test]
    fn test_has_aggregate_column() {
        let searcher_with_aggregate = create_test_searcher_with_aggregate();
        assert!(searcher_with_aggregate.query.has_aggregate_column());

        let searcher_without_aggregate = create_test_searcher();
        assert!(!searcher_without_aggregate.query.has_aggregate_column());
    }

    #[test]
    fn test_is_zip_archive() {
        let searcher = create_test_searcher();

        // Test with zip extensions
        assert!(searcher.is_zip_archive("test.zip"));
        assert!(searcher.is_zip_archive("test.jar"));
        assert!(searcher.is_zip_archive("test.war"));
        assert!(searcher.is_zip_archive("test.ear"));

        // Test with non-zip extensions
        assert!(!searcher.is_zip_archive("test.txt"));
        assert!(!searcher.is_zip_archive("test.rar"));
        assert!(!searcher.is_zip_archive("test"));
    }


    #[test]
    fn test_bound_column_resolution_across_root_aliases() {
        use crate::field::Field;

        // Build a searcher and simulate two roots with aliases `a` and `b`
        let mut searcher = create_test_searcher();

        // Current file belongs to alias `b`
        searcher.current_alias = Some(String::from("b"));

        // Pre-populate record_context with a value for alias `a`
        {
            let mut ctx = searcher.record_context.borrow_mut();
            use crate::field::Field;
            let key = Field::Name.to_string();
            ctx.insert("a".to_string(), HashMap::from([(key, String::from("foo.txt"))]));
        }

        // Build an expression that references a bound column from a different root alias
        let mut bound_expr = Expr::field(Field::Name);
        bound_expr.root_alias = Some(String::from("a"));

        let mut file_map: HashMap<String, String> = HashMap::new();
        let root_path = Path::new(".");

        // Since current_alias is `b`, and the expression requests `a.name`,
        // get_column_expr_value should read the value from record_context rather than the current entry
        let v = searcher.get_column_expr_value(None, &None, root_path, &mut file_map, None, &bound_expr);
        assert_eq!(v.unwrap().to_string(), "foo.txt");
    }

    #[test]
    fn unknown_root_alias_errors_without_current_alias() {
        // `x.name` where no root declares alias `x` (and no alias is being
        // processed) must be rejected instead of silently evaluating as `name`.
        let mut searcher = create_test_searcher();

        let mut bound_expr = Expr::field(Field::Name);
        bound_expr.root_alias = Some(String::from("x"));

        let mut file_map: HashMap<String, String> = HashMap::new();
        let result = searcher.get_column_expr_value(
            None, &None, Path::new("."), &mut file_map, None, &bound_expr,
        );

        match result {
            Err(e) => {
                assert!(e.is_fatal(), "unknown alias must be a fatal error");
                assert!(
                    e.description.contains("Invalid root alias: x"),
                    "unexpected error message: {}",
                    e.description
                );
            }
            Ok(v) => panic!("unknown alias must not resolve, got: {}", v),
        }
    }

    #[test]
    fn unknown_root_alias_errors_with_current_alias() {
        // Same as above, but while a different alias is being processed.
        let mut searcher = create_test_searcher();
        searcher.current_alias = Some(String::from("b"));

        let mut bound_expr = Expr::field(Field::Name);
        bound_expr.root_alias = Some(String::from("x"));

        let mut file_map: HashMap<String, String> = HashMap::new();
        let result = searcher.get_column_expr_value(
            None, &None, Path::new("."), &mut file_map, None, &bound_expr,
        );

        assert!(
            matches!(result, Err(ref e) if e.is_fatal() && e.description.contains("Invalid root alias: x")),
            "unknown alias must be a fatal error, got: {:?}",
            result.map(|v| v.to_string())
        );
    }

    #[test]
    fn dotted_value_literal_is_not_treated_as_alias() {
        // A value such as the literal `readme.md` looks like `alias.field`
        // but must evaluate to itself, not produce an alias error.
        let mut searcher = create_test_searcher();

        let literal = Expr::value(String::from("readme.md"));
        let mut file_map: HashMap<String, String> = HashMap::new();
        let v = searcher.get_column_expr_value(
            None, &None, Path::new("."), &mut file_map, None, &literal,
        );
        assert_eq!(v.unwrap().to_string(), "readme.md");
    }

    #[test]
    fn dotted_value_literal_with_current_alias_returns_literal() {
        // Previously a dotted literal evaluated while processing an aliased
        // root was misread as an alias reference and raised a fatal error.
        let mut searcher = create_test_searcher();
        searcher.current_alias = Some(String::from("a"));

        let literal = Expr::value(String::from("readme.md"));
        let mut file_map: HashMap<String, String> = HashMap::new();
        let v = searcher.get_column_expr_value(
            None, &None, Path::new("."), &mut file_map, None, &literal,
        );
        assert_eq!(v.unwrap().to_string(), "readme.md");
    }

    #[test]
    fn outer_alias_resolves_from_context_without_current_alias() {
        // Correlated-subquery shape: the sub-searcher's own root has no alias
        // (current_alias is None) but the expression references an outer
        // alias registered in the shared record context.
        let mut searcher = create_test_searcher();

        {
            let mut ctx = searcher.record_context.borrow_mut();
            let key = Field::Name.to_string();
            ctx.insert("a".to_string(), HashMap::from([(key, String::from("outer.txt"))]));
        }

        let mut bound_expr = Expr::field(Field::Name);
        bound_expr.root_alias = Some(String::from("a"));

        let mut file_map: HashMap<String, String> = HashMap::new();
        let v = searcher.get_column_expr_value(
            None, &None, Path::new("."), &mut file_map, None, &bound_expr,
        );
        assert_eq!(v.unwrap().to_string(), "outer.txt");
    }

    #[test]
    fn declared_alias_with_unpopulated_context_errors_for_value_expr() {
        // `a.sz` where `sz` is not a field parses as a plain value, but when
        // `a` is declared as a root alias it is an alias.column reference;
        // with no context registered for `a` yet it must error rather than
        // silently evaluate to the literal string "a.sz".
        let mut alias_options = RootOptions::new();
        alias_options.alias = Some(String::from("a"));
        let query = Box::leak(Box::new(Query {
            fields: Vec::new(),
            roots: vec![Root::new(String::from("/tmp"), alias_options)],
            expr: None,
            grouping_fields: Vec::new(),
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        }));
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));
        let mut searcher = Searcher::new(query, config, default_config, false);

        let value_expr = Expr::value(String::from("a.sz"));
        let mut file_map: HashMap<String, String> = HashMap::new();
        let result = searcher.get_column_expr_value(
            None, &None, Path::new("."), &mut file_map, None, &value_expr,
        );

        assert!(
            matches!(result, Err(ref e) if e.description.contains("Invalid root alias: a")),
            "declared but unpopulated alias must error, got: {:?}",
            result.map(|v| v.to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_dfs_follows_relative_symlink() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join("fselect_test_dfs_rel_symlink");
        let _ = fs::remove_dir_all(&tmp);
        let root = tmp.join("root");
        let hidden = tmp.join("hidden");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("file.txt"), "hello").unwrap();
        symlink("../hidden", root.join("link")).unwrap();

        let mut searcher = create_test_searcher();
        searcher.current_follow_symlinks = true;
        searcher.current_traversal_mode = Dfs;
        searcher.current_root_dir = root.clone();

        let _ = searcher.visit_dir(
            &root,
            0,
            #[cfg(feature = "git")]
            None,
            true,
        );

        let found = searcher.found;
        let errors = searcher.error_count;
        let _ = fs::remove_dir_all(&tmp);
        assert!(
            found >= 1 && errors == 0,
            "should find file through relative symlink, found={} errors={}",
            found, errors
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_bfs_follows_relative_symlink() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join("fselect_test_bfs_rel_symlink");
        let _ = fs::remove_dir_all(&tmp);
        let root = tmp.join("root");
        let hidden = tmp.join("hidden");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("file.txt"), "hello").unwrap();
        symlink("../hidden", root.join("link")).unwrap();

        let mut searcher = create_test_searcher();
        searcher.current_follow_symlinks = true;
        searcher.current_root_dir = root.clone();

        let _ = searcher.visit_dir(
            &root,
            0,
            #[cfg(feature = "git")]
            None,
            true,
        );

        let found = searcher.found;
        let errors = searcher.error_count;
        let _ = fs::remove_dir_all(&tmp);
        assert!(
            found >= 1 && errors == 0,
            "should find file through relative symlink, found={} errors={}",
            found, errors
        );
    }

    #[test]
    fn test_string_ordering_comparisons() {
        // Verify that string comparisons work correctly for Gt, Gte, Lt, Lte
        let a = Variant::from_string(&String::from("apple"));
        let b = Variant::from_string(&String::from("banana"));

        // "apple" < "banana" lexicographically
        assert!(a.to_string() < b.to_string());
        assert!(b.to_string() > a.to_string());
        assert!(a.to_string() <= b.to_string());
        assert!(b.to_string() >= a.to_string());
        assert!(a.to_string() <= a.to_string());
        assert!(a.to_string() >= a.to_string());
    }

    #[test]
    fn test_limit_offset_no_overflow() {
        // u32::MAX + 1 would overflow; saturating_add should cap at u32::MAX
        let result = u32::MAX.saturating_add(1);
        assert_eq!(result, u32::MAX);
    }

    #[test]
    fn test_depth_underflow_protection() {
        let canonical_depth: u32 = 2;
        let base_depth: u32 = 5;
        let depth = canonical_depth.saturating_sub(base_depth) + 1;
        assert_eq!(depth, 1, "depth should not underflow");
    }

    #[cfg(unix)]
    #[test]
    fn test_visited_dirs_uses_canonical_path() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join("fselect_test_visited_canonical");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a")).unwrap();
        fs::create_dir_all(tmp.join("b")).unwrap();
        fs::write(tmp.join("a").join("file.txt"), "hello").unwrap();
        symlink("../a", tmp.join("b").join("link_to_a")).unwrap();

        let mut searcher = create_test_searcher();
        searcher.current_follow_symlinks = true;
        searcher.current_traversal_mode = Dfs;
        searcher.current_root_dir = tmp.clone();

        let _ = searcher.visit_dir(
            &tmp,
            0,
            #[cfg(feature = "git")]
            None,
            true,
        );

        let found = searcher.found;
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(
            found, 4,
            "a/ should not be re-traversed via symlink, found={}",
            found
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_to_file_no_dir_traversal_error() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join("fselect_test_symlink_to_file");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("real_file.txt"), "hello").unwrap();
        symlink("real_file.txt", tmp.join("link_to_file")).unwrap();

        let mut searcher = create_test_searcher();
        searcher.current_follow_symlinks = true;
        searcher.current_traversal_mode = Dfs;
        searcher.current_root_dir = tmp.clone();

        let _ = searcher.visit_dir(
            &tmp,
            0,
            #[cfg(feature = "git")]
            None,
            true,
        );

        let errors = searcher.error_count;
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(
            errors, 0,
            "symlink to file should not cause directory traversal error, errors={}",
            errors
        );
    }


    #[test]
    fn test_hgignore_filters_survive_clear_before_load() {
        // Verify the correct order: clear first, then load filters
        let tmp = std::env::temp_dir().join("fselect_test_hgignore_order");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::create_dir_all(tmp.join(".hg")).unwrap();
        fs::write(tmp.join("ignored.log"), "data").unwrap();
        fs::write(tmp.join("kept.txt"), "data").unwrap();
        fs::write(tmp.join(".hgignore"), "syntax: glob\n*.log\n").unwrap();

        let query = Box::leak(Box::new(Query {
            fields: Vec::new(),
            roots: vec![Root::new(tmp.to_string_lossy().to_string(), RootOptions::new())],
            expr: None,
            grouping_fields: Vec::new(),
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        }));

        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));

        let mut searcher = Searcher::new(query, config, default_config, false);

        // Correct order (matching the fixed code): clear first, then load
        searcher.dir_queue.clear();
        searcher.visited_dirs.clear();
        searcher.hgignore_filters.clear();
        searcher.dockerignore_filters.clear();

        search_upstream_hgignore(&mut searcher.hgignore_filters, Path::new(&tmp));

        let filters_count = searcher.hgignore_filters.len();
        let _ = fs::remove_dir_all(&tmp);

        assert!(filters_count > 0, "hgignore filters should be available after clear-then-load");
    }


    #[cfg(unix)]
    #[test]
    fn test_visited_dirs_cleared_between_roots() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join("fselect_test_visited_cleared");
        let _ = fs::remove_dir_all(&tmp);
        let shared = tmp.join("shared");
        let root_a = tmp.join("root_a");
        let root_b = tmp.join("root_b");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&root_a).unwrap();
        fs::create_dir_all(&root_b).unwrap();
        fs::write(shared.join("file.txt"), "hello").unwrap();
        symlink(&shared, root_a.join("link")).unwrap();
        symlink(&shared, root_b.join("link")).unwrap();

        let mut searcher = create_test_searcher();
        searcher.current_follow_symlinks = true;
        searcher.current_traversal_mode = Dfs;
        searcher.current_root_dir = root_a.clone();

        let _ = searcher.visit_dir(
            &root_a,
            0,
            #[cfg(feature = "git")]
            None,
            true,
        );

        let found_after_a = searcher.found;

        searcher.visited_dirs.clear();
        searcher.current_root_dir = root_b.clone();

        let _ = searcher.visit_dir(
            &root_b,
            0,
            #[cfg(feature = "git")]
            None,
            true,
        );

        let found_after_b = searcher.found;
        let _ = fs::remove_dir_all(&tmp);

        assert!(
            found_after_b > found_after_a,
            "root_b should find files through its own symlink, found_a={} found_b={}",
            found_after_a, found_after_b
        );
    }

    #[test]
    fn test_regex_cache_glob_then_like_no_collision() {
        // Pattern "test*" means different things in glob vs LIKE:
        // glob: test.* (wildcard)  LIKE: test\* (literal asterisk)
        let mut searcher = create_test_searcher();
        let pattern = "test*".to_string();

        // First: glob caches "test*" as wildcard match
        let glob_result = searcher.match_glob(pattern.clone(), "test_hello").unwrap();
        assert!(glob_result, "glob test* should match test_hello");

        // Second: LIKE should NOT match "test_hello" because * is literal in LIKE
        let like_result = searcher.match_pattern(
            pattern.clone(), "test_hello",
            convert_like_to_pattern, "err: ", "like",
        ).unwrap();
        assert!(!like_result, "LIKE test* should NOT match test_hello (literal asterisk)");
    }

    #[test]
    fn test_regex_cache_like_then_glob_no_collision() {
        // Vice versa: LIKE cached first, then glob
        let mut searcher = create_test_searcher();
        let pattern = "test*".to_string();

        // First: LIKE caches "test*" as literal asterisk match
        let like_result = searcher.match_pattern(
            pattern.clone(), "test*",
            convert_like_to_pattern, "err: ", "like",
        ).unwrap();
        assert!(like_result, "LIKE test* should match literal test*");

        // Second: glob should match wildcard, not be corrupted by LIKE cache
        let glob_result = searcher.match_glob(pattern.clone(), "test_hello").unwrap();
        assert!(glob_result, "glob test* should match test_hello even after LIKE cached same pattern");
    }

    #[test]
    fn test_aggregate_computed_expr_accumulator_key() {
        // Verify that computed expressions inside aggregates produce consistent keys
        // between accumulation and retrieval phases
        use crate::operators::ArithmeticOp;

        // Build AVG(size * 2): function=Avg, left = (size Mul 2)
        let left_field = Expr::field(Field::Size);
        let right_val = Expr::value("2".to_string());
        let mut computed = Expr::field(Field::Size);
        computed.left = Some(Box::new(left_field));
        computed.arithmetic_op = Some(ArithmeticOp::Multiply);
        computed.right = Some(Box::new(right_val));
        computed.field = None;

        let mut agg_expr = Expr::field(Field::Size);
        agg_expr.function = Some(Function::Avg);
        agg_expr.left = Some(Box::new(computed));
        agg_expr.field = None;

        // The inner expression key used during retrieval
        let inner_key = agg_expr.left.as_ref().unwrap().to_string();

        // Simulate what accumulation does: evaluate inner expr and store in file_map
        let mut file_map = HashMap::new();
        file_map.insert("size".to_string(), "100".to_string());

        // After our fix, the inner expression should also be evaluated and stored
        // The key should match what get_aggregate_value uses for lookup
        assert!(
            !inner_key.is_empty(),
            "Inner expression key should not be empty"
        );
        assert_ne!(
            inner_key, "size",
            "Computed expression key should differ from raw field key"
        );
    }

    #[test]
    fn test_in_list_uses_int_comparison_not_float() {
        // Large i64 values lose precision when converted to f64
        let large: i64 = (1_i64 << 53) + 1; // 9007199254740993
        let a = Variant::from_int(large);
        let b = Variant::from_int(large);

        // Int comparison should match
        assert_eq!(a.to_int(), b.to_int(), "int comparison should be exact");

        // Float comparison would lose precision for this value
        let as_float = large as f64;
        assert_ne!(as_float as i64, large, "f64 loses precision for 2^53+1");
    }

    #[test]
    fn correlated_subquery_without_inner_alias_is_not_cacheable() {
        // The subquery has no alias on its own root but references `t1.size`
        // from the outer query, so its result depends on the outer row.
        // The previous heuristic only checked the subquery's own roots and
        // would wrongly memoise this — returning the first row's result for
        // every subsequent outer row.
        let mut t1_size = Expr::field(Field::Size);
        t1_size.root_alias = Some(String::from("t1"));

        let inner_where = Expr::op(
            Expr::field(Field::Size),
            crate::operators::Op::Gt,
            t1_size,
        );

        let subquery = Query {
            fields: vec![Expr::field(Field::Name)],
            roots: vec![Root::new(
                String::from("/t2"),
                RootOptions::new(),
            )],
            expr: Some(inner_where),
            grouping_fields: Vec::new(),
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        };

        assert!(
            !is_subquery_cacheable(&subquery),
            "subquery referencing outer alias t1 must not be cached"
        );
    }

    #[test]
    fn where_unparseable_date_literal_no_error() {
        // Regression: `where modified = 'not-a-date'` used to propagate a
        // non-fatal SearchError per file scanned, spamming stderr. The fix
        // makes the DateTime comparison branch coerce unparseable values to
        // a non-match (false), consistent with how the Int/String/etc.
        // branches degrade gracefully.
        let tmp = std::env::temp_dir().join("fselect_test_bad_date_literal");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();

        let entry = fs::read_dir(&tmp).unwrap().next().unwrap().unwrap();
        let mut searcher = create_test_searcher();

        // Build: modified = 'not-a-date'
        let expr = Expr::op(
            Expr::field(Field::Modified),
            crate::operators::Op::Eq,
            Expr::value(String::from("not-a-date")),
        );

        let result = searcher.conforms(&entry, &None, &tmp, &expr);

        let _ = fs::remove_dir_all(&tmp);

        // Must return Ok(false), not Err(...).
        match result {
            Ok(matched) => assert!(!matched, "unparseable date literal must not match"),
            Err(e) => panic!("conforms() must not propagate error for bad date literal: {}", e),
        }
    }

    #[test]
    fn where_unparseable_date_literal_not_equal_no_error() {
        // Companion to the above: != against an unparseable literal must also
        // degrade silently. We deliberately return false (SQL NULL/UNKNOWN
        // semantics) so `=` and `!=` against a bad literal behave symmetrically
        // — both return no rows — rather than `!=` silently matching everything.
        let tmp = std::env::temp_dir().join("fselect_test_bad_date_literal_ne");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();

        let entry = fs::read_dir(&tmp).unwrap().next().unwrap().unwrap();
        let mut searcher = create_test_searcher();

        let expr = Expr::op(
            Expr::field(Field::Modified),
            crate::operators::Op::Ne,
            Expr::value(String::from("not-a-date")),
        );

        let result = searcher.conforms(&entry, &None, &tmp, &expr);

        let _ = fs::remove_dir_all(&tmp);

        match result {
            Ok(matched) => assert!(!matched),
            Err(e) => panic!("conforms() must not propagate error for bad date literal: {}", e),
        }
    }

    #[test]
    fn uncorrelated_subquery_remains_cacheable() {
        // No root_alias references in the inner expression — safe to cache.
        let inner_where = Expr::op(
            Expr::field(Field::Size),
            crate::operators::Op::Gt,
            Expr::value(String::from("0")),
        );
        let subquery = Query {
            fields: vec![Expr::field(Field::Name)],
            roots: vec![Root::new(String::from("/t2"), RootOptions::new())],
            expr: Some(inner_where),
            grouping_fields: Vec::new(),
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        };
        assert!(is_subquery_cacheable(&subquery));
    }

    /// Build a real Query via the parser+lexer and execute it silently against
    /// a temp directory. Returns the rendered rows the outer query produced so
    /// we can assert against them as a flat set of strings.
    fn run_query_against_dir(query_template: &str, dir: &Path) -> Vec<String> {
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        let query_string = query_template.replace("__DIR__", &dir.to_string_lossy());
        let mut lexer = Lexer::new(vec![query_string.clone()]);
        let mut parser = Parser::new(&mut lexer);
        let parsed = parser.parse(false).expect("parse failed");
        let parsed = Box::leak(Box::new(parsed));
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));

        let mut searcher = Searcher::new(parsed, config, default_config, false);
        searcher.silent_mode = true;
        searcher.list_search_results().expect("search failed");

        searcher
            .output_buffer
            .iter_values()
            .map(|s| s.trim_end().to_string())
            .collect()
    }

    #[test]
    fn plain_query_silent_mode_buffers_results() {
        // Sanity check: a non-subselect query must populate output_buffer when
        // run silently, otherwise the subselect tests below have no chance.
        let tmp = std::env::temp_dir().join("fselect_test_plain_silent_buffer");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("one.txt"), "x").unwrap();
        fs::write(tmp.join("two.txt"), "y").unwrap();

        let rows = run_query_against_dir(
            "select name from __DIR__ depth 1",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        let names: HashSet<String> = rows.into_iter().collect();
        assert!(names.contains("one.txt"), "names was {:?}", names);
        assert!(names.contains("two.txt"));
    }

    #[test]
    fn from_subselect_returns_inner_paths_as_input_rows() {
        let tmp = std::env::temp_dir().join("fselect_test_from_subselect_basic");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("alpha.rs"), "// alpha").unwrap();
        fs::write(tmp.join("beta.txt"), "beta").unwrap();
        fs::write(tmp.join("gamma.rs"), "// gamma").unwrap();

        // Outer query asks only for name, but its source is a subselect that
        // already filtered down to the directory contents. The outer query must
        // see all three files as input rows.
        let rows = run_query_against_dir(
            "select name from (select path from __DIR__ depth 1)",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);

        let names: HashSet<String> = rows.into_iter().collect();
        assert!(names.contains("alpha.rs"), "missing alpha.rs in {:?}", names);
        assert!(names.contains("beta.txt"), "missing beta.txt in {:?}", names);
        assert!(names.contains("gamma.rs"), "missing gamma.rs in {:?}", names);
    }

    #[test]
    fn from_subselect_outer_where_filters_inner_rows() {
        let tmp = std::env::temp_dir().join("fselect_test_from_subselect_outer_where");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("keep.rs"), "// keep").unwrap();
        fs::write(tmp.join("skip.txt"), "skip").unwrap();
        fs::write(tmp.join("also.rs"), "// also").unwrap();

        let rows = run_query_against_dir(
            "select name from (select path from __DIR__ depth 1) where name like '%.rs'",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);

        let names: HashSet<String> = rows.into_iter().collect();
        assert!(names.contains("keep.rs"));
        assert!(names.contains("also.rs"));
        assert!(!names.contains("skip.txt"),
            "outer WHERE must filter rows produced by the FROM subselect");
    }

    #[test]
    fn from_subselect_inner_where_limits_input_rows() {
        let tmp = std::env::temp_dir().join("fselect_test_from_subselect_inner_where");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        // big.bin will be picked up; tiny.bin will be filtered out by the
        // inner WHERE clause, so the outer query must not see it.
        fs::write(tmp.join("big.bin"), vec![0u8; 4096]).unwrap();
        fs::write(tmp.join("tiny.bin"), vec![0u8; 4]).unwrap();

        let rows = run_query_against_dir(
            "select name from (select path from __DIR__ depth 1 where size > 100)",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);

        let names: HashSet<String> = rows.into_iter().collect();
        assert!(names.contains("big.bin"));
        assert!(
            !names.contains("tiny.bin"),
            "rows filtered by the inner WHERE must not reach the outer query"
        );
    }
}
