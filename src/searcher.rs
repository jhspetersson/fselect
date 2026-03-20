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
    dir_queue: VecDeque<PathBuf>,
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
                TopN::new(limit + query.offset)
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
            conforms_map: HashMap::new(),
            subquery_cache: HashMap::new(),
            silent_mode: false,

            error_count: 0,
        }
    }

    pub fn is_buffered(&self) -> bool {
        self.query.is_ordered() || self.query.has_aggregate_column() || self.silent_mode
    }

    /// Searches directories based on configured query and outputs results to stdout.
    pub fn list_search_results(&mut self) -> Result<(), SearchError> {
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
                                        if let Ok(file_type) = entry.file_type() {
                                            if file_type.is_dir()
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

            let result = self.visit_dir(
                &self.current_root_dir.clone(),
                0,
                #[cfg(feature = "git")]
                Repository::discover(&self.current_root_dir).ok().as_ref(),
                true,
            );

            if let Err(err) = result {
                if err.is_fatal() {
                    return Err(err);
                }
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
                        TopN::new(self.query.limit + self.query.offset)
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
                            None, &None, &Path::new(""), &mut file_map, Some(group_acc), column_expr,
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
                for items in grouped_results.iter_values().skip(self.query.offset as usize) {
                    let mut buf = WritableBuffer::new();
                    self.results_writer.write_row(&mut buf, items.clone())?;
                    let rendered = String::from(buf);
                    if !self.silent_mode {
                        if !first {
                            try_output!(self.results_writer.write_row_separator(&mut std::io::stdout()), Ok(()));
                        }
                        first = false;
                        try_output!(write!(std::io::stdout(), "{}", &rendered), Ok(()));
                    }
                    self.output_buffer.insert(
                        Criteria::new(Rc::new(vec![]), vec![], Rc::new(vec![])),
                        rendered,
                    );
                }
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
                        &Path::new(""),
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
            let mut first = true;
            for piece in self.output_buffer.iter_values().skip(self.query.offset as usize) {
                if first {
                    first = false;
                } else {
                    try_output!(self.results_writer.write_row_separator(&mut std::io::stdout()), Ok(()));
                }
                try_output!(write!(std::io::stdout(), "{}", piece), Ok(()));
            }
        }

        if !self.silent_mode {
            self.results_writer.write_footer(&mut std::io::stdout())?;
        }

        let completion_time = std::time::Instant::now();

        if self.config.debug {
            eprintln!("Search: {}ms\nCompute: {}ms", 
                      compute_time.duration_since(start_time).as_millis(), 
                      completion_time.duration_since(compute_time).as_millis());
        }

        Ok(())
    }

    fn get_list_from_subquery(&mut self, query: Query) -> Vec<String> {
        let query_str = format!("{:?}", query);
        
        let ok_to_cache = query.roots.iter().all(|root| root.options.alias.is_none());
        if ok_to_cache {
            if let Some(cached) = self.subquery_cache.get(&query_str) {
                return cached.clone();
            }
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
        let canonical_path = match canonical_path(&dir.to_path_buf()) {
            Ok(path) => path,
            Err(e) => {
                self.error_count += 1;
                error_message(
                    &dir.to_string_lossy(),
                    &format!("could not canonicalize path: {}", e),
                );
                return Ok(());
            }
        };

        // Prevents infinite loops when following symlinks
        if self.current_follow_symlinks {
            let canonical_pathbuf = PathBuf::from(&canonical_path);
            if self.visited_dirs.contains(&canonical_pathbuf) {
                return Ok(());
            } else {
                self.visited_dirs.insert(canonical_pathbuf);
            }
        }
        let canonical_depth = calc_depth(&canonical_path);

        let base_depth = match root_depth {
            0 => canonical_depth,
            _ => root_depth,
        };

        let depth = canonical_depth.saturating_sub(base_depth) + 1;

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
                                let canonical_entry_path = PathBuf::from(&canonical_path).join(entry.file_name());

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
                                if self.current_min_depth == 0 || depth >= self.current_min_depth {
                                    let checked = self.check_file(&entry, &root_dir, &None);
                                    match checked {
                                        Err(err) => {
                                            if err.is_fatal() {
                                                return Err(err);
                                            }
                                            self.handle_nonfatal_error(err, &path);
                                            continue;
                                        }
                                        Ok(()) => {}
                                    }

                                    if self.current_search_archives
                                        && self.is_zip_archive(&path.to_string_lossy())
                                    {
                                        if let Ok(file) = fs::File::open(&path) {
                                            if let Ok(mut archive) = zip::ZipArchive::new(file) {
                                                for i in 0..archive.len() {
                                                    if !self.is_buffered() && self.query.limit > 0
                                                        && self.query.limit <= self.found
                                                    {
                                                        break;
                                                    }

                                                    if let Ok(afile) = archive.by_index(i) {
                                                        let file_info = to_file_info(&afile);
                                                        match self.check_file(&entry, &root_dir, &Some(file_info)) {
                                                            Err(err) => {
                                                                if err.is_fatal() {
                                                                    return Err(err);
                                                                }
                                                                self.handle_nonfatal_error(err, &path);
                                                                continue;
                                                            }
                                                            Ok(()) => {}
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                // Recursively visit subdirectories if we're not too deep
                                if self.current_max_depth == 0 || depth < self.current_max_depth {
                                    match entry.file_type() {
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
                                                        base_depth,
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
                                                    self.dir_queue.push_back(path);
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
                let path = self.dir_queue.pop_front().unwrap();
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
                    base_depth,
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
            if let Some(ref current_alias) = self.current_alias {
                if column_expr_context_name != current_alias {
                    let context = self.record_context.borrow();
                    if let Some(ctx) = context.get(column_expr_context_name) {
                        if let Some(val) = ctx.get(captures.get(2).unwrap().as_str()) {
                            return Ok(Variant::from_string(val));
                        } else {
                            //TODO: this should be propagated up to the higher context
                            return Ok(Variant::empty(VariantType::String));
                        }
                    } else {
                        return Err(SearchError::fatal(format!("Invalid root alias: {}", column_expr_context_name)).with_source("query"));
                    }
                } else {
                    should_update_context = true;
                }
            }
        }

        if file_map.contains_key(&column_expr_str) {
            if should_update_context {
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_insert(HashMap::new());
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
            if entry.is_some() {
                let result = self.get_field_value(entry.unwrap(), file_info, root_path, field).unwrap_or(Variant::empty(VariantType::String));
                file_map.insert(column_expr_str, result.to_string());
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_insert(HashMap::new());
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
            return Ok(Variant::from_signed_string(&value, column_expr.minus));
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
            follow_symlinks: self.current_follow_symlinks,
            config: self.config,
            default_config: self.default_config,
            #[cfg(all(unix, feature = "users"))]
            user_cache: &self.user_cache,
        };
        dispatch::get_field_value(&mut ctx, field)
    }

    fn check_file(&mut self, entry: &DirEntry, root_path: &Path, file_info: &Option<FileInfo>) -> Result<(), SearchError> {
        self.fms.clear();

        let mut file_map = HashMap::new();

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
                let context_entry = context.entry(current_alias.to_string()).or_insert(HashMap::new());
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
            for field in self.query.grouping_fields.iter() {
                if file_map.get(&field.to_string()).is_none() {
                    self.get_column_expr_value(Some(entry), file_info, root_path, &mut file_map, None, field)?;
                }
            }
            let group_key: Vec<String> = self.query.grouping_fields.iter()
                .map(|f| file_map.get(&f.to_string()).cloned().unwrap_or_default())
                .collect();
            let accumulator = self.accumulators.entry(group_key).or_default();
            accumulator.increment_count();
            for (key, value) in &file_map {
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
                self.get_column_expr_value(Some(entry), file_info, root_path, &mut file_map, None, field)?;

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
                    .get_column_expr_value(Some(entry), file_info, root_path, &mut file_map, None, field)?
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
                VariantType::Int | VariantType::Float => arg_val.to_float() == field_value.to_float(),
                VariantType::Bool => arg_val.to_bool() == field_value.to_bool(),
                VariantType::DateTime => {
                    arg_val.to_datetime()?.0.and_utc().timestamp()
                        == field_value.to_datetime()?.0.and_utc().timestamp()
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
    ) -> Result<bool, SearchError> {
        if let Some(regex) = self.regex_cache.get(&val) {
            return Ok(regex.is_match(field_str));
        }
        match converter(&val) {
            Ok(pattern) => {
                match Regex::new(&pattern) {
                    Ok(regex) => {
                        let matched = regex.is_match(field_str);
                        self.regex_cache.insert(val, regex);
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
        if let Some(regex) = self.regex_cache.get(&val) {
            return Ok(regex.is_match(field_str));
        }
        match convert_glob_to_pattern(&val) {
            Ok(pattern) => {
                match Regex::new(&pattern) {
                    Ok(regex) => {
                        let matched = regex.is_match(field_str);
                        self.regex_cache.insert(val, regex);
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
                            let matched = self.match_pattern(val, &field_str, identity, "Incorrect regex expression: ");
                            return if *op == Op::NotRx { matched.map(|m| !m) } else { matched };
                        }
                        Op::Like => {
                            return self.match_pattern(val, &field_str, convert_like_to_pattern, "Incorrect LIKE expression: ");
                        }
                        Op::NotLike => {
                            return self.match_pattern(val, &field_str, convert_like_to_pattern, "Incorrect LIKE expression: ").map(|m| !m);
                        }
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
                        Op::Gt => field_value.to_bool() > val,
                        Op::Gte => field_value.to_bool() >= val,
                        Op::Lt => field_value.to_bool() < val,
                        Op::Lte => field_value.to_bool() <= val,
                        Op::In => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, false)?,
                        Op::NotIn => self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, true)?,
                        _ => false,
                    }
                }
                VariantType::DateTime => {
                    let (start, finish) = value.to_datetime()?;
                    let start = start.and_utc().timestamp();
                    let finish = finish.and_utc().timestamp();
                    let dt = field_value.to_datetime()?.0.and_utc().timestamp();
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

}
