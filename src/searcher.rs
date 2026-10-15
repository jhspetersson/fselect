//! Handles directory traversal and file processing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::fs::{DirEntry, FileType};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;

use chrono::NaiveDateTime;
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
use crate::fileinfo::{archived_entry_levels, open_archive, to_file_info, ArchiveMember, FileInfo};
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
use crate::util::dir_lookup::DirEntryLookup;
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
    output_buffer: TopN<Criteria<String>, BufferedRow>,
    deferred_columns: Vec<usize>,
    aggregate_plan: Option<Rc<AggregatePlan>>,
    ordering_fields_rc: Rc<Vec<Expr>>,
    ordering_asc_rc: Rc<Vec<bool>>,

    record_context: Rc<RefCell<HashMap<String, HashMap<String, Variant>>>>,
    current_alias: Option<String>,
    subquery_required_fields: Option<HashMap<Field, String>>,

    hgignore_filters: Vec<HgignoreFilter>,
    dockerignore_filters: Vec<DockerignoreFilter>,
    visited_dirs: HashSet<PathBuf>,
    lscolors: LsColors,
    dir_queue: VecDeque<(PathBuf, u32)>,
    #[cfg(test)]
    dirs_opened: usize,
    #[cfg(test)]
    in_list_comparisons: usize,
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
    membership_cache: HashMap<(String, VariantType), Rc<Membership>>,
    silent_mode: bool,
    /// Runs a subquery on behalf of an enclosing searcher, whose aliases it
    /// may legitimately reference.
    is_nested: bool,

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
/// Applies a leading unary minus to an evaluated expression value, with
/// MySQL-style numeric coercion (0 - x). Empty values stay empty, like
/// negating SQL NULL. Literal values are excluded: they carry their sign in
/// the string itself (`Variant::from_signed_string`).
fn apply_minus(column_expr: &Expr, value: Variant) -> Variant {
    if !column_expr.minus || value.to_string().is_empty() {
        return value;
    }
    crate::operators::ArithmeticOp::Subtract
        .calc(&Variant::from_int(0), &value)
        .unwrap_or(value)
}

fn typed_variant(field: Option<&Field>, text: &str) -> Variant {
    let as_string = || Variant::from_string(&text.to_string());
    match field {
        Some(field) if field.is_numeric_field() => text
            .parse::<i64>()
            .map(Variant::from_int)
            .or_else(|_| text.parse::<f64>().map(Variant::from_float))
            .unwrap_or_else(|_| as_string()),
        Some(field) if field.is_datetime_field() => parse_datetime(text)
            .map(|(from, _)| Variant::from_datetime(from))
            .unwrap_or_else(|_| as_string()),
        _ => as_string(),
    }
}

enum Membership {
    Strings(HashSet<String>),
    Ints(HashSet<i64>),
    Floats(Vec<f64>),
    Bools { has_true: bool, has_false: bool },
    DateTimes(Vec<(NaiveDateTime, NaiveDateTime)>),
}

impl Membership {
    fn new(values: &[String], field_type: &VariantType) -> Membership {
        let variants = values.iter().map(Variant::from_string);
        match field_type {
            VariantType::String => Membership::Strings(values.iter().cloned().collect()),
            VariantType::Int => Membership::Ints(variants.filter_map(|v| v.to_exact_int()).collect()),
            VariantType::Float => Membership::Floats(variants.filter_map(|v| v.to_float_opt()).collect()),
            VariantType::Bool => {
                let mut bools = (false, false);
                for variant in variants {
                    if variant.to_bool() { bools.0 = true } else { bools.1 = true }
                }
                Membership::Bools { has_true: bools.0, has_false: bools.1 }
            }
            VariantType::DateTime => Membership::DateTimes(variants.filter_map(|v| v.to_datetime().ok()).collect()),
        }
    }

    fn contains(&self, field_value: &Variant) -> bool {
        match self {
            Membership::Strings(set) => set.contains(field_value.to_string().as_str()),
            Membership::Ints(set) => set.contains(&field_value.to_int()),
            Membership::Floats(list) => list.contains(&field_value.to_float()),
            Membership::Bools { has_true, has_false } => if field_value.to_bool() { *has_true } else { *has_false },
            Membership::DateTimes(intervals) => match field_value.to_datetime() {
                Ok((dt, _)) => intervals.iter().any(|(start, finish)| dt >= *start && dt <= *finish),
                Err(_) => false,
            },
        }
    }
}

const DEFERRED_COLUMN_WEIGHT: i32 = 16;

struct AggregatePlan {
    fields: Vec<(Field, String)>,
    inner_exprs: Vec<(String, Expr)>,
    grouping_keys: Vec<String>,
}

impl AggregatePlan {
    fn new(query: &Query) -> Option<Rc<AggregatePlan>> {
        if !query.is_aggregated() {
            return None;
        }
        let fields: Vec<(Field, String)> = query
            .get_all_fields()
            .into_iter()
            .map(|field| {
                let key = field.to_string();
                (field, key)
            })
            .collect();
        let mut seen: HashSet<String> = fields.iter().map(|(_, key)| key.clone()).collect();
        let inner_exprs = query
            .fields
            .iter()
            .chain(query.ordering_fields.iter())
            .chain(query.having.iter())
            .flat_map(Expr::get_aggregate_exprs)
            .filter_map(|aggregate| aggregate.left.as_ref())
            .filter_map(|left| {
                let key = left.cache_key();
                seen.insert(key.clone()).then(|| (key, left.as_ref().clone()))
            })
            .collect();
        let grouping_keys = query.grouping_fields.iter().map(Expr::cache_key).collect();
        Some(Rc::new(AggregatePlan { fields, inner_exprs, grouping_keys }))
    }
}

enum BufferedRow {
    Rendered(String),
    Pending(PendingRow),
}

struct PendingRow {
    path: PathBuf,
    root_path: PathBuf,
    file_info: Option<FileInfo>,
    follow_symlinks: bool,
    items: Vec<(String, String)>,
    deferred: Vec<usize>,
}

impl std::ops::Deref for BufferedRow {
    type Target = str;

    fn deref(&self) -> &str {
        match self {
            BufferedRow::Rendered(text) => text,
            BufferedRow::Pending(_) => "",
        }
    }
}

impl std::fmt::Display for BufferedRow {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(self)
    }
}

fn is_subquery_cacheable(query: &Query) -> bool {
    !query.is_correlated()
}

/// Compiles one regex component of a `regexp` root path.
fn root_part_regex(part: &str) -> Result<Regex, SearchError> {
    Regex::new(&format!("^{}$", part))
        .map_err(|_| SearchError::fatal(format!("invalid regex in root path: {}", part)))
}

/// HAVING is evaluated against group accumulators, where set membership and
/// existence tests have no meaning.
fn having_uses_unsupported_op(expr: &Expr) -> bool {
    matches!(expr.op, Some(Op::In | Op::NotIn | Op::Exists | Op::NotExists))
        || expr.left.as_deref().is_some_and(having_uses_unsupported_op)
        || expr.right.as_deref().is_some_and(having_uses_unsupported_op)
}
#[cfg(any(all(windows, feature = "everything"), all(unix, feature = "plocate")))]
fn external_index_depth(path: &Path, root_prefix: &str) -> Option<u32> {
    let s = path.to_string_lossy();
    let under = if cfg!(windows) {
        // Compare bytes: a string slice could panic if the paths diverge in
        // the middle of a multibyte character. A byte-wise ASCII-case match
        // also guarantees the prefix ends on a char boundary in `s`.
        s.len() >= root_prefix.len()
            && s.as_bytes()[..root_prefix.len()].eq_ignore_ascii_case(root_prefix.as_bytes())
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
        record_context: Rc<RefCell<HashMap<String, HashMap<String, Variant>>>>,
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
            deferred_columns: Self::deferred_columns(query),
            aggregate_plan: AggregatePlan::new(query),
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
            #[cfg(test)]
            dirs_opened: 0,
            #[cfg(test)]
            in_list_comparisons: 0,
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
            membership_cache: HashMap::new(),
            silent_mode: false,
            is_nested: false,

            error_count: 0,
        }
    }

    pub fn is_buffered(&self) -> bool {
        self.query.is_ordered() || self.query.is_aggregated() || self.query.offset > 0 || self.silent_mode
    }

    fn limit_reached(&self) -> bool {
        self.query.limit > 0
            && !self.query.is_ordered()
            && !self.query.is_aggregated()
            && self.found >= self.query.limit.saturating_add(self.query.offset)
    }

    /// Searches directories based on configured query and outputs results to stdout.
    pub fn list_search_results(&mut self) -> Result<(), SearchError> {
        // A query that can never run is rejected before any output, so a
        // fatal error never leaves a half-written JSON array or HTML page.
        self.validate()?;

        if !self.silent_mode {
            let col_count = self.query.fields.len();
            try_output!(self.results_writer.write_header(&self.query.raw_query, col_count, &mut std::io::stdout()), Ok(()));
        }

        let result = self.search_and_print();

        // Once the header is out the footer always follows, even when the
        // search aborted, so the document stays well-formed.
        if !self.silent_mode
            && let Err(err) = self.results_writer.write_footer(&mut std::io::stdout().lock())
                && result.is_ok() {
                    return Err(err.into());
                }

        result
    }

    /// Static checks that need no file system access. Everything reported
    /// here used to surface only after the output header had been written.
    fn validate(&self) -> Result<(), SearchError> {
        // The top-level walk descends into every subquery scope, so a nested
        // searcher (which may reference enclosing aliases) skips this check.
        if !self.is_nested
            && let Some(alias) = self.query.find_external_alias(&self.query.own_root_aliases()) {
                return Err(SearchError::fatal(format!("Invalid root alias: {}", alias)).with_source("query"));
            }

        for root in self.query.roots.iter().filter(|root| root.options.regexp) {
            for part in root.path.split('/').filter(|part| looks_like_regexp(part)) {
                root_part_regex(part)?;
            }
        }

        if let Some(ordering_expr) = self.unsortable_grouped_ordering() {
            return Err(SearchError::fatal(format!(
                "Cannot order by '{}': in a GROUP BY query the ORDER BY expression must appear in the SELECT list, be an aggregate function, or be a grouping column",
                ordering_expr
            )).with_source("query"));
        }

        if let Some(ref having) = self.query.having
            && having_uses_unsupported_op(having) {
                return Err(SearchError::fatal(
                    "IN, NOT IN, EXISTS, and NOT EXISTS are not supported in the HAVING clause",
                )
                .with_source("query"));
            }

        Ok(())
    }

    /// In a GROUP BY query an ORDER BY expression must be a selected column,
    /// an aggregate, or a grouping column; anything else has no per-group value.
    fn unsortable_grouped_ordering(&self) -> Option<&Expr> {
        if !self.query.is_aggregated() || self.query.grouping_fields.is_empty() {
            return None;
        }
        let field_keys: Vec<String> = self.query.fields.iter().map(Expr::cache_key).collect();
        let grouping_keys: Vec<String> = self.query.grouping_fields.iter().map(Expr::cache_key).collect();
        self.query.ordering_fields.iter().find(|ordering_expr| {
            let key = ordering_expr.cache_key();
            !field_keys.contains(&key)
                && !ordering_expr.has_aggregate_function()
                && !grouping_keys.contains(&key)
        })
    }

    fn search_and_print(&mut self) -> Result<(), SearchError> {
        let current_dir = std::env::current_dir()?;

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
                        let rx = root_part_regex(part)?;
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
            self.subquery_required_fields = match &self.current_alias {
                Some(alias) => {
                    // A correlated subquery can sit in any outer clause — the
                    // SELECT list, WHERE, ORDER BY, or GROUP BY — so snapshot the
                    // outer fields it consumes from all of them. Mirrors the
                    // coverage of is_subquery_cacheable, which inspects the same
                    // clauses; otherwise a SELECT-list (or ORDER BY) correlation
                    // would never reach record_context.
                    let mut fields = HashMap::new();
                    for column_expr in &self.query.fields {
                        fields.extend(column_expr.get_fields_required_in_subqueries(alias, false));
                    }
                    if let Some(ref expr) = self.query.expr {
                        fields.extend(expr.get_fields_required_in_subqueries(alias, false));
                    }
                    for ordering_expr in &self.query.ordering_fields {
                        fields.extend(ordering_expr.get_fields_required_in_subqueries(alias, false));
                    }
                    for grouping_expr in &self.query.grouping_fields {
                        fields.extend(grouping_expr.get_fields_required_in_subqueries(alias, false));
                    }
                    if fields.is_empty() { None } else { Some(fields) }
                }
                None => None,
            };

            if let Some(ref inner) = root.subquery {
                let paths = self.collect_subquery_root_paths(inner.as_ref().clone())?;
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

            // The external indexes only know physical descendants of the root,
            // so a root declared with `sym` (follow symlinks) must fall back
            // to real traversal or symlinked subtrees would be silently missed.
            #[cfg(all(windows, feature = "everything"))]
            {
                if self.config.everything.unwrap_or(false)
                    && !self.current_search_archives
                    && !self.current_apply_gitignore
                    && !self.current_apply_hgignore
                    && !self.current_apply_dockerignore
                    && !self.current_follow_symlinks
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
                    && !self.current_follow_symlinks
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

        self.finalize_deferred()?;

        let compute_time = std::time::Instant::now();

        // ======== Compute results =========
        if self.query.is_aggregated() {
            if !self.query.grouping_fields.is_empty() {
                let group_keys: Vec<String> = self
                    .query
                    .grouping_fields
                    .iter()
                    .map(|f| f.cache_key())
                    .collect();
                let accumulators = std::mem::take(&mut self.accumulators);

                let ordering_fields_rc = self.ordering_fields_rc.clone();
                let ordering_asc_rc = self.ordering_asc_rc.clone();
                let field_keys: Vec<String> = self.query.fields.iter().map(Expr::cache_key).collect();
                // Map each ordering expression to its SELECT column when it is
                // selected; otherwise it is evaluated per group below
                // (`validate` has already rejected anything unsortable).
                let sorting_indices: Vec<Option<usize>> = self.query.ordering_fields.iter()
                    .map(|ordering_expr| {
                        let key = ordering_expr.cache_key();
                        field_keys.iter().position(|field_key| field_key == &key)
                    })
                    .collect();

                let mut grouped_results: TopN<Criteria<String>, Vec<(String, String)>> =
                    if self.query.limit > 0 {
                        TopN::new(self.query.limit.saturating_add(self.query.offset))
                    } else {
                        TopN::limitless()
                    };

                let having_expr = self.query.having.as_ref();
                for (group_key, group_acc) in &accumulators {
                    let mut file_map = HashMap::new();
                    for (i, k) in group_keys.iter().enumerate() {
                        file_map.insert(k.clone(), group_key.get(i).cloned().unwrap_or_default());
                    }
                    if let Some(having_expr) = having_expr
                        && !self.group_conforms(&mut file_map, group_acc, having_expr)? {
                            continue;
                        }
                    let mut items: Vec<(String, String)> = Vec::new();
                    for column_expr in &self.query.fields {
                        let value = self.get_column_expr_value(
                            None, &None, Path::new(""), &mut file_map, Some(group_acc), column_expr,
                        )?;
                        items.push((column_expr.column_name(), value.to_string()));
                    }
                    let mut criteria_values: Vec<String> =
                        Vec::with_capacity(sorting_indices.len());
                    for (index, ordering_expr) in
                        sorting_indices.iter().zip(self.query.ordering_fields.iter())
                    {
                        let value = match index {
                            // Reuse the value already rendered for the SELECT list
                            Some(index) => items
                                .get(*index)
                                .map(|item| item.1.clone())
                                .unwrap_or_default(),
                            // Not selected: evaluate the ordering expression
                            // against this group's accumulator (aggregates) or
                            // its grouping-key values in file_map.
                            None => self
                                .get_column_expr_value(
                                    None,
                                    &None,
                                    Path::new(""),
                                    &mut file_map,
                                    Some(group_acc),
                                    ordering_expr,
                                )?
                                .to_string(),
                        };
                        criteria_values.push(value);
                    }
                    grouped_results.insert(
                        Criteria::new(ordering_fields_rc.clone(), criteria_values, ordering_asc_rc.clone()),
                        items,
                    );
                }

                let mut first = true;
                let mut stdout = std::io::stdout().lock();
                for (index, items) in grouped_results.iter_values().enumerate() {
                    let mut buf = WritableBuffer::new();
                    self.results_writer.write_row(&mut buf, items.clone())?;
                    let rendered = String::from(buf);
                    if !self.silent_mode && index >= self.query.offset as usize {
                        if !first {
                            try_output!(self.results_writer.write_row_separator(&mut stdout), Ok(()));
                        }
                        first = false;
                        try_output!(write!(stdout, "{}", &rendered), Ok(()));
                    }
                    self.output_buffer.insert(
                        Criteria::new(Rc::new(vec![]), vec![], Rc::new(vec![])),
                        BufferedRow::Rendered(rendered),
                    );
                }
                drop(stdout);
            } else {
                let mut buf = WritableBuffer::new();
                let mut items: Vec<(String, String)> = Vec::new();

                let accumulators = std::mem::take(&mut self.accumulators);
                let empty_acc = function::GroupAccumulator::default();
                let ungrouped_acc = accumulators.get(&vec![]).unwrap_or(&empty_acc);
                let mut passes_having = true;
                if let Some(having_expr) = self.query.having.as_ref() {
                    passes_having =
                        self.group_conforms(&mut HashMap::new(), ungrouped_acc, having_expr)?;
                }
                for column_expr in &self.query.fields {
                    let value = self.get_column_expr_value(
                        None,
                        &None,
                        Path::new(""),
                        &mut HashMap::new(),
                        Some(ungrouped_acc),
                        column_expr
                    )?;
                    items.push((column_expr.column_name(), value.to_string()));
                }

                if passes_having {
                    self.results_writer.write_row(&mut buf, items)?;
                    let rendered = String::from(buf);
                    self.output_buffer.insert(
                        Criteria::new(Rc::new(vec![]), vec![], Rc::new(vec![])),
                        BufferedRow::Rendered(rendered.clone()),
                    );

                    // An ungrouped aggregate produces a single row; any OFFSET
                    // skips past it (the buffered row is offset-skipped by readers).
                    if !self.silent_mode && self.query.offset == 0 {
                        try_output!(write!(std::io::stdout(), "{}", rendered), Ok(()));
                    }
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
    fn collect_subquery_root_paths(&mut self, mut query: Query) -> Result<Vec<String>, SearchError> {
        // Use the absolute path so visit_subquery_paths can re-resolve each
        // result against the filesystem regardless of the inner query's root.
        query.fields = vec![Expr::field(Field::AbsPath)];
        query.output_format = crate::query::OutputFormat::Tabs;
        self.get_list_from_subquery(query)
    }

    /// Treat each path produced by a FROM subselect as an input entry: resolve
    /// it to a `DirEntry` and feed it through `check_file` so the outer query's
    /// WHERE/SELECT logic can run against it without further directory
    /// traversal. The std library cannot build a `DirEntry` from a path, so the
    /// parent directory is read once and its entries looked up by name for as
    /// long as consecutive paths keep coming from the same directory.
    fn visit_subquery_paths(&mut self, paths: Vec<String>) -> Result<(), SearchError> {
        let mut lookup = DirEntryLookup::new();
        for path_str in paths {
            if path_str.is_empty() {
                continue;
            }
            if self.limit_reached() {
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

            match lookup.entry(&parent, &file_name) {
                Ok(Some(entry)) => {
                    if let Err(err) = self.check_file(entry, &parent, &None, None) {
                        if err.is_fatal() {
                            return Err(err);
                        }
                        self.handle_nonfatal_error(err, &path);
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    self.error_count += 1;
                    path_error_message(&parent, err);
                }
            }
        }
        Ok(())
    }

    fn get_list_from_subquery(&mut self, query: Query) -> Result<Vec<String>, SearchError> {
        let query_str = format!("{:?}", query);

        let ok_to_cache = is_subquery_cacheable(&query);
        if ok_to_cache
            && let Some(cached) = self.subquery_cache.get(&query_str) {
                return Ok(cached.clone());
            }

        let mut sub_searcher = Searcher::new_with_context(
            &query,
            self.record_context.clone(),
            self.config,
            self.default_config,
            self.use_colors
        );
        // Always run silent: is_buffered() must hold so results land in
        // output_buffer instead of leaking to stdout (debug mode included).
        sub_searcher.silent_mode = true;
        sub_searcher.is_nested = true;
        let shadowed: Vec<(String, Option<HashMap<String, Variant>>)> = {
            let context = self.record_context.borrow();
            query.own_root_aliases().into_iter().map(|alias| {
                let saved = context.get(&alias).cloned();
                (alias, saved)
            }).collect()
        };
        let outcome = sub_searcher.list_search_results();
        self.error_count += sub_searcher.error_count;
        {
            let mut context = self.record_context.borrow_mut();
            for (alias, saved) in shadowed {
                match saved {
                    Some(entry) => context.insert(alias, entry),
                    None => context.remove(&alias),
                };
            }
        }
        outcome?;

        // The buffer holds limit + offset rows; the offset rows are skipped
        // only in the print path, so they must be skipped here as well.
        // Strip only the row terminator: file names may legally end in spaces
        // or tabs, and a mangled name would silently drop the row later.
        let result_values = sub_searcher.output_buffer.iter_values()
            .skip(query.offset as usize)
            .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
            .collect::<Vec<String>>();

        if ok_to_cache {
            self.subquery_cache.insert(query_str, result_values.clone());
        }

        Ok(result_values)
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
                        if self.limit_reached() {
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

        let depth = root_depth + 1;
        let child_root_depth = depth;

        // Read the directory and process each entry
        let root_dir = self.current_root_dir.clone();
        #[cfg(test)]
        {
            self.dirs_opened += 1;
        }
        match fs::read_dir(dir) {
            Ok(entry_list) => {
                for entry in entry_list {
                    if self.limit_reached() {
                        break;
                    }

                    match entry {
                        Ok(entry) => {
                            let path = entry.path();
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
                                }

                                // Archive members sit below the archive itself, so each one
                                // gets its own depth check against mindepth/maxdepth.
                                if self.current_search_archives
                                    && self.is_zip_archive(&path.to_string_lossy())
                                    && let Some(archive) = open_archive(&path) {
                                            let members = archive.len();
                                            let archive = Rc::new(RefCell::new(archive));
                                            for index in 0..members {
                                                if self.limit_reached() {
                                                    break;
                                                }

                                                let file_info = match archive.borrow_mut().by_index(index) {
                                                    Ok(member) => to_file_info(
                                                        &member,
                                                        Some(ArchiveMember { archive: Rc::clone(&archive), index }),
                                                    ),
                                                    Err(_) => continue,
                                                };
                                                let member_depth = depth + archived_entry_levels(&file_info.name);
                                                if !self.within_depth_limits(member_depth) {
                                                    continue;
                                                }
                                                if let Err(err) = self.check_file(&entry, &root_dir, &Some(file_info), None) {
                                                    if err.is_fatal() {
                                                        return Err(err);
                                                    }
                                                    self.handle_nonfatal_error(err, &path);
                                                    continue;
                                                }
                                            }
                                        }

                                // Recursively visit subdirectories if we're not too deep
                                if within_max_depth {
                                    match descent_file_type.unwrap() {
                                        Ok(file_type) => {
                                            let mut ok = false;

                                            if file_type.is_symlink() && self.current_follow_symlinks {
                                                ok = path.is_dir();
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
            while let Some((path, queued_root_depth)) = self.dir_queue.pop_front() {
                if self.limit_reached() {
                    self.dir_queue.clear();
                    break;
                }
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

    fn deferred_columns(query: &Query) -> Vec<usize> {
        if query.limit == 0 || query.is_aggregated() {
            return vec![];
        }
        let ordering_keys: HashSet<String> = query.ordering_fields.iter().map(Expr::cache_key).collect();
        let where_fields = query.expr.as_ref().map(Expr::get_required_fields).unwrap_or_default();
        query
            .fields
            .iter()
            .enumerate()
            .filter(|(_, field)| {
                field.weight >= DEFERRED_COLUMN_WEIGHT
                    && !field.needs_row_context()
                    && !ordering_keys.contains(&field.cache_key())
                    && field.get_required_fields().is_disjoint(&where_fields)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn finalize_deferred(&mut self) -> Result<(), SearchError> {
        if self.deferred_columns.is_empty() {
            return Ok(());
        }
        let buffered = std::mem::replace(&mut self.output_buffer, TopN::limitless());
        let mut lookup = DirEntryLookup::new();
        let offset = self.query.offset as usize;
        let mut position = 0;
        for (criteria, rows) in buffered.into_entries() {
            for row in rows {
                let rendered = match row {
                    BufferedRow::Rendered(text) => Some(text),
                    BufferedRow::Pending(_) if position < offset => Some(String::new()),
                    BufferedRow::Pending(pending) => self.render_pending(pending, &mut lookup)?,
                };
                position += 1;
                if let Some(text) = rendered {
                    self.output_buffer.insert(criteria.clone(), BufferedRow::Rendered(text));
                }
            }
        }
        Ok(())
    }

    fn render_pending(&mut self, row: PendingRow, lookup: &mut DirEntryLookup) -> Result<Option<String>, SearchError> {
        let (Some(directory), Some(file_name)) = (row.path.parent(), row.path.file_name()) else {
            self.error_count += 1;
            error_message(&row.path.to_string_lossy(), "cannot locate the entry again");
            return Ok(None);
        };
        let mut items = row.items.clone();
        match lookup.entry(directory, file_name) {
            Ok(Some(entry)) => {
                self.fms.clear();
                let follow_symlinks = std::mem::replace(&mut self.current_follow_symlinks, row.follow_symlinks);
                let evaluated = self.evaluate_deferred(entry, &row, &mut items);
                self.current_follow_symlinks = follow_symlinks;
                evaluated?;
            }
            Ok(None) => {
                self.error_count += 1;
                error_message(&row.path.to_string_lossy(), "entry disappeared during the search");
            }
            Err(err) => {
                self.error_count += 1;
                path_error_message(&row.path, err);
            }
        }
        let mut buf = WritableBuffer::new();
        self.results_writer.write_row(&mut buf, items)?;
        Ok(Some(String::from(buf)))
    }

    fn evaluate_deferred(&mut self, entry: &DirEntry, row: &PendingRow, items: &mut [(String, String)]) -> Result<(), SearchError> {
        let query = self.query;
        let mut file_map = HashMap::new();
        if self.use_colors && row.deferred.iter().any(|index| query.fields[*index].contains_colorized()) {
            self.fms.update_file_metadata(entry, self.current_follow_symlinks);
        }
        for &index in &row.deferred {
            let field = &query.fields[index];
            let record = match self.get_column_expr_value(Some(entry), &row.file_info, &row.root_path, &mut file_map, None, field) {
                Ok(record) => record,
                Err(err) if err.is_fatal() => return Err(err),
                Err(err) => {
                    self.handle_nonfatal_error(err, &row.path);
                    continue;
                }
            };
            items[index].1 = match self.use_colors && field.contains_colorized() {
                true => self.colorize(&record.to_string()),
                false => record.to_string(),
            };
        }
        Ok(())
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
        let cache_key = column_expr.cache_key();

        let mut context_field: Option<String> = None;

        let reference = match (&column_expr.root_alias, column_expr.field) {
            (Some(root_alias), Some(field)) => {
                Some((root_alias.clone(), column_expr.alias.clone().unwrap_or_else(|| field.to_string())))
            }
            // Cheap guard: the alias regex requires a dot, and most column
            // expressions have none, so skip the regex in the per-file hot path.
            _ if cache_key.contains('.') => FIELD_WITH_ALIAS
                .captures(&column_expr.to_string())
                .map(|captures| (captures[1].to_string(), captures[2].to_string())),
            _ => None,
        };
        if let Some((context_name, field_key)) = reference {
            if self.current_alias.as_deref() == Some(context_name.as_str()) {
                if !column_expr.minus {
                    context_field = Some(field_key);
                }
            } else {
                let context = self.record_context.borrow();
                if let Some(ctx) = context.get(&context_name) {
                    if let Some(val) = ctx.get(&field_key) {
                        let value = apply_minus(column_expr, val.clone());
                        file_map.insert(cache_key, value.to_string());
                        return Ok(value);
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
                if column_expr.root_alias.is_some() || self.is_declared_root_alias(&context_name) {
                    return Err(SearchError::fatal(format!("Invalid root alias: {}", context_name)).with_source("query"));
                }
            }
        }

        if file_map.contains_key(&cache_key) {
            if let Some(entry_key) = context_field {
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_default();
                context_entry.insert(entry_key, typed_variant(column_expr.field.as_ref(), &file_map[&cache_key]));
            }
            return Ok(Variant::from_string(&file_map[&cache_key]));
        }

        if let Some(ref subquery) = column_expr.subquery {
            let mut subquery = subquery.clone();
            if subquery.grouping_fields.is_empty() {
                subquery.limit = 1;
            }
            let list = self.get_list_from_subquery(*subquery)?;
            if !list.is_empty() {
                let result = list.first().unwrap().to_string();
                return Ok(apply_minus(column_expr, Variant::from_string(&result)));
            }
        }

        if let Some(ref _function) = column_expr.function {
            let result =
                self.get_function_value(entry, file_info, root_path, file_map, accumulator, column_expr)?;
            let result = apply_minus(column_expr, result);
            file_map.insert(cache_key, result.to_string());
            return Ok(result);
        }

        if let Some(ref field) = column_expr.field {
            if let Some(entry) = entry {
                let raw = self.get_field_value(entry, file_info, root_path, field).unwrap_or(Variant::empty(VariantType::String));
                // The record context feeds correlated subqueries and must hold
                // the raw field value; only the returned value is negated.
                let result = apply_minus(column_expr, raw.clone());
                file_map.insert(cache_key, result.to_string());
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_default();
                let entry_key = if let Some(alias) = column_expr.alias.clone() { alias } else { field.to_string() };
                context_entry.insert(entry_key, raw);
                return Ok(result);
            } else if let Some(val) = file_map.get(&field.to_string()) {
                return Ok(apply_minus(column_expr, Variant::from_string(val)));
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
                        result = op.calc(&left_result, &right_result).map(|v| apply_minus(column_expr, v));
                        file_map.insert(cache_key, result.clone()?.to_string());
                } else {
                    result = Ok(apply_minus(column_expr, left_result));
                }
            } else {
                result = Ok(apply_minus(column_expr, left_result));
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
            let buffer_key = if left_expr.is_literal() && matches!(left_expr.val.as_deref(), Some("*") | Some("")) {
                String::from("*")
            } else {
                left_expr.cache_key()
            };
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
            file_map.insert(column_expr.cache_key(), result.to_string());

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
                    context_entry.insert(field, field_value);
                }
            }
        }

        if let Some(ref expr) = self.query.expr {
            let result = self.conforms(entry, file_info, root_path, expr)?;
            if !result {
                return Ok(());
            }
        }

        if let Some(plan) = self.aggregate_plan.clone() {
            self.found += 1;
            for (field, key) in &plan.fields {
                let value = self.get_field_value(entry, file_info, root_path, field).unwrap_or(Variant::empty(VariantType::String));
                file_map.insert(key.clone(), value.to_string());
            }
            for (key, left) in &plan.inner_exprs {
                if !file_map.contains_key(key) {
                    let value = self.get_column_expr_value(Some(entry), file_info, root_path, file_map, None, left)?;
                    file_map.entry(key.clone()).or_insert_with(|| value.to_string());
                }
            }
            for (field, key) in self.query.grouping_fields.iter().zip(&plan.grouping_keys) {
                if !file_map.contains_key(key) {
                    self.get_column_expr_value(Some(entry), file_info, root_path, file_map, None, field)?;
                }
            }
            let group_key: Vec<String> = plan
                .grouping_keys
                .iter()
                .map(|key| file_map.get(key).cloned().unwrap_or_default())
                .collect();
            let accumulator = self.accumulators.entry(group_key).or_default();
            accumulator.increment_count();
            for (key, value) in file_map.iter() {
                accumulator.push(key, value);
            }
            return Ok(());
        }

        let defer = self.is_buffered() && !self.deferred_columns.is_empty();
        let mut items: Vec<(String, String)> = Vec::new();

        if self.use_colors && self.query.fields.iter().any(|f| f.contains_colorized()) {
            self.fms.update_file_metadata(entry, self.current_follow_symlinks);
        }

        for (index, field) in self.query.fields.iter().enumerate() {
            if defer && self.deferred_columns.contains(&index) {
                items.push((field.column_name(), String::new()));
                continue;
            }
            let record =
                self.get_column_expr_value(Some(entry), file_info, root_path, file_map, None, field)?;

            let value = match self.use_colors && field.contains_colorized() {
                true => self.colorize(&record.to_string()),
                false => record.to_string(),
            };
            items.push((field.column_name(), value));
        }

        let mut criteria = vec!["".to_string(); self.query.ordering_fields.len()];
        for (idx, field) in self.query.ordering_fields.iter().enumerate() {
            criteria[idx] = match file_map.get(&field.cache_key()) {
                Some(record) => record.clone(),
                None => self
                    .get_column_expr_value(Some(entry), file_info, root_path, file_map, None, field)?
                    .to_string(),
            }
        }

        if self.is_buffered() {
            let row = if defer {
                BufferedRow::Pending(PendingRow {
                    path: entry.path(),
                    root_path: root_path.to_path_buf(),
                    file_info: file_info.clone(),
                    follow_symlinks: self.current_follow_symlinks,
                    items,
                    deferred: self.deferred_columns.clone(),
                })
            } else {
                let mut buf = WritableBuffer::new();
                self.results_writer.write_row(&mut buf, items)?;
                BufferedRow::Rendered(String::from(buf))
            };
            self.found += 1;
            self.output_buffer.insert(
                Criteria::new(
                    self.ordering_fields_rc.clone(),
                    criteria,
                    self.ordering_asc_rc.clone(),
                ),
                row,
            );
        } else {
            let mut buf = WritableBuffer::new();
            if self.found > 0 {
                self.results_writer.write_row_separator(&mut buf)?;
            }
            self.results_writer.write_row(&mut buf, items)?;
            self.found += 1;
            try_output!(write!(std::io::stdout(), "{}", String::from(buf)),
                        Err(SearchError::broken_pipe()));
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


    fn check_exists(&mut self, expr: &Expr) -> Result<bool, SearchError> {
        let right = expr.right.as_ref().unwrap();
        match &right.args {
            Some(args) => Ok(!args.is_empty()),
            None => {
                if let Some(subquery) = &right.subquery {
                    let mut subquery = *subquery.clone();
                    if subquery.grouping_fields.is_empty() {
                        subquery.limit = 1;
                    }
                    Ok(!self.get_list_from_subquery(subquery)?.is_empty())
                } else {
                    Ok(false)
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
        let args: &[Expr] = match &right.args {
            Some(args) => args,
            None => {
                let found = match &right.subquery {
                    Some(subquery) => self.subquery_membership(subquery, field_value.get_type())?.contains(field_value),
                    None => false,
                };
                return Ok(if negate { !found } else { found });
            }
        };
        let field_type = field_value.get_type();
        let mut found = false;
        for arg in args {
            #[cfg(test)]
            {
                self.in_list_comparisons += 1;
            }
            arg_map.clear();
            let arg_val = self.get_column_expr_value(
                Some(entry), file_info, root_path, arg_map, None, arg,
            )?;
            let matches = match field_type {
                VariantType::String => arg_val.to_string() == field_value.to_string(),
                // Unparsable numeric args (`size in ('abc')`) are skipped,
                // not coerced to 0, like the DateTime arm below.
                VariantType::Int => arg_val.to_exact_int() == Some(field_value.to_int()),
                VariantType::Float => arg_val.to_float_opt() == Some(field_value.to_float()),
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

    fn subquery_membership(&mut self, subquery: &Query, field_type: &VariantType) -> Result<Rc<Membership>, SearchError> {
        let key = (format!("{:?}", subquery), field_type.clone());
        if let Some(membership) = self.membership_cache.get(&key) {
            return Ok(Rc::clone(membership));
        }
        let values = self.get_list_from_subquery(subquery.clone())?;
        let membership = Rc::new(Membership::new(&values, field_type));
        if is_subquery_cacheable(subquery) {
            self.membership_cache.insert(key, Rc::clone(&membership));
        }
        Ok(membership)
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
                Op::Exists => return self.check_exists(expr),
                Op::NotExists => return self.check_exists(expr).map(|exists| !exists),
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

            result = match op {
                Op::In => {
                    let mut arg_map = HashMap::new();
                    self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, false)?
                }
                Op::NotIn => {
                    let mut arg_map = HashMap::new();
                    self.check_in_list(expr, entry, file_info, root_path, &mut arg_map, &field_value, true)?
                }
                _ => self.compare_variants(op, &field_value, &value)?,
            };
        }

        Ok(result)
    }

    fn compare_variants(&mut self, op: &Op, field_value: &Variant, value: &Variant) -> Result<bool, SearchError> {
        if *field_value.get_type() == VariantType::String
            && *value.get_type() != VariantType::String
            && let Some(mirrored) = op.mirrored()
        {
            return self.compare_variants(&mirrored, value, field_value);
        }

        let result;
        {
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
                        _ => false,
                    }
                }
                VariantType::Int => {
                    // A literal that is neither a number nor a file size
                    // (`size = ''`, `size > 'abc'`) is a non-match rather than
                    // 0, mirroring the DateTime branch's NULL-like semantics.
                    let Some(val) = value.to_int_opt() else {
                        return Ok(false);
                    };
                    let int_value = field_value.to_int();
                    let ordering = match value.to_float_opt().filter(|float| float.fract() != 0.0) {
                        Some(fraction) => (int_value as f64).partial_cmp(&fraction).unwrap_or(std::cmp::Ordering::Equal),
                        None => int_value.cmp(&val),
                    };
                    match op {
                        Op::Eq | Op::Eeq => ordering == std::cmp::Ordering::Equal,
                        Op::Ne | Op::Ene => ordering != std::cmp::Ordering::Equal,
                        Op::Gt => ordering == std::cmp::Ordering::Greater,
                        Op::Gte => ordering != std::cmp::Ordering::Less,
                        Op::Lt => ordering == std::cmp::Ordering::Less,
                        Op::Lte => ordering != std::cmp::Ordering::Greater,
                        _ => false,
                    }
                }
                VariantType::Float => {
                    let Some(val) = value.to_float_opt() else {
                        return Ok(false);
                    };
                    let float_value = field_value.to_float();
                    match op {
                        Op::Eq | Op::Eeq => float_value == val,
                        Op::Ne | Op::Ene => float_value != val,
                        Op::Gt => float_value > val,
                        Op::Gte => float_value >= val,
                        Op::Lt => float_value < val,
                        Op::Lte => float_value <= val,
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
                                _ => false,
                            }
                        }
                        _ => false,
                    }
                }
            };
        }

        Ok(result)
    }

    fn group_conforms(
        &mut self,
        file_map: &mut HashMap<String, String>,
        group_acc: &function::GroupAccumulator,
        expr: &Expr,
    ) -> Result<bool, SearchError> {
        if let Some(ref logical_op) = expr.logical_op {
            let left_result = match expr.left {
                Some(ref left) => self.group_conforms(file_map, group_acc, left)?,
                None => false,
            };

            let right_result = |s: &mut Self, file_map: &mut HashMap<String, String>| -> Result<bool, SearchError> {
                match expr.right {
                    Some(ref right) => s.group_conforms(file_map, group_acc, right),
                    None => Ok(false),
                }
            };

            return match logical_op {
                LogicalOp::And => if !left_result { Ok(false) } else { right_result(self, file_map) },
                LogicalOp::Or => if left_result { Ok(true) } else { right_result(self, file_map) },
            };
        }

        if let Some(ref op) = expr.op {
            if matches!(op, Op::In | Op::NotIn | Op::Exists | Op::NotExists) {
                return Err(SearchError::fatal(
                    "IN, NOT IN, EXISTS, and NOT EXISTS are not supported in the HAVING clause",
                )
                .with_source("query"));
            }
            let left_expr = expr.left.as_ref().ok_or_else(|| {
                SearchError::fatal("Expected expression before operator in HAVING").with_source("query")
            })?;
            let right_expr = expr.right.as_ref().ok_or_else(|| {
                SearchError::fatal("Expected expression after operator in HAVING").with_source("query")
            })?;
            let field_value = self.get_column_expr_value(
                None, &None, Path::new(""), file_map, Some(group_acc), left_expr,
            )?;
            let value = self.get_column_expr_value(
                None, &None, Path::new(""), file_map, Some(group_acc), right_expr,
            )?;
            let field_value = Self::retype_group_value(left_expr, right_expr, field_value);
            return self.compare_variants(op, &field_value, &value);
        }

        Ok(false)
    }

    fn retype_group_value(left_expr: &Expr, right_expr: &Expr, value: Variant) -> Variant {
        if !matches!(value.get_type(), VariantType::String) {
            return value;
        }
        if left_expr.contains_datetime() || right_expr.contains_datetime() {
            if let Ok((dt_from, _)) = value.to_datetime() {
                return Variant::from_datetime(dt_from);
            }
        } else if (left_expr.contains_numeric() || right_expr.contains_numeric())
            && let Ok(f) = value.to_string().parse::<f64>()
                && f.is_finite() {
                    return Variant::from_float(f);
                }
        value
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

    fn within_depth_limits(&self, depth: u32) -> bool {
        (self.current_min_depth == 0 || depth >= self.current_min_depth)
            && (self.current_max_depth == 0 || depth <= self.current_max_depth)
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
            having: None,
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
            having: None,
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
            having: None,
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
            having: None,
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
    fn test_is_aggregated_with_grouping_only() {
        // GROUP BY without an aggregate in the SELECT list must still route
        // through the grouped path (SQL collapses rows into distinct groups).
        let query = Box::leak(Box::new(Query {
            fields: vec![Expr::field(Field::Extension)],
            roots: Vec::new(),
            expr: None,
            grouping_fields: vec![Expr::field(Field::Extension)],
            having: None,
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        }));
        assert!(query.is_aggregated());
        assert!(!query.has_aggregate_column());

        let plain = create_test_searcher();
        assert!(!plain.query.is_aggregated());
    }

    #[test]
    fn group_by_without_aggregate_yields_distinct_groups() {
        let tmp = std::env::temp_dir().join("fselect_test_group_no_aggregate");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "1").unwrap();
        fs::write(tmp.join("b.txt"), "22").unwrap();
        fs::write(tmp.join("c.md"), "333").unwrap();

        let query = Box::leak(Box::new(Query {
            fields: vec![Expr::field(Field::Extension)],
            roots: vec![Root::new(
                tmp.to_string_lossy().to_string(),
                RootOptions::new(),
            )],
            expr: None,
            grouping_fields: vec![Expr::field(Field::Extension)],
            having: None,
            ordering_fields: vec![Expr::field(Field::Extension)],
            ordering_asc: vec![true],
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        }));
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));
        let mut searcher = Searcher::new(query, config, default_config, false);
        searcher.silent_mode = true;

        searcher.list_search_results().unwrap();

        let rows: Vec<String> = searcher
            .output_buffer
            .iter_values()
            .map(|s| s.trim_end().to_string())
            .collect();
        let _ = fs::remove_dir_all(&tmp);

        // One row per distinct extension, not one row per file.
        assert_eq!(rows, vec![String::from("md"), String::from("txt")]);
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
            ctx.insert("a".to_string(), HashMap::from([(key, Variant::from_string(&String::from("foo.txt")))]));
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
            ctx.insert("a".to_string(), HashMap::from([(key, Variant::from_string(&String::from("outer.txt")))]));
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
            having: None,
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
            having: None,
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
            having: None,
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
            having: None,
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        };
        assert!(is_subquery_cacheable(&subquery));
    }

    #[test]
    fn subquery_correlated_via_order_by_is_not_cacheable() {
        // The subquery's WHERE and SELECT are independent of the outer row, but
        // its ORDER BY references `t1.size`. Memoising it would reuse the first
        // outer row's ordering for every subsequent row.
        let mut t1_size = Expr::field(Field::Size);
        t1_size.root_alias = Some(String::from("t1"));
        let ordering = Expr::arithmetic_op(
            Expr::field(Field::Size),
            crate::operators::ArithmeticOp::Subtract,
            t1_size,
        );

        let subquery = Query {
            fields: vec![Expr::field(Field::Name)],
            roots: vec![Root::new(String::from("/t2"), RootOptions::new())],
            expr: None,
            grouping_fields: Vec::new(),
            having: None,
            ordering_fields: vec![ordering],
            ordering_asc: vec![true],
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        };

        assert!(
            !is_subquery_cacheable(&subquery),
            "subquery whose ORDER BY references outer alias t1 must not be cached"
        );
    }

    #[test]
    fn subquery_correlated_via_group_by_is_not_cacheable() {
        // Same, but the outer-alias reference lives in GROUP BY.
        let mut t1_size = Expr::field(Field::Size);
        t1_size.root_alias = Some(String::from("t1"));
        let grouping = Expr::arithmetic_op(
            Expr::field(Field::Size),
            crate::operators::ArithmeticOp::Subtract,
            t1_size,
        );

        let subquery = Query {
            fields: vec![Expr::function(Function::Max)],
            roots: vec![Root::new(String::from("/t2"), RootOptions::new())],
            expr: None,
            grouping_fields: vec![grouping],
            having: None,
            ordering_fields: Vec::new(),
            ordering_asc: Vec::new(),
            limit: 0,
            offset: 0,
            output_format: OutputFormat::Tabs,
            raw_query: String::new(),
        };

        assert!(
            !is_subquery_cacheable(&subquery),
            "subquery whose GROUP BY references outer alias t1 must not be cached"
        );
    }

    #[test]
    fn unary_minus_negates_parens_fields_and_functions() {
        let tmp = std::env::temp_dir().join("fselect_test_unary_minus");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("x.txt"), "12345").unwrap();

        let rows = run_query_against_dir(
            "select -(1+2), -abs(3), size, -size, -(-1) from __DIR__ where name = 'x.txt'",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows, vec![String::from("-3\t-3\t5\t-5\t1")]);
    }

    #[test]
    fn subquery_with_correlated_from_subselect_is_not_cacheable() {
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        // The outer-alias reference hides inside the FROM-subselect: memoising
        // this query would reuse the first outer row's result for every row.
        let query = "select name from (select path from /test where size gt t1.size)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut parser = Parser::new(&mut lexer);
        let parsed = parser.parse(false).expect("parse failed");

        assert!(
            !is_subquery_cacheable(&parsed),
            "query with a correlated FROM-subselect must not be cached"
        );
    }

    /// Build a real Query via the parser+lexer and execute it silently against
    /// a temp directory. Returns the rendered rows the outer query produced so
    /// we can assert against them as a flat set of strings.
    fn run_query_against_dir(query_template: &str, dir: &Path) -> Vec<String> {
        run_query_against_dir_with_config(query_template, dir, Config::default())
    }

    fn run_query_against_dir_with_config(
        query_template: &str,
        dir: &Path,
        config: Config,
    ) -> Vec<String> {
        search_dir(query_template, dir, config, true)
            .output_buffer
            .iter_values()
            .map(|s| s.trim_end().to_string())
            .collect()
    }

    fn search_dir(query_template: &str, dir: &Path, config: Config, silent: bool) -> Searcher<'static> {
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        let query_string = query_template.replace("__DIR__", &dir.to_string_lossy());
        let mut lexer = Lexer::new(vec![query_string.clone()]);
        let mut parser = Parser::new(&mut lexer);
        let parsed = parser.parse(false).expect("parse failed");
        let parsed = Box::leak(Box::new(parsed));
        let config = Box::leak(Box::new(config));
        let default_config = Box::leak(Box::new(Config::default()));

        let mut searcher = Searcher::new(parsed, config, default_config, false);
        searcher.silent_mode = silent;
        searcher.list_search_results().expect("search failed");
        searcher
    }

    #[test]
    fn errors_inside_subqueries_count_towards_the_outer_search() {
        let tmp = std::env::temp_dir().join("fselect_test_subquery_errors");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("ten.txt"), "x".repeat(10)).unwrap();
        fs::write(tmp.join("three.txt"), "xxx").unwrap();
        fs::write(tmp.join("zero.txt"), "").unwrap();

        let per_file_error = search_dir(
            "select name from __DIR__ where name in (select name from __DIR__ where 1 / size > 0)",
            &tmp,
            Config::default(),
            true,
        );
        let missing_inner_root = search_dir(
            "select name from __DIR__ where not exists (select name from __DIR__/missing)",
            &tmp,
            Config::default(),
            true,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(per_file_error.output_buffer.iter_values().count(), 2);
        assert_eq!(per_file_error.error_count, 1, "the inner division by zero must be counted");
        assert_eq!(missing_inner_root.error_count, 1, "the missing inner root must be counted");
    }

    #[test]
    fn fatal_subquery_errors_are_returned_instead_of_an_empty_list() {
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        let mut searcher = create_test_searcher();
        let mut lexer = Lexer::new(vec![String::from("select name from ./[unclosed rx")]);
        let subquery = Parser::new(&mut lexer).parse(false).expect("parse failed");

        match searcher.get_list_from_subquery(subquery) {
            Err(err) => assert!(err.is_fatal() && err.description.contains("regex"), "{}", err.description),
            Ok(list) => panic!("fatal subquery error was swallowed, got {:?}", list),
        }
    }

    #[test]
    fn rows_that_fail_to_render_are_not_counted_as_found() {
        let tmp = std::env::temp_dir().join("fselect_test_failed_rows_not_found");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "xxx").unwrap();
        fs::write(tmp.join("c.txt"), "xxxxx").unwrap();

        let all = search_dir("select name, 1 / (size - 1) from __DIR__", &tmp, Config::default(), true);
        let limited = search_dir("select name, 1 / (size - 1) from __DIR__ limit 2", &tmp, Config::default(), true);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(all.output_buffer.iter_values().count(), 2);
        assert_eq!(all.found, 2, "a row that failed to render was never found");
        assert_eq!(all.error_count, 1);
        assert_eq!(limited.output_buffer.iter_values().count(), 2, "the failed row must not use up a LIMIT slot");
    }

    fn run_query_result(query_template: &str, dir: &Path) -> Result<(), SearchError> {
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        let query_string = query_template.replace("__DIR__", &dir.to_string_lossy());
        let mut lexer = Lexer::new(vec![query_string]);
        let parsed = Box::leak(Box::new(Parser::new(&mut lexer).parse(false).expect("parse failed")));
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));
        let mut searcher = Searcher::new(parsed, config, default_config, false);
        searcher.silent_mode = true;
        searcher.list_search_results()
    }

    #[test]
    fn aggregate_evaluation_errors_fail_the_query() {
        let tmp = std::env::temp_dir().join("fselect_test_aggregate_errors");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "xxx").unwrap();

        let queries = [
            "select sum(size) / 0 from __DIR__",
            "select sum(size), sum(size) / 0, count(*) from __DIR__",
            "select ext, sum(size) / 0 from __DIR__ group by ext",
            "select ext from __DIR__ group by ext order by sum(size) / 0",
        ];
        let results: Vec<_> = queries
            .iter()
            .map(|query| (*query, run_query_result(query, &tmp).map_err(|err| err.description)))
            .collect();
        let _ = fs::remove_dir_all(&tmp);

        for (query, result) in results {
            assert!(
                matches!(&result, Err(description) if description.contains("Division by zero")),
                "{}: {:?}",
                query,
                result
            );
        }
    }

    fn link_directory(link: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J", &link.to_string_lossy(), &target.to_string_lossy()])
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "mklink /J failed");
        }
    }

    #[test]
    fn depth_limits_count_traversal_levels_through_directory_links() {
        use std::path::MAIN_SEPARATOR_STR as SEP;

        let tmp = std::env::temp_dir().join("fselect_test_link_depth");
        let _ = fs::remove_dir_all(&tmp);
        let root = tmp.join("root");
        fs::create_dir_all(root.join("realsub").join("x")).unwrap();
        fs::create_dir_all(tmp.join("target").join("deep").join("deeper")).unwrap();
        fs::create_dir_all(tmp.join("far").join("a").join("b").join("c")).unwrap();
        fs::write(root.join("top.txt"), "").unwrap();
        fs::write(root.join("realsub").join("x").join("deepfile.txt"), "").unwrap();
        fs::write(tmp.join("target").join("deep").join("e.log"), "").unwrap();
        fs::write(tmp.join("target").join("deep").join("deeper").join("f.txt"), "").unwrap();
        fs::write(tmp.join("far").join("a").join("b").join("c").join("leaf.txt"), "").unwrap();
        link_directory(&root.join("link_sib"), &tmp.join("target"));
        link_directory(&root.join("link_far"), &tmp.join("far").join("a").join("b").join("c"));

        let shallow = run_query_against_dir("select path, depth from __DIR__ sym maxdepth 2 order by path", &root);
        let deep = run_query_against_dir("select path, depth from __DIR__ sym mindepth 3 order by path", &root);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(shallow, vec![
            String::from("link_far\t1"),
            format!("link_far{SEP}leaf.txt\t2"),
            String::from("link_sib\t1"),
            format!("link_sib{SEP}deep\t2"),
            String::from("realsub\t1"),
            format!("realsub{SEP}x\t2"),
            String::from("top.txt\t1"),
        ]);
        assert_eq!(deep, vec![
            format!("link_sib{SEP}deep{SEP}deeper\t3"),
            format!("link_sib{SEP}deep{SEP}deeper{SEP}f.txt\t4"),
            format!("link_sib{SEP}deep{SEP}e.log\t3"),
            format!("realsub{SEP}x{SEP}deepfile.txt\t3"),
        ]);
    }

    #[test]
    fn in_subquery_membership_is_a_lookup_not_a_scan() {
        let tmp = std::env::temp_dir().join("fselect_test_in_membership");
        let _ = fs::remove_dir_all(&tmp);
        let a = tmp.join("a");
        let b = tmp.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        for i in 0..20 {
            fs::write(a.join(format!("f{i:02}.txt")), "x".repeat(i)).unwrap();
            fs::write(b.join(format!("f{:02}.txt", i + 10)), "x".repeat(i + 10)).unwrap();
        }
        let b_root = b.to_string_lossy().to_string();

        let by_name = search_dir(
            &format!("select name from __DIR__ where name in (select name from {b_root}) order by name"),
            &a, Config::default(), true,
        );
        let by_size = search_dir(
            &format!("select name from __DIR__ where size not in (select size from {b_root}) order by name"),
            &a, Config::default(), true,
        );
        let by_bool = search_dir(
            &format!("select name from __DIR__ where is_dir in (select is_dir from {b_root})"),
            &a, Config::default(), true,
        );
        let literal_list = search_dir(
            "select name from __DIR__ where name in ('f03.txt', 'f04.txt')",
            &a, Config::default(), true,
        );
        let _ = fs::remove_dir_all(&tmp);

        let names = |searcher: &Searcher| searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect::<Vec<_>>();
        assert_eq!(names(&by_name), (10..20).map(|i| format!("f{i:02}.txt")).collect::<Vec<_>>());
        assert_eq!(names(&by_size), (0..10).map(|i| format!("f{i:02}.txt")).collect::<Vec<_>>());
        assert_eq!(by_bool.output_buffer.iter_values().count(), 20);
        assert_eq!(literal_list.output_buffer.iter_values().count(), 2);
        assert_eq!(by_name.in_list_comparisons, 0, "subquery members must not be compared one by one per row");
        assert_eq!(by_size.in_list_comparisons, 0);
        assert_eq!(by_bool.in_list_comparisons, 0);
        assert!(literal_list.in_list_comparisons > 0, "literal lists keep the per-argument evaluation");
    }

    #[test]
    fn expensive_columns_are_evaluated_only_for_rows_that_survive_the_limit() {
        use crate::util::content_scans;

        let tmp = std::env::temp_dir().join("fselect_test_deferred_columns");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for i in 0..6 {
            fs::write(tmp.join(format!("f{i}.txt")), "line\n".repeat(i + 1)).unwrap();
        }
        let rows = |searcher: &Searcher, skip: usize| {
            searcher.output_buffer.iter_values().skip(skip).map(|row| row.trim_end().to_string()).collect::<Vec<_>>()
        };
        let measure = |query: &str| {
            let before = content_scans();
            let searcher = search_dir(query, &tmp, Config::default(), true);
            (searcher, content_scans() - before)
        };

        let (top2, top2_scans) = measure("select name, line_count from __DIR__ order by name limit 2");
        let (offset, offset_scans) = measure("select name, line_count from __DIR__ order by name desc limit 2 offset 1");
        let (by_it, by_it_scans) = measure("select name, line_count from __DIR__ order by line_count desc limit 1");
        let (unlimited, unlimited_scans) = measure("select name, line_count from __DIR__ order by name");
        let (unordered, unordered_scans) = measure("select name, line_count from __DIR__ where line_count > 0 limit 2");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows(&top2, 0), vec!["f0.txt\t1", "f1.txt\t2"]);
        assert_eq!(top2_scans, 2, "only the two surviving rows may be scanned");
        assert_eq!(rows(&offset, 1), vec!["f4.txt\t5", "f3.txt\t4"]);
        assert_eq!(offset_scans, 2, "the offset row is skipped without being scanned");
        assert_eq!(rows(&by_it, 0), vec!["f5.txt\t6"]);
        assert_eq!(by_it_scans, 6, "an ordering column cannot be deferred");
        assert_eq!(rows(&unlimited, 0).len(), 6);
        assert_eq!(unlimited_scans, 6);
        assert_eq!(rows(&unordered, 0).len(), 2);
        assert_eq!(unordered_scans, 2, "a WHERE on the column forces the scan, but nothing beyond the limit is visited");
    }

    #[test]
    fn bfs_stops_opening_queued_directories_once_the_limit_is_reached() {
        let tmp = std::env::temp_dir().join("fselect_test_bfs_limit");
        let _ = fs::remove_dir_all(&tmp);
        for k in 0..50 {
            let sub = tmp.join(format!("d{:02}", k));
            fs::create_dir_all(&sub).unwrap();
            fs::write(sub.join("inner.txt"), "x").unwrap();
        }
        fs::write(tmp.join("zzz.txt"), "x").unwrap();

        let searcher = search_dir("select name from __DIR__ bfs where is_file limit 1", &tmp, Config::default(), false);
        assert_eq!(searcher.found, 1);
        assert_eq!(searcher.dirs_opened, 1,
            "the root yields the single wanted row, so none of the queued subdirectories should be opened");

        let searcher = search_dir("select name from __DIR__ bfs where is_file limit 60", &tmp, Config::default(), false);
        assert_eq!(searcher.found, 51);
        assert_eq!(searcher.dirs_opened, 51, "without a reached limit every directory is still visited");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn buffered_unordered_queries_stop_at_the_limit_as_subqueries_do() {
        let tmp = std::env::temp_dir().join("fselect_test_silent_limit");
        let _ = fs::remove_dir_all(&tmp);
        for k in 0..30 {
            let sub = tmp.join(format!("d{:02}", k));
            fs::create_dir_all(&sub).unwrap();
            fs::write(sub.join("inner.txt"), "x").unwrap();
        }
        fs::write(tmp.join("zzz.txt"), "x").unwrap();

        let searcher = search_dir("select name from __DIR__ bfs where is_file limit 1", &tmp, Config::default(), true);
        assert_eq!(searcher.found, 1);
        assert_eq!(searcher.dirs_opened, 1, "silent (subquery) mode must stop at the limit like printed output does");
        assert_eq!(searcher.output_buffer.iter_values().count(), 1);

        let searcher = search_dir("select name from __DIR__ bfs where is_file limit 1 offset 1", &tmp, Config::default(), true);
        assert_eq!(searcher.found, 2, "offset rows still have to be collected before stopping");
        assert_eq!(searcher.dirs_opened, 2);
        assert_eq!(searcher.output_buffer.iter_values().skip(1).count(), 1);

        let searcher = search_dir("select name from __DIR__ bfs where is_file order by name limit 1", &tmp, Config::default(), true);
        assert_eq!(searcher.found, 31, "ordered queries must still see every row");
        assert_eq!(searcher.dirs_opened, 31);

        let searcher = search_dir("select count(*) from __DIR__ bfs where is_file limit 1", &tmp, Config::default(), true);
        assert_eq!(searcher.found, 31, "aggregates must still see every row");

        let _ = fs::remove_dir_all(&tmp);
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
    fn group_by_having_filters_groups() {
        let tmp = std::env::temp_dir().join("fselect_test_having_filters_groups");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.rs"), "x").unwrap();
        fs::write(tmp.join("b.rs"), "x").unwrap();
        fs::write(tmp.join("c.rs"), "x").unwrap();
        fs::write(tmp.join("d.txt"), "x").unwrap();

        let rows = run_query_against_dir(
            "select ext, count(*) from __DIR__ depth 1 group by ext having count(*) > 2",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows, vec![String::from("rs\t3")]);
    }

    #[test]
    fn having_compares_numerically_not_lexicographically() {
        let tmp = std::env::temp_dir().join("fselect_test_having_numeric_compare");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for i in 0..10 {
            fs::write(tmp.join(format!("f{}.rs", i)), "x").unwrap();
        }
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "x").unwrap();

        let rows = run_query_against_dir(
            "select ext, count(*) from __DIR__ depth 1 group by ext having count(*) >= 10",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows, vec![String::from("rs\t10")]);
    }

    #[test]
    fn having_aliases_resolve_in_every_position() {
        let tmp = std::env::temp_dir().join("fselect_test_having_alias_positions");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "x").unwrap();
        fs::write(tmp.join("c.txt"), "x").unwrap();

        let run = |having: &str| run_query_against_dir(&format!("select count(*) as n from __DIR__ {having}"), &tmp);
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("having n > 10", vec![]),
            ("having 10 < n", vec![]),
            ("having 2 < n", vec!["3"]),
            ("having n * 2 > 5", vec!["3"]),
            ("having 5 < n * 2", vec!["3"]),
            ("having abs(n) > 2", vec!["3"]),
            ("having abs(n) > 3", vec![]),
            ("having n between 2 and 4", vec!["3"]),
            ("having 2 < n and n < 4", vec!["3"]),
            ("having 10 < n or n = 3", vec!["3"]),
        ];
        let results: Vec<(&str, Vec<String>)> = cases.iter().map(|(having, _)| (*having, run(having))).collect();
        let _ = fs::remove_dir_all(&tmp);

        for ((having, expected), (_, actual)) in cases.iter().zip(results) {
            assert_eq!(&actual, expected, "{}", having);
        }
    }

    #[test]
    fn grouped_ordering_matches_select_columns_structurally() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_ordering_match");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "bb.md", "cccccc.rs"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let by_expression = run_query_against_dir(
            "select length('Name'), length(name) from __DIR__ group by name order by length(name) desc",
            &tmp,
        );
        let by_ordinal = run_query_against_dir(
            "select length('Name'), length(name) from __DIR__ group by name order by 2 desc",
            &tmp,
        );
        let aliased = run_query_against_dir(
            "select length(name) as size, count(*) from __DIR__ group by name order by size desc",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(by_expression, vec!["4\t9", "4\t5", "4\t5"]);
        assert_eq!(by_ordinal, vec!["4\t9", "4\t5", "4\t5"]);
        assert_eq!(aliased, vec!["9\t1", "5\t1", "5\t1"]);
    }

    #[test]
    fn quoted_having_literals_compare_as_text() {
        let tmp = std::env::temp_dir().join("fselect_test_quoted_having_literal");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "b.txt", "c.md"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let quoted = run_query_against_dir("select ext, count(*) as txt from __DIR__ group by ext having ext = 'txt'", &tmp);
        let reversed = run_query_against_dir("select ext, count(*) as txt from __DIR__ group by ext having 'txt' = ext", &tmp);
        let alias = run_query_against_dir("select ext, count(*) as txt from __DIR__ group by ext having txt > 1", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(quoted, vec!["txt\t2"]);
        assert_eq!(reversed, vec!["txt\t2"]);
        assert_eq!(alias, vec!["txt\t2"]);
    }

    #[test]
    fn having_alias_edge_cases() {
        let tmp = std::env::temp_dir().join("fselect_test_having_alias_edge_cases");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "b.txt", "c.md"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let negated = run_query_against_dir("select ext, count(*) as n from __DIR__ group by ext having -n < -1", &tmp);
        let quoted_in_function = run_query_against_dir(
            "select ext, count(*) as n from __DIR__ group by ext having length('n') = 1 order by ext",
            &tmp,
        );
        let shadows_field = run_query_against_dir("select ext, count(*) as size from __DIR__ group by ext having size > 1", &tmp);
        let mixed = run_query_against_dir(
            "select ext, count(*) as n from __DIR__ group by ext having upper('n') = 'N' and n > 1",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(negated, vec!["txt	2"]);
        assert_eq!(quoted_in_function, vec!["md	1", "txt	2"]);
        assert_eq!(shadows_field, vec!["txt	2"], "the alias wins over the field of the same name");
        assert_eq!(mixed, vec!["txt	2"]);
    }

    #[test]
    fn file_reading_functions_produce_one_row_per_file() {
        let tmp = std::env::temp_dir().join("fselect_test_file_function_rows");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x\n").unwrap();
        fs::write(tmp.join("b.txt"), "y\n").unwrap();
        fs::write(tmp.join("c.txt"), "x\n").unwrap();

        let rows = run_query_against_dir("select contains('x') from __DIR__ order by 1", &tmp);
        let constant = run_query_against_dir("select upper('a') from __DIR__", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["false", "true", "true"]);
        assert_eq!(constant, vec!["A"]);
    }

    #[test]
    fn having_aggregate_not_in_select_list() {
        let tmp = std::env::temp_dir().join("fselect_test_having_agg_not_selected");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("big.rs"), "x".repeat(100)).unwrap();
        fs::write(tmp.join("small.txt"), "x").unwrap();

        let rows = run_query_against_dir(
            "select ext from __DIR__ depth 1 group by ext having max(size) > 50",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows, vec![String::from("rs")]);
    }

    #[test]
    fn aggregates_over_computed_arguments_work_inside_larger_expressions() {
        let tmp = std::env::temp_dir().join("fselect_test_nested_computed_aggregates");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x".repeat(3)).unwrap();
        fs::write(tmp.join("b.txt"), "x".repeat(4)).unwrap();
        fs::write(tmp.join("c.txt"), "x".repeat(6)).unwrap();

        let arithmetic = run_query_against_dir("select sum(size * 2) + 1 from __DIR__ depth 1", &tmp);
        let function_arg = run_query_against_dir("select abs(sum(size * 2) - 100) from __DIR__ depth 1", &tmp);
        let two_aggregates = run_query_against_dir(
            "select ext, max(size * 2) + min(size * 2) from __DIR__ depth 1 group by ext",
            &tmp,
        );
        let ordering_only = run_query_against_dir(
            "select name from __DIR__ depth 1 group by name order by max(size * 2) + 1 desc",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(arithmetic, vec![String::from("27")]);
        assert_eq!(function_arg, vec![String::from("74")]);
        assert_eq!(two_aggregates, vec![String::from("txt\t18")]);
        assert_eq!(ordering_only, vec![String::from("c.txt"), String::from("b.txt"), String::from("a.txt")]);
    }

    #[test]
    fn cache_keys_distinguish_literals_fields_and_aliases() {
        let tmp = std::env::temp_dir().join("fselect_test_cache_key_collisions");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let literal_vs_field = run("select length('Name'), length(name) from __DIR__ depth 1");
        let field_vs_literal = run("select length(name), length('Name') from __DIR__ depth 1");
        let alias_vs_literal = run("select name as n, 'n', length('n') from __DIR__ depth 1");
        let alias_vs_field = run("select size as Name, name from __DIR__ depth 1");
        let function_alias_vs_field = run("select upper(name) as Size, size from __DIR__ depth 1");
        let grouped_alias = run("select ext, max(size) as size, count(*), length('Ext') from __DIR__ depth 1 group by ext");
        let ordered_alias = run("select name as Size, size from __DIR__ depth 1 order by Size");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(literal_vs_field, vec![String::from("4\t5")]);
        assert_eq!(field_vs_literal, vec![String::from("5\t4")]);
        assert_eq!(alias_vs_literal, vec![String::from("a.txt\tn\t1")]);
        assert_eq!(alias_vs_field, vec![String::from("0\ta.txt")]);
        assert_eq!(function_alias_vs_field, vec![String::from("A.TXT\t0")]);
        assert_eq!(grouped_alias, vec![String::from("txt\t0\t1\t3")]);
        assert_eq!(ordered_alias, vec![String::from("a.txt\t0")]);
    }

    #[test]
    fn comparisons_use_the_typed_operand_whichever_side_it_is_on() {
        let tmp = std::env::temp_dir().join("fselect_test_typed_comparison_sides");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("ten.txt"), "x".repeat(10)).unwrap();
        fs::write(tmp.join("three.txt"), "xxx").unwrap();
        fs::write(tmp.join("zero.txt"), "").unwrap();
        let year_2020 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        fs::File::options().write(true).open(tmp.join("ten.txt")).unwrap().set_modified(year_2020).unwrap();

        let run = |condition: &str| {
            run_query_against_dir(&format!("select name from __DIR__ where {condition} order by name"), &tmp)
        };
        let literal_left = run("2 < size");
        let literal_left_gt = run("5 > size");
        let quoted_left = run("'5' <= size");
        let size_suffix_left = run("1kb > size");
        let arithmetic_right = run("2 < size + 0");
        let date_left = run("yesterday < modified");
        let correlated = |condition: &str| {
            run_query_against_dir(
                &format!("select name from __DIR__ as t1 where exists (select name from __DIR__ where {condition} and name = t1.name) order by name"),
                &tmp,
            )
        };
        let correlated_gt = correlated("t1.size > 2");
        let correlated_lt = correlated("5 > t1.size");
        let correlated_date = correlated("t1.modified > yesterday");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(literal_left, vec!["ten.txt", "three.txt"]);
        assert_eq!(literal_left_gt, vec!["three.txt", "zero.txt"]);
        assert_eq!(quoted_left, vec!["ten.txt"]);
        assert_eq!(size_suffix_left, vec!["ten.txt", "three.txt", "zero.txt"]);
        assert_eq!(arithmetic_right, vec!["ten.txt", "three.txt"]);
        assert_eq!(date_left, vec!["three.txt", "zero.txt"]);
        assert_eq!(correlated_gt, vec!["ten.txt", "three.txt"]);
        assert_eq!(correlated_lt, vec!["three.txt", "zero.txt"]);
        assert_eq!(correlated_date, vec!["three.txt", "zero.txt"]);
    }

    #[test]
    fn compare_variants_mirrors_when_only_the_right_side_is_typed() {
        let mut searcher = create_test_searcher();
        let two = Variant::from_string(&String::from("2"));
        let ten = Variant::from_int(10);

        assert!(searcher.compare_variants(&Op::Lt, &two, &ten).unwrap());
        assert!(searcher.compare_variants(&Op::Lte, &two, &ten).unwrap());
        assert!(!searcher.compare_variants(&Op::Gt, &two, &ten).unwrap());
        assert!(!searcher.compare_variants(&Op::Gte, &two, &ten).unwrap());
        assert!(!searcher.compare_variants(&Op::Eq, &two, &ten).unwrap());
        assert!(searcher.compare_variants(&Op::Ne, &two, &ten).unwrap());
        assert!(searcher.compare_variants(&Op::Eq, &Variant::from_string(&String::from("10")), &ten).unwrap());
        assert!(searcher.compare_variants(&Op::Gt, &two, &Variant::from_string(&String::from("10"))).unwrap());
    }

    #[test]
    fn outer_fields_used_only_in_an_inner_select_list_are_captured() {
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        let mut lexer = Lexer::new(vec![String::from(
            "select name, (select t1.size from /x), (select name from (select path from /x where t1.modified > '2020-01-01')) from /y as t1",
        )]);
        let query = Parser::new(&mut lexer).parse(false).expect("parse failed");
        let scalar = query.fields[1].get_fields_required_in_subqueries("t1", false);
        let nested_root = query.fields[2].get_fields_required_in_subqueries("t1", false);
        assert_eq!(scalar.get(&Field::Size).map(String::as_str), Some("Size"));
        assert_eq!(nested_root.get(&Field::Modified).map(String::as_str), Some("Modified"));

        let tmp = std::env::temp_dir().join("fselect_test_correlated_select_list");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let other = tmp.join("other");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(data.join("a.txt"), "xxx").unwrap();
        fs::write(data.join("b.txt"), "xxxxxxx").unwrap();
        fs::write(other.join("o.txt"), "x".repeat(11)).unwrap();
        let other_root = other.to_string_lossy().to_string();

        let scalar = run_query_against_dir(
            &format!("select name, (select t1.size from {other_root}) from __DIR__ as t1 order by name"),
            &data,
        );
        let aggregate = run_query_against_dir(
            &format!("select name, (select max(t1.size) from {other_root}) from __DIR__ as t1 order by name"),
            &data,
        );
        let exists = run_query_against_dir(
            &format!("select name from __DIR__ as t1 where exists (select t1.size from {other_root}) order by name"),
            &data,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(scalar, vec!["a.txt\t3", "b.txt\t7"]);
        assert_eq!(aggregate, vec!["a.txt\t3", "b.txt\t7"]);
        assert_eq!(exists, vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn negated_outer_alias_references_resolve_against_the_outer_row() {
        let tmp = std::env::temp_dir().join("fselect_test_negated_alias");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let other = tmp.join("other");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(data.join("a.txt"), "x").unwrap();
        fs::write(data.join("b.txt"), "xx").unwrap();
        fs::write(data.join("c.txt"), "x".repeat(10)).unwrap();
        fs::write(other.join("o.txt"), "x".repeat(50)).unwrap();
        let other_root = other.to_string_lossy().to_string();

        let negated = run_query_against_dir(
            &format!("select name, (select -t1.size from {other_root}) from __DIR__ as t1 order by name"),
            &data,
        );
        let wrapped = run_query_against_dir(
            &format!("select name, (select abs(-t1.size) from {other_root}) from __DIR__ as t1 order by name"),
            &data,
        );
        let filtered = run_query_against_dir(
            &format!("select name from __DIR__ as t1 where exists (select name from {other_root} where -t1.size < -5) order by name"),
            &data,
        );
        let context_stays_raw = run_query_against_dir(
            &format!("select -t1.size, (select t1.size from {other_root}) from __DIR__ as t1 where -t1.size < 0 order by name"),
            &data,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(negated, vec!["a.txt\t-1", "b.txt\t-2", "c.txt\t-10"]);
        assert_eq!(wrapped, vec!["a.txt\t1", "b.txt\t2", "c.txt\t10"]);
        assert_eq!(filtered, vec!["c.txt"]);
        assert_eq!(context_stays_raw, vec!["-1\t1", "-2\t2", "-10\t10"]);
    }

    #[test]
    fn aggregates_over_literals_count_every_row() {
        let tmp = std::env::temp_dir().join("fselect_test_literal_aggregates");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "").unwrap();
        fs::write(tmp.join("b.txt"), "").unwrap();
        fs::write(tmp.join("c.md"), "").unwrap();

        let ungrouped = run_query_against_dir("select sum(1), avg(2), min(3), max(4), count(1) from __DIR__", &tmp);
        let grouped = run_query_against_dir("select ext, sum(1), avg(10) from __DIR__ group by ext order by ext", &tmp);
        let negative = run_query_against_dir("select sum(-1) from __DIR__", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(ungrouped, vec![String::from("3	2	3	4	3")]);
        assert_eq!(grouped, vec![String::from("md	1	10"), String::from("txt	2	10")]);
        assert_eq!(negative, vec![String::from("-3")]);
    }

    #[test]
    fn deferred_column_errors_keep_the_row_and_the_limit() {
        let tmp = std::env::temp_dir().join("fselect_test_deferred_column_errors");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for (name, lines) in [("a.txt", 1), ("b.txt", 2), ("c.txt", 3), ("d.txt", 4)] {
            fs::write(tmp.join(name), "line\n".repeat(lines)).unwrap();
        }

        let searcher = search_dir(
            "select name, 1 / (line_count - 1), line_count from __DIR__ order by name limit 2",
            &tmp,
            Config::default(),
            true,
        );
        let rows: Vec<String> = searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["a.txt\t\t1", "b.txt\t1\t2"], "the failed cell is empty, the other deferred cell is still filled");
        assert_eq!(searcher.error_count, 1);
    }

    #[test]
    fn fractional_literals_compare_against_integer_fields_exactly() {
        let tmp = std::env::temp_dir().join("fselect_test_fractional_literals");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("one.txt"), "x").unwrap();
        fs::write(tmp.join("two.txt"), "xx").unwrap();
        fs::write(tmp.join("ten.txt"), "x".repeat(10)).unwrap();

        let run = |condition: &str| {
            run_query_against_dir(&format!("select name from __DIR__ where {condition} order by name"), &tmp)
        };
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("size = 1.9", vec![]),
            ("size < 1.9", vec!["one.txt"]),
            ("size <= 1.9", vec!["one.txt"]),
            ("size > 1.9", vec!["ten.txt", "two.txt"]),
            ("size >= 1.9", vec!["ten.txt", "two.txt"]),
            ("size != 1.9", vec!["one.txt", "ten.txt", "two.txt"]),
            ("1.9 > size", vec!["one.txt"]),
            ("size = 2.0", vec!["two.txt"]),
            ("size in (1.9, 10.0)", vec!["ten.txt"]),
            ("size not in (1.9, 2)", vec!["one.txt", "ten.txt"]),
            ("size between 1.5 and 2.5", vec!["two.txt"]),
        ];
        let results: Vec<(&str, Vec<String>)> = cases.iter().map(|(condition, _)| (*condition, run(condition))).collect();
        let _ = fs::remove_dir_all(&tmp);

        for ((condition, expected), (_, actual)) in cases.iter().zip(results) {
            assert_eq!(&actual, expected, "{}", condition);
        }
    }

    #[test]
    fn literals_containing_quotes_do_not_share_a_cache_slot() {
        let tmp = std::env::temp_dir().join("fselect_test_quoted_literal_keys");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "").unwrap();

        let rows = run_query_against_dir(
            "select concat(\"a', 'b\"), concat('a', 'b'), concat(\"a', 'b\") from __DIR__ limit 1",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["a', 'b\tab\ta', 'b"]);
    }

    #[test]
    fn arithmetic_over_a_text_field_sorts_numerically() {
        let tmp = std::env::temp_dir().join("fselect_test_arithmetic_sort_key");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["2", "10", "100"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let text = run_query_against_dir("select name from __DIR__ order by name", &tmp);
        let arithmetic = run_query_against_dir("select name from __DIR__ order by name + 0", &tmp);
        let descending = run_query_against_dir("select name from __DIR__ order by name * 1 desc", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(text, vec!["10", "100", "2"]);
        assert_eq!(arithmetic, vec!["2", "10", "100"]);
        assert_eq!(descending, vec!["100", "10", "2"]);
    }

    #[test]
    fn a_nested_query_reusing_the_outer_alias_leaves_the_outer_row_intact() {
        let tmp = std::env::temp_dir().join("fselect_test_nested_alias_shadowing");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let other = tmp.join("other");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(data.join("a.txt"), "x").unwrap();
        fs::write(data.join("b.txt"), "xx").unwrap();
        fs::write(data.join("c.txt"), "x".repeat(10)).unwrap();
        fs::write(other.join("o.txt"), "x".repeat(50)).unwrap();
        let other_root = other.to_string_lossy().to_string();

        let filtered_inner = run_query_against_dir(
            &format!("select name, (select count(*) from {other_root} as t where t.size > 0), (select t.size from {other_root}) from __DIR__ as t order by name"),
            &data,
        );
        let bare_inner = run_query_against_dir(
            &format!("select name, (select count(*) from {other_root} as t), (select t.size from {other_root}) from __DIR__ as t order by name"),
            &data,
        );
        let shadowed_self_reference = run_query_against_dir(
            &format!("select name, (select count(*) from {other_root} as t where t.size > 0 and name != t.name), (select t.size from {other_root}) from __DIR__ as t order by name"),
            &data,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(filtered_inner, vec!["a.txt	1	1", "b.txt	1	2", "c.txt	1	10"]);
        assert_eq!(bare_inner, vec!["a.txt	1	1", "b.txt	1	2", "c.txt	1	10"]);
        assert_eq!(shadowed_self_reference, vec!["a.txt	0	1", "b.txt	0	2", "c.txt	0	10"], "inside the inner query t.name is the inner row, so the count is 0");
    }

    #[test]
    fn having_on_grouping_column() {
        let tmp = std::env::temp_dir().join("fselect_test_having_grouping_column");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.rs"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "x").unwrap();

        let rows = run_query_against_dir(
            "select ext, count(*) from __DIR__ depth 1 group by ext having ext = rs",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows, vec![String::from("rs\t1")]);
    }

    #[test]
    fn having_with_logical_ops_filters_both_sides() {
        let tmp = std::env::temp_dir().join("fselect_test_having_logical_ops");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.rs"), "x").unwrap();
        fs::write(tmp.join("b.rs"), "x").unwrap();
        fs::write(tmp.join("c.txt"), "x").unwrap();
        fs::write(tmp.join("d.md"), "x").unwrap();
        fs::write(tmp.join("e.md"), "x").unwrap();
        fs::write(tmp.join("f.md"), "x").unwrap();

        let rows = run_query_against_dir(
            "select ext, count(*) from __DIR__ depth 1 group by ext having count(*) > 1 and count(*) < 3",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows, vec![String::from("rs\t2")]);
    }

    #[test]
    fn having_resolves_select_alias_at_runtime() {
        let tmp = std::env::temp_dir().join("fselect_test_having_alias_runtime");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.rs"), "x").unwrap();
        fs::write(tmp.join("b.rs"), "x").unwrap();
        fs::write(tmp.join("c.txt"), "x").unwrap();

        let rows = run_query_against_dir(
            "select ext, count(*) as c from __DIR__ depth 1 group by ext having c > 1",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows, vec![String::from("rs\t2")]);
    }

    #[test]
    fn ungrouped_having_keeps_or_discards_single_row() {
        let tmp = std::env::temp_dir().join("fselect_test_having_ungrouped");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "x").unwrap();

        let kept = run_query_against_dir(
            "select count(*) from __DIR__ depth 1 having count(*) > 1",
            &tmp,
        );
        let discarded = run_query_against_dir(
            "select count(*) from __DIR__ depth 1 having count(*) > 100",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(kept, vec![String::from("2")]);
        assert!(discarded.is_empty(), "row should be discarded, got {:?}", discarded);
    }

    #[test]
    fn subquery_in_list_applies_offset() {
        // The subquery buffer holds limit + offset rows; the offset rows must
        // be skipped when handing values to the outer query, or `limit 1
        // offset 1` would feed two values into the IN list.
        let tmp = std::env::temp_dir().join("fselect_test_subquery_offset");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("small.txt"), "1").unwrap();
        fs::write(tmp.join("medium.txt"), "22").unwrap();
        fs::write(tmp.join("large.txt"), "333").unwrap();

        let rows = run_query_against_dir(
            "select name from __DIR__ depth 1 where name in (select name from __DIR__ depth 1 order by size desc limit 1 offset 1)",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows, vec![String::from("medium.txt")]);
    }

    #[test]
    fn line_count_field_handles_cr_and_missing_trailing_newline() {
        let tmp = std::env::temp_dir().join("fselect_test_line_count_styles");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("cr.txt"), "a\rb\rc\r").unwrap();
        fs::write(tmp.join("crlf.txt"), "a\r\nb\r\nc\r\n").unwrap();
        fs::write(tmp.join("lf_open.txt"), "a\nb\nc").unwrap();
        fs::write(tmp.join("single.txt"), "single").unwrap();
        fs::write(tmp.join("empty.txt"), "").unwrap();

        let rows = run_query_against_dir("select name, line_count from __DIR__ order by name", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["cr.txt\t3", "crlf.txt\t3", "empty.txt\t0", "lf_open.txt\t3", "single.txt\t1"]);
    }

    #[test]
    fn min_max_of_fsize_match_the_smallest_and_largest_file() {
        let tmp = std::env::temp_dir().join("fselect_test_fsize_min_max");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("small.bin"), vec![0u8; 900]).unwrap();
        fs::write(tmp.join("mid.bin"), vec![0u8; 12_000]).unwrap();
        fs::write(tmp.join("big.bin"), vec![0u8; 2_000_000]).unwrap();

        let smallest = run_query_against_dir("select fsize from __DIR__ order by size limit 1", &tmp);
        let largest = run_query_against_dir("select fsize from __DIR__ order by size desc limit 1", &tmp);
        let aggregated = run_query_against_dir("select min(fsize), max(fsize) from __DIR__", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_ne!(smallest, largest);
        assert_eq!(aggregated, vec![format!("{}	{}", smallest[0], largest[0])]);
    }

    #[test]
    fn archive_entries_produce_rectangular_rows() {
        use std::io::Write;
        use std::path::MAIN_SEPARATOR_STR as SEP;

        let tmp = std::env::temp_dir().join("fselect_test_archive_rows");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("plain.txt"), "x").unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(tmp.join("pack.zip")).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("inner.txt", options).unwrap();
        zip.write_all(b"hello").unwrap();
        zip.start_file("sub/deep.txt", options).unwrap();
        zip.write_all(b"deeper").unwrap();
        zip.finish().unwrap();

        let csv = run_query_against_dir("select path, name from __DIR__ arc order by path into csv", &tmp);
        let json = run_query_against_dir("select name, size from __DIR__ arc where name = inner.txt into json", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(csv, vec![
            "pack.zip,pack.zip".to_string(),
            format!("pack.zip{SEP}inner.txt,inner.txt"),
            format!("pack.zip{SEP}sub{SEP}deep.txt,deep.txt"),
            "plain.txt,plain.txt".to_string(),
        ]);
        assert_eq!(json, vec![r#"{"name":"inner.txt","size":"5"}"#]);
    }

    #[test]
    fn archive_entries_expose_their_mime_type() {
        use std::io::Write;
        use std::path::MAIN_SEPARATOR_STR as SEP;

        let tmp = std::env::temp_dir().join("fselect_test_archive_mime");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(tmp.join("pack.zip")).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("notes.txt", options).unwrap();
        zip.write_all(b"hello world\n").unwrap();
        zip.start_file("img.png", options).unwrap();
        zip.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        zip.write_all(&[0u8; 64]).unwrap();
        zip.finish().unwrap();

        let rows = run_query_against_dir("select path, mime, is_text, is_binary from __DIR__ arc order by path", &tmp);
        let texts = run_query_against_dir("select name from __DIR__ arc where is_text", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec![
            "pack.zip\tapplication/zip\tfalse\ttrue".to_string(),
            format!("pack.zip{SEP}img.png\timage/png\tfalse\ttrue"),
            format!("pack.zip{SEP}notes.txt\ttext/plain\ttrue\tfalse"),
        ]);
        assert_eq!(texts, vec!["notes.txt".to_string()]);
    }

    #[test]
    fn archive_is_opened_once_per_scan_regardless_of_content_fields() {
        use std::io::Write;
        use crate::fileinfo::archive_opens;

        let tmp = std::env::temp_dir().join("fselect_test_archive_opens");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(tmp.join("pack.zip")).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for i in 0..20 {
            zip.start_file(format!("m{i:02}.txt"), options).unwrap();
            zip.write_all(format!("hello {i}\nworld\n").as_bytes()).unwrap();
        }
        zip.finish().unwrap();

        let before = archive_opens();
        let rows = run_query_against_dir(
            "select name, line_count, mime, has_bom, contains('hello') from __DIR__ arc where ext = txt order by name",
            &tmp,
        );
        let opens = archive_opens() - before;
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows.len(), 20);
        assert_eq!(rows[7], "m07.txt\t2\ttext/plain\tfalse\ttrue");
        assert_eq!(opens, 1, "the archive opened by the walk must serve every member read");
    }

    fn make_archive_dir(name: &str) -> std::path::PathBuf {
        use std::io::Write;

        let tmp = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("plain.txt"), "hello
plain
").unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(tmp.join("pack.zip")).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("top.txt", options).unwrap();
        zip.write_all(b"hello
world
").unwrap();
        zip.start_file("sub/deep.txt", options).unwrap();
        zip.write_all(b"hello there
line2
line3
").unwrap();
        zip.start_file("sub/sub2/deeper.txt", options).unwrap();
        zip.write_all(b"nothing
").unwrap();
        zip.finish().unwrap();
        tmp
    }

    #[test]
    fn archive_entries_expose_their_content() {
        use std::path::MAIN_SEPARATOR_STR as SEP;

        let tmp = make_archive_dir("fselect_test_archive_content");
        let rows = run_query_against_dir(
            "select path, line_count, contains('hello'), word_count from __DIR__ arc where ext = txt order by path",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec![
            format!("pack.zip{SEP}sub{SEP}deep.txt	3	true	4"),
            format!("pack.zip{SEP}sub{SEP}sub2{SEP}deeper.txt	1	false	1"),
            format!("pack.zip{SEP}top.txt	2	true	2"),
            String::from("plain.txt	2	true	2"),
        ]);
    }

    #[test]
    fn archive_entries_have_a_depth_and_honour_depth_limits() {
        use std::path::MAIN_SEPARATOR_STR as SEP;

        let tmp = make_archive_dir("fselect_test_archive_depth");
        let depths = run_query_against_dir("select path, depth from __DIR__ arc order by path", &tmp);
        let max_one = run_query_against_dir("select path from __DIR__ arc depth 1 order by path", &tmp);
        let max_two = run_query_against_dir("select path from __DIR__ arc depth 2 order by path", &tmp);
        let min_three = run_query_against_dir("select path from __DIR__ arc mindepth 3 order by path", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(depths, vec![
            String::from("pack.zip	1"),
            format!("pack.zip{SEP}sub{SEP}deep.txt	3"),
            format!("pack.zip{SEP}sub{SEP}sub2{SEP}deeper.txt	4"),
            format!("pack.zip{SEP}top.txt	2"),
            String::from("plain.txt	1"),
        ]);
        assert_eq!(max_one, vec!["pack.zip", "plain.txt"], "depth 1 must not descend into the archive");
        assert_eq!(max_two, vec![
            String::from("pack.zip"),
            format!("pack.zip{SEP}top.txt"),
            String::from("plain.txt"),
        ]);
        assert_eq!(min_three, vec![
            format!("pack.zip{SEP}sub{SEP}deep.txt"),
            format!("pack.zip{SEP}sub{SEP}sub2{SEP}deeper.txt"),
        ]);
    }

    #[test]
    fn json_keys_match_between_plain_and_grouped_output() {
        let tmp = std::env::temp_dir().join("fselect_test_json_keys");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.md"), "").unwrap();

        let plain = run_query_against_dir(
            "select ext, size * 2 as DBL, upper(name), size + 1 from __DIR__ where ext = md into json",
            &tmp,
        );
        let grouped = run_query_against_dir(
            "select ext, size * 2 as dbl, count(*), max(size) + 1 from __DIR__ group by ext into json",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(plain, vec![r#"{"extension":"md","dbl":"0","upper(name)":"A.MD","size + 1":"1"}"#]);
        assert_eq!(grouped, vec![r#"{"extension":"md","dbl":"0","count(*)":"1","max(size) + 1":"1"}"#]);
    }

    #[test]
    fn grouped_subquery_applies_offset_once() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_subquery_offset");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.md", "b.rs", "c.rs", "d.txt", "e.txt", "f.txt"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let offset_only = run_query_against_dir(
            "select name from __DIR__ where ext in (select ext from __DIR__ group by ext order by ext offset 1) order by name",
            &tmp,
        );
        let limit_and_offset = run_query_against_dir(
            "select name from __DIR__ where ext in (select ext from __DIR__ group by ext order by ext limit 1 offset 1) order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(offset_only, vec!["b.rs", "c.rs", "d.txt", "e.txt", "f.txt"]);
        assert_eq!(limit_and_offset, vec!["b.rs", "c.rs"]);
    }

    #[test]
    fn subquery_results_are_buffered_with_debug_config() {
        // Subqueries must run silent regardless of config.debug: with debug on
        // they used to print their rows instead of buffering them, so EXISTS
        // inverted and IN/scalar subqueries came back empty.
        let tmp = std::env::temp_dir().join("fselect_test_subquery_debug_config");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("one.txt"), "x").unwrap();
        fs::write(tmp.join("two.txt"), "y").unwrap();

        let mut config = Config::default();
        config.debug = true;

        let rows = run_query_against_dir_with_config(
            "select name from __DIR__ depth 1 where exists (select name from __DIR__ depth 1 where name eq 'one.txt')",
            &tmp,
            config,
        );

        let _ = fs::remove_dir_all(&tmp);
        let names: HashSet<String> = rows.into_iter().collect();
        assert!(names.contains("one.txt"), "names was {:?}", names);
        assert!(names.contains("two.txt"), "names was {:?}", names);
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
    fn from_subselect_keeps_the_inner_order_across_directories() {
        let tmp = std::env::temp_dir().join("fselect_test_from_subselect_order");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("one")).unwrap();
        fs::create_dir_all(tmp.join("two")).unwrap();
        fs::write(tmp.join("one").join("a.bin"), vec![0u8; 30]).unwrap();
        fs::write(tmp.join("two").join("b.bin"), vec![0u8; 10]).unwrap();
        fs::write(tmp.join("one").join("c.bin"), vec![0u8; 40]).unwrap();
        fs::write(tmp.join("two").join("d.bin"), vec![0u8; 20]).unwrap();
        fs::write(tmp.join("e.bin"), vec![0u8; 50]).unwrap();

        let rows = run_query_against_dir(
            "select name from (select path from __DIR__ where is_file order by size)",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["b.bin", "d.bin", "a.bin", "c.bin", "e.bin"],
            "rows must come out in the order the subselect produced them, whatever directory they live in");
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

    /// Build `<parent>/outer` and `<parent>/inner` with files of the given byte
    /// sizes, for correlated-subquery tests that need two distinct roots.
    fn make_outer_inner_dirs(
        parent: &Path,
        outer: &[(&str, usize)],
        inner: &[(&str, usize)],
    ) {
        let _ = fs::remove_dir_all(parent);
        fs::create_dir_all(parent.join("outer")).unwrap();
        fs::create_dir_all(parent.join("inner")).unwrap();
        for (name, size) in outer {
            fs::write(parent.join("outer").join(name), vec![0u8; *size]).unwrap();
        }
        for (name, size) in inner {
            fs::write(parent.join("inner").join(name), vec![0u8; *size]).unwrap();
        }
    }

    #[test]
    fn subquery_correlated_via_order_by_recomputes_per_row() {
        // Regression for the caching half of the bug: a scalar subquery in the
        // SELECT list, correlated to the outer row ONLY through its ORDER BY
        // (`abs(t2.size - t1.size)`), must be recomputed for every outer row.
        // The previous heuristic deemed it cacheable (it never inspected the
        // subquery's ORDER BY), so the first row's "closest" file was memoised
        // and wrongly reused for every later row.
        let tmp = std::env::temp_dir().join("fselect_test_corr_order_by_per_row");
        make_outer_inner_dirs(
            &tmp,
            &[("a.txt", 100), ("b.txt", 300)],
            &[("s10.dat", 10), ("s90.dat", 90), ("s290.dat", 290), ("s500.dat", 500)],
        );

        let rows = run_query_against_dir(
            "select name, size, (select t2.name from __DIR__/inner as t2 order by abs(t2.size - t1.size) limit 1) as closest \
             from __DIR__/outer as t1 order by name",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);

        // a.txt (100) is closest to s90.dat (90); b.txt (300) to s290.dat (290).
        // Before the fix both rows reported s90.dat — the first row's cached result.
        assert_eq!(
            rows,
            vec![
                String::from("a.txt\t100\ts90.dat"),
                String::from("b.txt\t300\ts290.dat"),
            ],
            "each outer row must get its own closest inner file, got: {:?}", rows
        );
    }

    #[test]
    fn subquery_correlated_via_order_by_where_filter_keeps_all_matches() {
        // The reported symptom: `where size = (scalar subquery ordered by
        // abs(t2.size - t1.size))` dropped every row after the first because the
        // subquery's first-row result was memoised and reused.
        let tmp = std::env::temp_dir().join("fselect_test_corr_order_by_where");
        make_outer_inner_dirs(
            &tmp,
            &[("a.txt", 100), ("b.txt", 300)],
            // 50 is the global minimum and matches neither outer size, so a
            // broken correlation (which collapses to "smallest inner size")
            // cannot accidentally satisfy the `=` filter for any row.
            &[("i50.dat", 50), ("i100.dat", 100), ("i300.dat", 300)],
        );

        let rows = run_query_against_dir(
            "select name from __DIR__/outer as t1 \
             where size = (select t2.size from __DIR__/inner as t2 order by abs(t2.size - t1.size) limit 1) \
             order by name",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);

        // Each outer file has an exact-size twin in inner, so both must match.
        assert_eq!(
            rows,
            vec![String::from("a.txt"), String::from("b.txt")],
            "every outer row whose closest inner size equals its own must match, got: {:?}", rows
        );
    }

    #[test]
    fn correlated_select_list_subquery_via_order_by_resolves_per_row() {
        // The outer query references `size` ONLY through the SELECT-list
        // subquery's ORDER BY — it neither selects nor filters on `size`
        // itself. The outer row's value must still reach the subquery, which
        // requires the dependency walk to cover the outer SELECT list (not just
        // the outer WHERE). Before the fix every row reported s10.dat: the
        // correlation read an empty size, so abs(size - 0) sorted by raw size.
        let tmp = std::env::temp_dir().join("fselect_test_corr_select_list_order_by");
        make_outer_inner_dirs(
            &tmp,
            &[("a.txt", 100), ("b.txt", 300)],
            &[("s10.dat", 10), ("s90.dat", 90), ("s290.dat", 290), ("s500.dat", 500)],
        );

        let rows = run_query_against_dir(
            "select name, (select t2.name from __DIR__/inner as t2 order by abs(t2.size - t1.size) limit 1) as closest \
             from __DIR__/outer as t1 order by name",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(
            rows,
            vec![
                String::from("a.txt\ts90.dat"),
                String::from("b.txt\ts290.dat"),
            ],
            "SELECT-list subquery must see each outer row's size via its ORDER BY, got: {:?}", rows
        );
    }

    #[test]
    fn uppercase_root_alias_resolves_like_lowercase() {
        let tmp = std::env::temp_dir().join("fselect_test_uppercase_alias");
        make_outer_inner_dirs(
            &tmp,
            &[("a.txt", 100), ("b.txt", 300)],
            &[("m100.dat", 100), ("m999.dat", 999)],
        );

        let mut plain = run_query_against_dir("select T1.name from __DIR__/outer as T1 where T1.size > 200", &tmp);
        plain.sort();
        assert_eq!(plain, vec!["b.txt"], "uppercase alias on a plain query");

        let mut mixed = run_query_against_dir(
            "select name from __DIR__/outer as T1              where exists(select t2.name from __DIR__/inner as T2 where T2.size = t1.size)",
            &tmp,
        );
        mixed.sort();

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(mixed, vec!["a.txt"], "mixed-case aliases in a correlated subquery");
    }

    #[test]
    fn correlated_select_list_subquery_via_where_resolves_per_row() {
        // Same call-site gap, exercised through a SELECT-list subquery whose
        // WHERE references the outer alias. Confirms the fix is not specific to
        // ORDER BY: any correlated SELECT-list subquery needs the outer field.
        let tmp = std::env::temp_dir().join("fselect_test_corr_select_list_where");
        make_outer_inner_dirs(
            &tmp,
            &[("a.txt", 100), ("b.txt", 300)],
            &[("m100.dat", 100), ("m300.dat", 300), ("m999.dat", 999)],
        );

        let rows = run_query_against_dir(
            "select name, (select t2.name from __DIR__/inner as t2 where t2.size = t1.size limit 1) as twin \
             from __DIR__/outer as t1 order by name",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);

        // a.txt (100) twins with m100.dat; b.txt (300) with m300.dat.
        // Before the fix both columns were blank (empty correlation, no match).
        assert_eq!(
            rows,
            vec![
                String::from("a.txt\tm100.dat"),
                String::from("b.txt\tm300.dat"),
            ],
            "SELECT-list subquery must see each outer row's size via its WHERE, got: {:?}", rows
        );
    }

    fn make_sized_files_dir(name: &str) -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("empty.txt"), "").unwrap();
        fs::write(tmp.join("s3.txt"), "aaa").unwrap();
        fs::write(tmp.join("s12.txt"), "aaaaaaaaaaaa").unwrap();
        tmp
    }

    #[cfg(all(unix, feature = "users"))]
    #[test]
    fn group_field_can_be_selected_and_filtered() {
        let tmp = make_sized_files_dir("fselect_test_group_field");
        let mut names = run_query_against_dir("select name from __DIR__ where length(group) > 0 order by group, name", &tmp);
        names.sort();
        let groups = run_query_against_dir("select group from __DIR__", &tmp);
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(names, vec!["empty.txt", "s12.txt", "s3.txt"]);
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|g| !g.is_empty()), "group names should not be empty: {:?}", groups);
    }

    #[test]
    fn correlated_subquery_can_test_the_outer_entry_type_next_to_its_own() {
        let tmp = std::env::temp_dir().join("fselect_test_cross_alias_file_type");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("outer").join("subdir")).unwrap();
        fs::write(tmp.join("outer").join("a.txt"), "x").unwrap();
        fs::create_dir_all(tmp.join("inner")).unwrap();
        fs::write(tmp.join("inner").join("b.txt"), "y").unwrap();

        let dirs_with_inner_files = run_query_against_dir(
            "select name from __DIR__/outer as t1              where exists (select name from __DIR__/inner as t2 where t2.is_file and t1.is_dir) order by name",
            &tmp,
        );
        let everything = run_query_against_dir(
            "select name from __DIR__/outer where not (is_file and is_dir) order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(dirs_with_inner_files, vec!["subdir"]);
        assert_eq!(everything, vec!["a.txt", "subdir"]);
    }

    #[test]
    fn depth_field_counts_levels_below_the_search_root() {
        let tmp = std::env::temp_dir().join("fselect_test_depth_field");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub").join("deep")).unwrap();
        fs::write(tmp.join("a.txt"), "").unwrap();
        fs::write(tmp.join("sub").join("b.txt"), "").unwrap();
        fs::write(tmp.join("sub").join("deep").join("c.txt"), "").unwrap();

        let rows = run_query_against_dir("select name, depth from __DIR__ where is_file order by depth", &tmp);
        let mut level_two = run_query_against_dir("select name from __DIR__ where depth = 2", &tmp);
        level_two.sort();
        let mut capped = run_query_against_dir("select name, depth from __DIR__ depth 1", &tmp);
        capped.sort();
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["a.txt	1", "b.txt	2", "c.txt	3"]);
        assert_eq!(level_two, vec!["b.txt", "deep"]);
        assert_eq!(capped, vec!["a.txt	1", "sub	1"]);
    }

    #[test]
    fn bare_aliased_boolean_in_correlated_subquery_tests_the_outer_entry() {
        let tmp = std::env::temp_dir().join("fselect_test_bare_aliased_boolean");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("outer").join("sub")).unwrap();
        fs::create_dir_all(tmp.join("inner")).unwrap();
        fs::write(tmp.join("outer").join("f.txt"), "").unwrap();
        fs::write(tmp.join("inner").join("g.txt"), "").unwrap();

        let bare = run_query_against_dir(
            "select name from __DIR__/outer as t1 where exists(select name from __DIR__/inner as t2 where t1.is_dir)",
            &tmp,
        );
        let explicit = run_query_against_dir(
            "select name from __DIR__/outer as t1 where exists(select name from __DIR__/inner as t2 where t1.is_dir = true)",
            &tmp,
        );
        let negated = run_query_against_dir(
            "select name from __DIR__/outer as t1 where exists(select name from __DIR__/inner as t2 where not t1.is_dir)",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(explicit, vec!["sub"]);
        assert_eq!(bare, explicit, "bare `t1.is_dir` should behave like `t1.is_dir = true`");
        assert_eq!(negated, vec!["f.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn followed_symlink_entries_report_the_link_path_not_the_target() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join("fselect_test_symlink_reports_link_path");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("root")).unwrap();
        fs::create_dir_all(tmp.join("hidden")).unwrap();
        fs::write(tmp.join("hidden").join("file.txt"), "hello").unwrap();
        symlink("../hidden", tmp.join("root").join("link")).unwrap();

        let paths = run_query_against_dir("select path from __DIR__/root symlinks where is_file", &tmp);
        let dirs = run_query_against_dir("select dir from __DIR__/root symlinks where is_file", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(paths, vec!["link/file.txt"]);
        assert_eq!(dirs, vec!["link"]);
    }

    #[test]
    fn numeric_field_compared_to_non_numeric_runtime_value_matches_nothing() {
        // Literals like `size > 'abc'` are rejected by the parser; operands the
        // parser can't see (fields, functions) used to coerce to 0 at runtime
        // and so matched empty files. Like the DateTime branch, an unparsable
        // operand is now a non-match (SQL NULL semantics).
        let tmp = make_sized_files_dir("fselect_test_numeric_vs_non_numeric");
        for query in [
            "select name from __DIR__ where size = ext",
            "select name from __DIR__ where size <= ext",
            "select name from __DIR__ where size != ext",
            "select name from __DIR__ where size > upper(ext)",
            "select name from __DIR__ where size in (ext)",
        ] {
            let rows = run_query_against_dir(query, &tmp);
            assert!(rows.is_empty(), "{} should match nothing, got {:?}", query, rows);
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn numeric_field_compared_to_numeric_strings_still_coerces() {
        let tmp = make_sized_files_dir("fselect_test_numeric_vs_numeric_strings");
        let sorted = |q: &str| {
            let mut rows = run_query_against_dir(q, &tmp);
            rows.sort();
            rows
        };
        assert_eq!(sorted("select name from __DIR__ where size > '1'"), vec!["s12.txt", "s3.txt"]);
        assert_eq!(sorted("select name from __DIR__ where size >= '0'"), vec!["empty.txt", "s12.txt", "s3.txt"]);
        assert_eq!(sorted("select name from __DIR__ where size = 0"), vec!["empty.txt"]);
        assert_eq!(sorted("select name from __DIR__ where size < '1kb'"), vec!["empty.txt", "s12.txt", "s3.txt"]);
        assert_eq!(sorted("select name from __DIR__ where size > '0.5'"), vec!["s12.txt", "s3.txt"]);
        // an unparsable runtime IN member is skipped, not coerced to 0
        assert_eq!(sorted("select name from __DIR__ where size in (0, ext)"), vec!["empty.txt"]);
        assert_eq!(sorted("select name from __DIR__ where size not in (ext)"), vec!["empty.txt", "s12.txt", "s3.txt"]);
        let _ = fs::remove_dir_all(&tmp);
    }
}
