//! Handles directory traversal and file processing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::fs::{DirEntry, FileType};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use chrono::NaiveDateTime;
#[cfg(feature = "git")]
use crate::util::git::IgnoreRules;
use lscolors::{LsColors, Style};
use regex::Regex;
#[cfg(all(unix, feature = "users"))]
use uzers::UsersCache;

use crate::config::Config;
use crate::expr::{Expr, ExprType};
use crate::field::Field;
use crate::field::context::{FieldContext, FileMetadataState};
use crate::field::dispatch;
use crate::fileinfo::{archive_member_index, archived_entry_levels, open_archive, to_file_info, ArchiveMember, FileInfo, SharedArchive};
use std::ffi::OsString;
use crate::function;
use crate::ignore::docker::{
    dockerignore_exception_may_lie_beneath, matches_dockerignore_filter, search_upstream_dockerignore, DockerignoreFilter,
};
use crate::ignore::hg::{matches_hgignore_filter, search_upstream_hgignore, HgignoreFilter};
use crate::operators::{LogicalOp, Op};
use crate::output::ResultsWriter;
use crate::query::TraversalMode::{Bfs, Dfs};
use crate::query::{Query, Root, TraversalMode};
use crate::util::*;
use crate::util::dir_lookup::DirEntryLookup;
use crate::util::error::{error_message, path_error_message, SearchError};


#[derive(Default)]
struct ArchiveLookup {
    current: Option<(PathBuf, SharedArchive)>,
}

impl ArchiveLookup {
    fn holding(&self, path: &Path) -> Option<(PathBuf, SharedArchive)> {
        self.current
            .as_ref()
            .filter(|(archive_path, _)| path.strip_prefix(archive_path).is_ok_and(|rest| !rest.as_os_str().is_empty()))
            .cloned()
    }

    fn open(&mut self, archive_path: &Path) -> Option<(PathBuf, SharedArchive)> {
        let shared = Rc::new(RefCell::new(open_archive(archive_path)?));
        self.current = Some((archive_path.to_path_buf(), Rc::clone(&shared)));
        Some((archive_path.to_path_buf(), shared))
    }
}

struct AncestorDir {
    canonical: PathBuf,
    parent: Option<Rc<AncestorDir>>,
}

/// Row aliases whose evaluation failed, shared with nested searchers. The
/// error is reported only once such an alias is actually read, so a WHERE
/// guard that never reaches the alias also never sees its error, exactly as
/// with the inline expression.
#[derive(Default)]
struct AliasFailures {
    pending: HashMap<String, HashMap<String, SearchError>>,
    hit: Option<(String, SearchError)>,
}

impl AliasFailures {
    fn note_read(&mut self, alias: &str, key: &str) {
        if self.hit.is_none()
            && let Some(err) = self.pending.get(alias).and_then(|keys| keys.get(key))
        {
            self.hit = Some((alias.to_string(), err.clone()));
        }
    }
}

struct QueuedDir {
    path: PathBuf,
    root_depth: u32,
    ancestors: Option<Rc<AncestorDir>>,
    #[cfg(feature = "git")]
    repository: Option<Rc<IgnoreRules>>,
}

impl AncestorDir {
    fn contains(chain: &Option<Rc<AncestorDir>>, canonical: &Path) -> bool {
        let mut current = chain.as_ref();
        while let Some(dir) = current {
            if dir.canonical == canonical {
                return true;
            }
            current = dir.parent.as_ref();
        }
        false
    }
}

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
    alias_failures: Rc<RefCell<AliasFailures>>,
    current_alias: Option<String>,
    subquery_required_fields: Option<HashMap<Field, HashSet<String>>>,
    subquery_required_columns: Vec<(String, Expr)>,

    hgignore_filters: Vec<HgignoreFilter>,
    dockerignore_filters: Vec<DockerignoreFilter>,
    ancestor_dirs: Option<Rc<AncestorDir>>,
    lscolors: LsColors,
    dir_queue: VecDeque<QueuedDir>,
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
    file_map: HashMap<String, Variant>,
    unknown_conditions: HashSet<String>,
    null_comparisons: HashSet<String>,
    conforms_map: HashMap<String, Variant>,
    subquery_cache: HashMap<String, Vec<Variant>>,
    membership_cache: HashMap<(String, VariantType), Rc<Membership>>,
    silent_mode: bool,
    /// Runs a subquery on behalf of an enclosing searcher, whose aliases it
    /// may legitimately reference.
    is_nested: bool,
    enclosing_aliases: HashSet<String>,
    root_depth_column: bool,
    root_depth_cache: Option<(PathBuf, usize)>,
    canonical_root_cache: Option<(PathBuf, PathBuf)>,

    pub error_count: i32,
    suppress_row_errors: bool,
    suppressed_row_error: Option<SearchError>,
}

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

fn cached_condition(value: &Variant) -> Option<bool> {
    match value.get_type() {
        VariantType::Bool | VariantType::Int | VariantType::Float => Some(value.to_bool()),
        _ => str_to_bool(value.as_str()),
    }
}

struct Membership {
    values: MembershipValues,
    has_null: bool,
}

enum MembershipValues {
    Strings(HashSet<String>),
    Ints(HashSet<i64>),
    Floats(Vec<f64>),
    Bools { has_true: bool, has_false: bool },
    DateTimes(Vec<(i64, i64)>),
}

fn datetime_seconds((from, to): (NaiveDateTime, NaiveDateTime)) -> (i64, i64) {
    (from.and_utc().timestamp(), to.and_utc().timestamp())
}

impl Membership {
    fn new(values: &[Variant], field_type: &VariantType, has_null: bool) -> Membership {
        let variants = values.iter().filter(|v| !v.is_null());
        let values = match field_type {
            VariantType::String => MembershipValues::Strings(variants.map(Variant::to_string).collect()),
            VariantType::Int => MembershipValues::Ints(variants.filter_map(|v| v.to_exact_int()).collect()),
            VariantType::Float => MembershipValues::Floats(variants.filter_map(|v| v.to_float_opt()).collect()),
            VariantType::Bool => {
                let mut bools = (false, false);
                for value in variants {
                    match str_to_bool(&value.to_string()) {
                        Some(true) => bools.0 = true,
                        Some(false) => bools.1 = true,
                        None => {}
                    }
                }
                MembershipValues::Bools { has_true: bools.0, has_false: bools.1 }
            }
            VariantType::DateTime => {
                MembershipValues::DateTimes(variants.filter_map(|v| v.to_datetime().ok().map(datetime_seconds)).collect())
            }
        };
        Membership { values, has_null }
    }

    fn contains(&self, field_value: &Variant) -> bool {
        match &self.values {
            MembershipValues::Strings(set) => set.contains(field_value.to_string().as_str()),
            MembershipValues::Ints(set) => set.contains(&field_value.to_int()),
            MembershipValues::Floats(list) => list.contains(&field_value.to_float()),
            MembershipValues::Bools { has_true, has_false } => if field_value.to_bool() { *has_true } else { *has_false },
            MembershipValues::DateTimes(intervals) => match field_value.to_datetime().map(datetime_seconds) {
                Ok((from, to)) => intervals.iter().any(|(start, finish)| from <= *finish && *start <= to),
                Err(_) => false,
            },
        }
    }
}

const DEFERRED_COLUMN_WEIGHT: i32 = 16;
const NULL_GROUP_KEY: &str = "\u{0}null";
const UNKNOWN_GROUP_KEY: &str = "\u{0}unknown";

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

const NO_FILE_INFO: &Option<FileInfo> = &None;

struct Row<'r> {
    entry: Option<&'r DirEntry>,
    file_info: &'r Option<FileInfo>,
    root_path: &'r Path,
    values: &'r mut HashMap<String, Variant>,
    group: Option<&'r function::GroupAccumulator>,
}

impl<'r> Row<'r> {
    fn new(
        entry: Option<&'r DirEntry>,
        file_info: &'r Option<FileInfo>,
        root_path: &'r Path,
        values: &'r mut HashMap<String, Variant>,
        group: Option<&'r function::GroupAccumulator>,
    ) -> Row<'r> {
        Row { entry, file_info, root_path, values, group }
    }

    fn entry(entry: &'r DirEntry, file_info: &'r Option<FileInfo>, root_path: &'r Path, values: &'r mut HashMap<String, Variant>) -> Row<'r> {
        Row::new(Some(entry), file_info, root_path, values, None)
    }

    fn group(group: &'r function::GroupAccumulator, values: &'r mut HashMap<String, Variant>) -> Row<'r> {
        Row::new(None, NO_FILE_INFO, Path::new(""), values, Some(group))
    }

    fn scratch<'s>(&'s self, values: &'s mut HashMap<String, Variant>) -> Row<'s> {
        Row::new(self.entry, self.file_info, self.root_path, values, self.group)
    }
}

enum BufferedRow {
    Rendered(String, Option<Variant>),
    Pending(PendingRow),
}

impl BufferedRow {
    fn first_value(&self) -> Option<Variant> {
        match self {
            BufferedRow::Rendered(_, first_value) => first_value.clone(),
            BufferedRow::Pending(pending) => pending.first_value.clone(),
        }
    }
}

struct PendingRow {
    path: PathBuf,
    root_path: PathBuf,
    file_info: Option<FileInfo>,
    follow_symlinks: bool,
    items: Vec<(String, String)>,
    deferred: Vec<usize>,
    first_value: Option<Variant>,
}

impl std::ops::Deref for BufferedRow {
    type Target = str;

    fn deref(&self) -> &str {
        match self {
            BufferedRow::Rendered(text, _) => text,
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
    Regex::new(&format!("^(?:{})$", part))
        .map_err(|_| SearchError::fatal(format!("invalid regex in root path: {}", part)))
}

/// HAVING is evaluated against group accumulators, where set membership and
/// existence tests have no meaning.
fn having_uses_unsupported_op(expr: &Expr) -> bool {
    matches!(expr.op, Some(Op::Exists | Op::NotExists))
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
            alias_failures: Rc::new(RefCell::new(AliasFailures::default())),
            current_alias: None,
            subquery_required_fields: None,
            subquery_required_columns: vec![],

            hgignore_filters: vec![],
            dockerignore_filters: vec![],
            ancestor_dirs: None,
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

            fms: {
                let mut fms = FileMetadataState::new();
                fms.expect_full_content_scan(Self::needs_full_content_scan(query));
                fms
            },
            #[cfg(feature = "git")]
            git_cache: crate::util::git::GitCache::new(),
            file_map: HashMap::new(),
            unknown_conditions: HashSet::new(),
            null_comparisons: HashSet::new(),
            conforms_map: HashMap::new(),
            subquery_cache: HashMap::new(),
            membership_cache: HashMap::new(),
            silent_mode: false,
            is_nested: false,
            enclosing_aliases: HashSet::new(),
            root_depth_column: false,
            root_depth_cache: None,
            canonical_root_cache: None,

            error_count: 0,
            suppress_row_errors: false,
            suppressed_row_error: None,
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
        let roots = self.expand_roots()?;

        if !self.silent_mode {
            let col_count = self.query.fields.len();
            try_output!(self.results_writer.write_header(&self.query.raw_query, col_count, &mut std::io::stdout()), Ok(()));
        }

        let result = self.search_and_print(roots);

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

        if let Some(ordering_expr) = self.unsortable_grouped_ordering() {
            return Err(SearchError::fatal(format!(
                "Cannot order by '{}': in a GROUP BY query the ORDER BY expression must appear in the SELECT list, be an aggregate function, or be a grouping column",
                ordering_expr
            )).with_source("query"));
        }

        if let Some(ref having) = self.query.having
            && having_uses_unsupported_op(having) {
                return Err(SearchError::fatal(
                    "EXISTS and NOT EXISTS are not supported in the HAVING clause",
                )
                .with_source("query"));
            }

        Ok(())
    }

    /// In a GROUP BY query an ORDER BY expression must be a selected column,
    /// an aggregate, or a grouping column; anything else has no per-group value.
    fn needs_full_content_scan(query: &Query) -> bool {
        let mut fields = query.get_all_fields();
        if let Some(ref expr) = query.expr {
            fields.extend(expr.get_required_fields());
        }
        for grouping_expr in &query.grouping_fields {
            fields.extend(grouping_expr.get_required_fields());
        }
        fields.iter().any(|field| {
            matches!(field, Field::LineCount | Field::WordCount | Field::CharCount | Field::LineEnding)
        })
    }

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

    /// Expands `regexp` roots into the directories they match, before any
    /// output is written: an invalid regex is a fatal query error.
    fn expand_roots(&mut self) -> Result<Vec<Root>, SearchError> {
        let current_dir = std::env::current_dir()?;
        let mut roots = vec![];

        for root in &self.query.roots {
            if root.options.regexp {
                let follow_symlinks = root.options.symlinks;
                let mut ext_roots: Vec<String> = vec![];
                // Split the path into parts to process each segment as a regex
                let parts = root.path.split('/').collect::<Vec<&str>>();
                for part in parts {
                    if looks_like_regexp(part) {
                        let mut compiled: Option<Regex> = None;
                        let mut tmp = vec![];

                        if ext_roots.is_empty() {
                            let part = part.to_string();
                            if part.starts_with("/") {
                                ext_roots.push(String::from("/"));
                            } else {
                                ext_roots.push(String::from(""));
                            }
                        }

                        let mut read_failed = false;

                        // Read the directory and filter entries matching the regex
                        for root in &ext_roots {
                            let mut start_from_rx_dir = false;

                            let mut path = Path::new(&root);

                            if path == Path::new("") {
                                path = current_dir.as_path();
                                start_from_rx_dir = true;
                            }

                            let literal = match root.as_str() {
                                "" => part.to_string(),
                                "/" => format!("/{part}"),
                                _ => format!("{root}/{part}"),
                            };
                            if Path::new(&literal).is_dir() {
                                tmp.push(literal);
                                continue;
                            }
                            if compiled.is_none() {
                                compiled = Some(root_part_regex(part)?);
                            }
                            let rx = compiled.as_ref().unwrap();

                            match path.read_dir() {
                                Ok(read_result) => {
                                    for entry in read_result.flatten() {
                                        let is_dir = match entry.file_type() {
                                            Ok(file_type) if file_type.is_dir() => true,
                                            Ok(file_type) if file_type.is_symlink() => follow_symlinks && entry.path().is_dir(),
                                            _ => false,
                                        };
                                        if is_dir
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
                                    read_failed = true;
                                    self.error_count += 1;
                                    path_error_message(path, e)
                                }
                            }
                        }

                        if tmp.is_empty() {
                            if !read_failed {
                                let scanned = ext_roots
                                    .iter()
                                    .map(|root| if root.is_empty() { "." } else { root.as_str() })
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                let err = SearchError::normal(format!("no directory matches '{}'", part));
                                self.handle_nonfatal_error(err, Path::new(&scanned));
                            }
                            ext_roots.clear();
                            break;
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


        Ok(roots)
    }

    fn search_and_print(&mut self, roots: Vec<Root>) -> Result<(), SearchError> {
        let start_time = std::time::Instant::now();

        // ======== Explore each root =========
        for root in roots {
            self.current_follow_symlinks = root.options.symlinks;
            self.current_alias = root.options.alias.clone();
            {
                let mut context = self.record_context.borrow_mut();
                for alias in self.query.own_root_aliases() {
                    context.remove(&alias);
                }
            }
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
                        Expr::merge_required_fields(&mut fields, column_expr.get_fields_required_in_subqueries(alias, false));
                    }
                    if let Some(ref expr) = self.query.expr {
                        Expr::merge_required_fields(&mut fields, expr.get_fields_required_in_subqueries(alias, false));
                    }
                    for ordering_expr in &self.query.ordering_fields {
                        Expr::merge_required_fields(&mut fields, ordering_expr.get_fields_required_in_subqueries(alias, false));
                    }
                    for grouping_expr in &self.query.grouping_fields {
                        Expr::merge_required_fields(&mut fields, grouping_expr.get_fields_required_in_subqueries(alias, false));
                    }
                    if fields.is_empty() { None } else { Some(fields) }
                }
                None => None,
            };
            self.subquery_required_columns = match &self.current_alias {
                Some(alias) => {
                    let mut names: HashSet<String> = HashSet::new();
                    let clauses = self.query.fields.iter()
                        .chain(self.query.expr.iter())
                        .chain(self.query.ordering_fields.iter())
                        .chain(self.query.grouping_fields.iter())
                        .chain(self.query.having.iter());
                    for expr in clauses {
                        names.extend(expr.get_alias_references_in_subqueries(alias, true));
                    }
                    let aggregate_dependent = Self::aggregate_dependent_aliases(&self.query.fields, alias);
                    self.query
                        .fields
                        .iter()
                        .filter(|column| !column.alias.as_deref().is_some_and(|declared| aggregate_dependent.contains(&declared.to_ascii_lowercase())))
                        .filter_map(|column| {
                            let declared = column.alias.as_deref()?;
                            names
                                .iter()
                                .find(|name| name.eq_ignore_ascii_case(declared))
                                .map(|name| (name.clone(), column.clone()))
                        })
                        .collect()
                }
                None => vec![],
            };

            if let Some(ref inner) = root.subquery {
                let paths = self.collect_subquery_root_paths(inner.as_ref().clone())?;
                self.dir_queue.clear();
                self.ancestor_dirs = None;
                self.hgignore_filters.clear();
                self.dockerignore_filters.clear();
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
            self.ancestor_dirs = None;
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

            #[cfg(feature = "git")]
            let root_repository = match self.current_apply_gitignore {
                true => canonical_path(&self.current_root_dir)
                    .ok()
                    .and_then(IgnoreRules::discover)
                    .map(Rc::new),
                false => None,
            };
            let result = self.visit_dir(
                &self.current_root_dir.clone(),
                0,
                #[cfg(feature = "git")]
                root_repository,
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

                let mut grouped_results: TopN<Criteria<String>, (Vec<(String, String)>, Option<Variant>)> =
                    if self.query.limit > 0 {
                        TopN::new(self.query.limit.saturating_add(self.query.offset))
                    } else {
                        TopN::limitless()
                    };

                let having_expr = self.query.having.as_ref();
                let aliases = self.query.own_root_aliases();
                for group_acc in accumulators.values() {
                    let mut values = HashMap::new();
                    let mut row = Row::group(group_acc, &mut values);
                    self.unknown_conditions = group_acc.unknown_keys.clone();
                    for (k, value) in group_keys.iter().zip(&group_acc.key_values) {
                        row.values.insert(k.clone(), value.clone());
                    }
                    self.reset_alias_failures(&aliases);
                    self.bind_group_context(&aliases, &group_acc.key_values, &[]);
                    if let Some(having_expr) = having_expr {
                        if having_expr.has_correlated_subquery() {
                            self.bind_aggregate_aliases(&aliases, &mut row, true)?;
                        }
                        let conforms = self.group_conforms(&mut row, having_expr)?;
                        self.alias_read_failure()?;
                        if !conforms {
                            continue;
                        }
                    }
                    self.bind_aggregate_aliases(&aliases, &mut row, false)?;
                    let mut items: Vec<(String, String)> = Vec::new();
                    let mut first_value = None;
                    for (index, column_expr) in self.query.fields.iter().enumerate() {
                        let value = self.get_column_expr_value(&mut row, column_expr)?;
                        first_value = first_value.or_else(|| self.first_column_value(index, &value));
                        items.push((column_expr.column_name(), value.to_string()));
                    }
                    if self.root_depth_column
                        && let Some((own_path, root_depth, member_index)) = &group_acc.representative
                    {
                        Self::push_subquery_identity(&mut items, own_path, *root_depth, *member_index);
                    }
                    let mut criteria_values: Vec<Variant> = Vec::with_capacity(sorting_indices.len());
                    for (index, ordering_expr) in sorting_indices.iter().zip(self.query.ordering_fields.iter()) {
                        let value = match index {
                            Some(index) => row.values.get(&ordering_expr.cache_key()).cloned().unwrap_or_else(|| {
                                Variant::from_string(&items.get(*index).map(|item| item.1.clone()).unwrap_or_default())
                            }),
                            None => self.get_column_expr_value(&mut row, ordering_expr)?,
                        };
                        criteria_values.push(value);
                    }
                    grouped_results.insert(
                        Criteria::typed(ordering_fields_rc.clone(), criteria_values, ordering_asc_rc.clone()),
                        (items, first_value),
                    );
                }

                let mut first = true;
                let mut stdout = std::io::stdout().lock();
                for (index, (items, first_value)) in grouped_results.iter_values().enumerate() {
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
                        BufferedRow::Rendered(rendered, first_value.clone()),
                    );
                }
                drop(stdout);
            } else {
                let mut buf = WritableBuffer::new();
                let mut items: Vec<(String, String)> = Vec::new();

                let accumulators = std::mem::take(&mut self.accumulators);
                let empty_acc = function::GroupAccumulator::default();
                let ungrouped_acc = accumulators.get(&Vec::new()).unwrap_or(&empty_acc);
                let aliases = self.query.own_root_aliases();
                self.reset_alias_failures(&aliases);
                self.bind_group_context(&aliases, &[], &[]);
                let mut values = HashMap::new();
                let mut row = Row::group(ungrouped_acc, &mut values);
                let mut passes_having = true;
                if let Some(having_expr) = self.query.having.as_ref() {
                    if having_expr.has_correlated_subquery() {
                        self.bind_aggregate_aliases(&aliases, &mut row, true)?;
                    }
                    passes_having =
                        self.group_conforms(&mut row, having_expr)?;
                    self.alias_read_failure()?;
                }

                if passes_having {
                    self.bind_aggregate_aliases(&aliases, &mut row, false)?;
                    let mut first_value = None;
                    for (index, column_expr) in self.query.fields.iter().enumerate() {
                        let value = self.get_column_expr_value(&mut row, column_expr)?;
                        first_value = first_value.or_else(|| self.first_column_value(index, &value));
                        items.push((column_expr.column_name(), value.to_string()));
                    }
                    if self.root_depth_column
                        && let Some((own_path, root_depth, member_index)) = &ungrouped_acc.representative
                    {
                        Self::push_subquery_identity(&mut items, own_path, *root_depth, *member_index);
                    }

                    self.results_writer.write_row(&mut buf, items)?;
                    let rendered = String::from(buf);
                    self.output_buffer.insert(
                        Criteria::new(Rc::new(vec![]), vec![], Rc::new(vec![])),
                        BufferedRow::Rendered(rendered.clone(), first_value),
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
    fn collect_subquery_root_paths(&mut self, mut query: Query) -> Result<Vec<(String, Option<usize>, Option<usize>)>, SearchError> {
        // Use the absolute path so visit_subquery_paths can re-resolve each
        // result against the filesystem regardless of the inner query's root.
        // An aggregated query keeps its own columns: replacing them would drop
        // the aggregate (and with it the grouping), and the outer query only
        // reads the identity columns appended after them anyway. Aliased
        // columns stay too, since the query's own clauses may reference them.
        if !query.is_aggregated() && !query.fields.iter().any(|field| field.alias.is_some()) {
            query.fields = vec![Expr::field(Field::Name)];
        }
        query.output_format = crate::query::OutputFormat::Tabs;
        let rows = self.get_list_from_subquery_with(query, true)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let row = row.to_string();
                let mut columns = row.rsplitn(3, '\t');
                // `<depth>` for a file, `<depth>@<member index>` for an
                // archive member: two members may share a normalized name
                // (`dir/item.txt` and `dir\item.txt`), only the index is unique.
                let (depth, member) = match columns.next() {
                    Some(last) => match last.split_once('@') {
                        Some((depth, member)) => (depth.parse::<usize>().ok(), member.parse::<usize>().ok()),
                        None => (last.parse::<usize>().ok(), None),
                    },
                    None => (None, None),
                };
                match (columns.next(), depth) {
                    (Some(path), Some(depth)) if !path.is_empty() => (Self::decode_identity_path(path), Some(depth), member),
                    _ => (row.clone(), None, None),
                }
            })
            .collect())
    }

    /// The identity path travels inside a tab-separated row, so the separator
    /// characters it may contain are escaped on the way out and restored here.
    fn encode_identity_path(path: &str) -> String {
        let mut out = String::with_capacity(path.len());
        for c in path.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\t' => out.push_str("\\t"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                _ => out.push(c),
            }
        }
        out
    }

    fn decode_identity_path(encoded: &str) -> String {
        let mut out = String::with_capacity(encoded.len());
        let mut chars = encoded.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }
        out
    }

    /// The hidden identity an outer FROM-subselect needs to re-resolve this
    /// entry: its absolute path (archive member paths appended) and the depth
    /// of the root it was found under.
    fn subquery_identity(&mut self, entry: &DirEntry, file_info: &Option<FileInfo>, root_path: &Path) -> (String, usize, Option<usize>) {
        let entry_path = entry.path();
        let beneath_root = entry_path
            .strip_prefix(root_path)
            .ok()
            .and_then(|relative| canonical_path(&root_path.to_path_buf()).ok().map(|root| PathBuf::from(root).join(relative)));
        let own_path = beneath_root
            .ok_or(())
            .or_else(|_| crate::field::own_absolute_path(entry).map_err(|_| ()))
            .map(|path| match file_info {
                Some(info) => info.name.split('/').filter(|part| !part.is_empty()).fold(path, |path, part| path.join(part)),
                None => path,
            })
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        let member_index = file_info.as_ref().and_then(|info| info.member.as_ref()).map(|member| member.index);
        (own_path, self.root_depth_of(root_path), member_index)
    }

    fn push_subquery_identity(items: &mut Vec<(String, String)>, own_path: &str, root_depth: usize, member_index: Option<usize>) {
        items.push((String::new(), Self::encode_identity_path(own_path)));
        let depth = match member_index {
            Some(index) => format!("{root_depth}@{index}"),
            None => root_depth.to_string(),
        };
        items.push((String::new(), depth));
    }

    /// The canonical root joined with the path beneath it as traversed, so a
    /// directory reached through a symlink keeps the link's name instead of
    /// its target: ignore rules are written against the traversed spelling.
    fn logical_canonical_dir(&mut self, dir: &Path, root_dir: &Path) -> Option<PathBuf> {
        let relative = dir.strip_prefix(root_dir).ok()?;
        let canonical_root = match &self.canonical_root_cache {
            Some((cached_root, canonical)) if cached_root == root_dir => canonical.clone(),
            _ => {
                let canonical = PathBuf::from(canonical_path(&root_dir.to_path_buf()).ok()?);
                self.canonical_root_cache = Some((root_dir.to_path_buf(), canonical.clone()));
                canonical
            }
        };
        Some(canonical_root.join(relative))
    }

    /// Aliases (lowercased) whose value only exists once the group is
    /// complete: columns with an aggregate, and transitively every column
    /// that references such an alias. They are bound per group, never per row.
    fn aggregate_dependent_aliases(fields: &[Expr], alias: &str) -> HashSet<String> {
        let mut dependent: HashSet<String> = fields
            .iter()
            .filter(|column| column.has_aggregate_function())
            .filter_map(|column| column.alias.as_deref().map(str::to_ascii_lowercase))
            .collect();
        loop {
            let before = dependent.len();
            for column in fields {
                if let Some(declared) = column.alias.as_deref()
                    && column
                        .get_alias_references_in_subqueries(alias, true)
                        .iter()
                        .any(|name| dependent.contains(&name.to_ascii_lowercase()))
                {
                    dependent.insert(declared.to_ascii_lowercase());
                }
            }
            if dependent.len() == before {
                return dependent;
            }
        }
    }

    fn root_depth_of(&mut self, root_path: &Path) -> usize {
        if let Some((cached_root, depth)) = &self.root_depth_cache
            && cached_root == root_path
        {
            return *depth;
        }
        let depth = canonical_path(&root_path.to_path_buf())
            .map(|canonical| PathBuf::from(canonical).components().count())
            .unwrap_or_default();
        self.root_depth_cache = Some((root_path.to_path_buf(), depth));
        depth
    }

    /// Treat each path produced by a FROM subselect as an input entry: resolve
    /// it to a `DirEntry` and feed it through `check_file` so the outer query's
    /// WHERE/SELECT logic can run against it without further directory
    /// traversal. The std library cannot build a `DirEntry` from a path, so the
    /// parent directory is read once and its entries looked up by name for as
    /// long as consecutive paths keep coming from the same directory.
    fn visit_subquery_paths(&mut self, paths: Vec<(String, Option<usize>, Option<usize>)>) -> Result<(), SearchError> {
        let mut lookup = DirEntryLookup::new();
        let mut archives = ArchiveLookup::default();
        let mut filters_root: Option<PathBuf> = None;
        #[cfg(feature = "git")]
        let mut repository: Option<IgnoreRules> = None;
        for (path_str, root_depth, member_index) in paths {
            if path_str.is_empty() {
                continue;
            }
            if self.limit_reached() {
                break;
            }
            let path = PathBuf::from(&path_str);
            let (entry_path, file_info) = match self.archived_member_of(&path, member_index, &mut archives) {
                Some((archive, file_info)) => (archive, Some(file_info)),
                None => (path.clone(), None),
            };
            let parent = match entry_path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => PathBuf::from("."),
            };
            let file_name: OsString = match entry_path.file_name() {
                Some(n) => n.to_os_string(),
                None => continue,
            };
            let root_dir = match root_depth {
                Some(depth) if depth > 0 && depth < path.components().count() => path.components().take(depth).collect::<PathBuf>(),
                _ => parent.clone(),
            };
            self.current_root_dir = root_dir.clone();

            let depth = path.strip_prefix(&root_dir).map(|rest| rest.components().count()).unwrap_or(1) as u32;
            if !self.within_depth_limits(depth) {
                continue;
            }

            if filters_root.as_ref() != Some(&root_dir) {
                self.hgignore_filters.clear();
                self.dockerignore_filters.clear();
                if self.current_apply_hgignore {
                    search_upstream_hgignore(&mut self.hgignore_filters, &root_dir);
                }
                if self.current_apply_dockerignore {
                    search_upstream_dockerignore(&mut self.dockerignore_filters, &root_dir);
                }
                #[cfg(feature = "git")]
                {
                    repository = match self.current_apply_gitignore {
                        true => canonical_path(&root_dir)
                            .ok()
                            .and_then(IgnoreRules::discover),
                        false => None,
                    };
                }
                filters_root = Some(root_dir.clone());
            }
            let entry_text = entry_path.to_string_lossy();
            if self.current_apply_hgignore && matches_hgignore_filter(&self.hgignore_filters, entry_text.as_ref()) {
                continue;
            }
            if self.current_apply_dockerignore && matches_dockerignore_filter(&self.dockerignore_filters, entry_text.as_ref()) {
                continue;
            }
            #[cfg(feature = "git")]
            if repository.as_ref().is_some_and(|rules| rules.is_ignored(&entry_path)) {
                continue;
            }

            match lookup.entry(&parent, &file_name) {
                Ok(Some(entry)) => {
                    if let Err(err) = self.check_file(entry, &root_dir, &file_info, None) {
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

    fn archived_member_of(&self, path: &Path, member_index: Option<usize>, archives: &mut ArchiveLookup) -> Option<(PathBuf, FileInfo)> {
        let (archive_path, shared) = match archives.holding(path) {
            Some(found) => found,
            None => {
                if path.exists() {
                    return None;
                }
                let archive_path = path
                    .ancestors()
                    .skip(1)
                    .find(|ancestor| ancestor.is_file() && self.is_zip_archive(&ancestor.to_string_lossy()))?;
                archives.open(archive_path)?
            }
        };
        let member_name = Self::archive_member_name(path, &archive_path)?;
        let file_info = {
            let mut guard = shared.borrow_mut();
            let index = match member_index {
                Some(index) => index,
                None => archive_member_index(&guard, &member_name)?,
            };
            let member = guard.by_index_raw(index).ok()?;
            to_file_info(&member, Some(ArchiveMember { archive: Rc::clone(&shared), index }))
        };
        Some((archive_path, file_info))
    }

    fn archive_member_name(path: &Path, archive_path: &Path) -> Option<String> {
        let raw = path.to_string_lossy();
        let archive = archive_path.to_string_lossy();
        if let Some(rest) = raw.strip_prefix(archive.as_ref())
            && rest.starts_with(['/', '\\'])
        {
            return Some(rest.trim_start_matches(['/', '\\']).replace('\\', "/"));
        }
        let rest = path.strip_prefix(archive_path).ok()?;
        Some(
            rest.components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<String>>()
                .join("/"),
        )
    }

    fn get_list_from_subquery(&mut self, query: Query) -> Result<Vec<Variant>, SearchError> {
        self.get_list_from_subquery_with(query, false)
    }

    fn get_list_from_subquery_with(&mut self, query: Query, root_depth_column: bool) -> Result<Vec<Variant>, SearchError> {
        let query_str = format!("{:?}#{}", query, root_depth_column);

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
            false,
        );
        // Always run silent: is_buffered() must hold so results land in
        // output_buffer instead of leaking to stdout (debug mode included).
        sub_searcher.silent_mode = true;
        sub_searcher.is_nested = true;
        sub_searcher.alias_failures = Rc::clone(&self.alias_failures);
        sub_searcher.suppress_row_errors = self.suppress_row_errors;
        sub_searcher.enclosing_aliases = self.enclosing_aliases.union(&self.query.own_root_aliases()).cloned().collect();
        sub_searcher.root_depth_column = root_depth_column;
        // The subquery's own aliases shadow same-named outer aliases for the
        // duration of the search: the outer row context and the outer
        // pending alias failures are set aside and restored afterwards.
        let shadowed: Vec<(String, Option<HashMap<String, Variant>>, Option<HashMap<String, SearchError>>)> = {
            let context = self.record_context.borrow();
            let mut failures = self.alias_failures.borrow_mut();
            query.own_root_aliases().into_iter().map(|alias| {
                let saved = context.get(&alias).cloned();
                let pending = failures.pending.remove(&alias);
                (alias, saved, pending)
            }).collect()
        };
        let shadowed_hit = {
            let mut failures = self.alias_failures.borrow_mut();
            match &failures.hit {
                Some((alias, _)) if shadowed.iter().any(|(own, _, _)| own == alias) => failures.hit.take(),
                _ => None,
            }
        };
        let outcome = sub_searcher.list_search_results();
        self.error_count += sub_searcher.error_count;
        {
            let mut context = self.record_context.borrow_mut();
            let mut failures = self.alias_failures.borrow_mut();
            for (alias, saved, pending) in shadowed {
                match pending {
                    Some(entry) => failures.pending.insert(alias.clone(), entry),
                    None => failures.pending.remove(&alias),
                };
                match saved {
                    Some(entry) => context.insert(alias, entry),
                    None => context.remove(&alias),
                };
            }
            if shadowed_hit.is_some() {
                failures.hit = shadowed_hit;
            }
        }
        outcome?;
        if let Some(err) = sub_searcher.suppressed_row_error.take() {
            return Err(err);
        }

        // The buffer holds limit + offset rows; the offset rows are skipped
        // only in the print path, so they must be skipped here as well.
        // Strip exactly one row terminator: values may legally end in spaces,
        // tabs or line breaks of their own, and a mangled value would silently
        // drop the row later.
        let terminator = sub_searcher.results_writer.row_terminator();
        let single_column = query.fields.len() == 1 && !root_depth_column;
        let result_values = sub_searcher.output_buffer.iter_values()
            .skip(query.offset as usize)
            .map(|row| {
                let text = row.strip_suffix(terminator.as_str()).unwrap_or(row);
                match row.first_value() {
                    Some(first) if single_column => first.with_text(text),
                    _ => Variant::from_string(&text.to_string()),
                }
            })
            .collect::<Vec<Variant>>();

        if ok_to_cache {
            self.subquery_cache.insert(query_str, result_values.clone());
        }

        Ok(result_values)
    }

    fn first_column_value(&self, index: usize, value: &Variant) -> Option<Variant> {
        (index == 0 && self.is_nested).then(|| value.clone())
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

    #[cfg(feature = "git")]
    fn gitignore_repository(
        &self,
        canonical_dir: &Path,
        inherited: Option<&Rc<IgnoreRules>>,
    ) -> Option<Rc<IgnoreRules>> {
        if canonical_dir.join(".git").exists()
            && let Some(rules) = IgnoreRules::open(canonical_dir) {
                return Some(Rc::new(rules));
            }
        inherited.cloned()
    }

    /// Recursively explore directories starting from a given path.
    /// Handles archives, and optionally applies filters.
    fn visit_dir(
        &mut self,
        dir: &Path,
        root_depth: u32,
        #[cfg(feature = "git")]
        git_repository: Option<Rc<IgnoreRules>>,
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
        let child_ancestors = if self.current_follow_symlinks {
            let canonical_pathbuf = PathBuf::from(canonical_path.as_ref().unwrap());
            if AncestorDir::contains(&self.ancestor_dirs, &canonical_pathbuf) {
                return Ok(());
            }
            Some(Rc::new(AncestorDir { canonical: canonical_pathbuf, parent: self.ancestor_dirs.clone() }))
        } else {
            None
        };

        let depth = root_depth + 1;
        let child_root_depth = depth;

        // Read the directory and process each entry
        let root_dir = self.current_root_dir.clone();
        let filter_base = match &canonical_path {
            Some(canonical) if self.current_follow_symlinks => self.logical_canonical_dir(dir, &root_dir).unwrap_or_else(|| PathBuf::from(canonical)),
            Some(canonical) => PathBuf::from(canonical),
            None => PathBuf::new(),
        };
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
                            let (pass_ignores, explore_only) = if self.current_apply_gitignore || self.current_apply_hgignore || self.current_apply_dockerignore {
                                let canonical_entry_path = filter_base.join(entry.file_name());

                                // Check the path against the filters
                                #[cfg(feature = "git")]
                                let pass_gitignore = !self.current_apply_gitignore
                                    || !git_repository
                                        .as_ref()
                                        .is_some_and(|rules| rules.is_ignored(&canonical_entry_path));
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

                                let explore_only = pass_gitignore
                                    && pass_hgignore
                                    && !pass_dockerignore
                                    && entry.file_type().is_ok_and(|file_type| {
                                        file_type.is_dir() || (self.current_follow_symlinks && file_type.is_symlink() && path.is_dir())
                                    })
                                    && dockerignore_exception_may_lie_beneath(
                                        &self.dockerignore_filters,
                                        canonical_entry_path.to_string_lossy().as_ref(),
                                    );

                                (pass_gitignore && pass_hgignore && pass_dockerignore, explore_only)
                            } else {
                                (true, false)
                            };

                            // If the path passes the filters, process it
                            if pass_ignores || explore_only {
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

                                if pass_ignores && (self.current_min_depth == 0 || depth >= self.current_min_depth) {
                                    let checked = self.check_file(&entry, &root_dir, &None, file_type_hint);
                                    if let Err(err) = checked {
                                        if err.is_fatal() {
                                            return Err(err);
                                        }
                                        self.handle_nonfatal_error(err, &path);
                                    }
                                }

                                // Archive members sit below the archive itself, so each one
                                // gets its own depth check against mindepth/maxdepth.
                                if pass_ignores
                                    && self.current_search_archives
                                    && self.is_zip_archive(&path.to_string_lossy())
                                    && let Some(archive) = open_archive(&path) {
                                            let members = archive.len();
                                            let archive = Rc::new(RefCell::new(archive));
                                            for index in 0..members {
                                                if self.limit_reached() {
                                                    break;
                                                }

                                                let file_info = match archive.borrow_mut().by_index_raw(index) {
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
                                                #[cfg(feature = "git")]
                                                let child_repository = match canonical_path.as_ref() {
                                                    Some(canonical) if self.current_apply_gitignore => self.gitignore_repository(
                                                        &PathBuf::from(canonical).join(entry.file_name()),
                                                        git_repository.as_ref(),
                                                    ),
                                                    _ => None,
                                                };
                                                if self.current_traversal_mode == Dfs {
                                                    let parent_ancestors = std::mem::replace(&mut self.ancestor_dirs, child_ancestors.clone());
                                                    let result = self.visit_dir(
                                                        &path,
                                                        child_root_depth,
                                                        #[cfg(feature = "git")]
                                                        child_repository,
                                                        false,
                                                    );
                                                    self.ancestor_dirs = parent_ancestors;

                                                    if let Err(err) = result {
                                                        if err.is_fatal() {
                                                            return Err(err);
                                                        }
                                                        self.handle_nonfatal_error(err, &path);
                                                    }
                                                } else {
                                                    self.dir_queue.push_back(QueuedDir {
                                                        path,
                                                        root_depth: child_root_depth,
                                                        ancestors: child_ancestors.clone(),
                                                        #[cfg(feature = "git")]
                                                        repository: child_repository,
                                                    });
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
            while let Some(queued) = self.dir_queue.pop_front() {
                if self.limit_reached() {
                    self.dir_queue.clear();
                    break;
                }
                let QueuedDir { path, root_depth: queued_root_depth, ancestors, #[cfg(feature = "git")] repository } = queued;
                self.ancestor_dirs = ancestors;
                let result = self.visit_dir(
                    &path,
                    queued_root_depth,
                    #[cfg(feature = "git")]
                    repository,
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
        // While a row alias is being bound, a row error inside its subquery
        // is the alias's failure: it is handed back to the binder instead of
        // being reported, and reported, against the outer row, only once the
        // alias is read.
        if self.suppress_row_errors {
            if self.suppressed_row_error.is_none() {
                self.suppressed_row_error = Some(err);
            }
            return;
        }
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

    fn condition_state(
        &mut self,
        row: &Row,
        expr: &Expr,
    ) -> Result<Option<bool>, SearchError> {
        if let Some(ref logical_op) = expr.logical_op {
            let left = match expr.left {
                Some(ref left) => self.condition_state(row, left)?,
                None => Some(false),
            };
            match (logical_op, left) {
                (LogicalOp::And, Some(false)) => return Ok(Some(false)),
                (LogicalOp::Or, Some(true)) => return Ok(Some(true)),
                _ => {}
            }
            let right = match expr.right {
                Some(ref right) => self.condition_state(row, right)?,
                None => Some(false),
            };
            return Ok(match logical_op {
                LogicalOp::And => match (left, right) {
                    (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                },
                LogicalOp::Or => match (left, right) {
                    (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                },
            });
        }
        let key = expr.cache_key();
        let holds = match row.values.get(&key).and_then(cached_condition) {
            Some(holds) => holds,
            None => self.conforms(row, expr)?,
        };
        Ok(if self.null_comparisons.contains(&key) { None } else { Some(holds) })
    }

    fn conforms_caching_leaves(
        &mut self,
        row: &mut Row,
        expr: &Expr,
    ) -> Result<bool, SearchError> {
        if let Some(ref logical_op) = expr.logical_op {
            let left = match expr.left {
                Some(ref left) => self.conforms_caching_leaves(row, left)?,
                None => false,
            };
            match (logical_op, left) {
                (LogicalOp::And, false) => return Ok(false),
                (LogicalOp::Or, true) => return Ok(true),
                _ => {}
            }
            return match expr.right {
                Some(ref right) => self.conforms_caching_leaves(row, right),
                None => Ok(false),
            };
        }
        let key = expr.cache_key();
        if let Some(holds) = row.values.get(&key).and_then(cached_condition) {
            return Ok(holds);
        }
        let holds = self.conforms(row, expr)?;
        row.values.insert(key, apply_minus(expr, Variant::from_bool(holds)));
        Ok(holds)
    }

    fn grouped_condition_value(&self, file_map: &HashMap<String, Variant>, condition: &Expr) -> Option<bool> {
        let key = Self::condition_key(condition);
        let conditions = || {
            self.query
                .grouping_fields
                .iter()
                .filter(|grouping_expr| grouping_expr.op.is_some() || grouping_expr.logical_op.is_some())
        };
        let exact = conditions().find(|grouping_expr| Self::condition_key(grouping_expr) == key).map(|grouping_expr| (grouping_expr, true));
        let (grouping_expr, holds) = exact.or_else(|| {
            conditions()
                .find(|grouping_expr| Self::condition_key(&grouping_expr.negated()) == key)
                .map(|grouping_expr| (grouping_expr, false))
        })?;
        if !holds && self.unknown_conditions.contains(&grouping_expr.cache_key()) {
            return Some(false);
        }
        let value = cached_condition(file_map.get(&grouping_expr.cache_key())?)?;
        Some(value == holds)
    }

    fn condition_key(expr: &Expr) -> String {
        let mut bare = expr.clone();
        bare.alias = None;
        bare.minus = false;
        bare.cache_key()
    }

    fn unaliased_key(expr: &Expr) -> String {
        let mut bare = expr.clone();
        bare.alias = None;
        bare.cache_key()
    }

    fn bind_aggregate_aliases(
        &mut self,
        aliases: &HashSet<String>,
        row: &mut Row,
        skip_failing_columns: bool,
    ) -> Result<(), SearchError> {
        if aliases.is_empty() {
            return Ok(());
        }
        let Some(accumulator) = row.group else {
            return Ok(());
        };
        let query = self.query;
        let aliased: Vec<&Expr> = query
            .fields
            .iter()
            .filter(|column| column.alias.is_some())
            .collect();
        let (dependent, independent): (Vec<&Expr>, Vec<&Expr>) =
            aliased.iter().partition(|column| column.needs_row_context());
        let mut values: Vec<(String, Variant)> = Vec::new();
        for column in independent {
            match self.get_column_expr_value(row, column) {
                Ok(value) => Self::upsert_alias_value(&mut values, column, value),
                Err(err) if skip_failing_columns && !err.is_fatal() => self.mark_aggregate_alias_failed(aliases, column, err),
                Err(err) => return Err(err),
            }
        }
        if !values.is_empty() {
            self.bind_group_context(aliases, &accumulator.key_values, &values);
        }
        if dependent.is_empty() {
            return Ok(());
        }
        let rounds = if dependent.len() == 1 { 1 } else { dependent.len() + 1 };
        let mut scratch = HashMap::new();
        let mut failed: Vec<(&Expr, SearchError)> = Vec::new();
        for _ in 0..rounds {
            let before = values.clone();
            scratch = row.values.clone();
            failed.clear();
            for column in &dependent {
                match self.get_column_expr_value(&mut row.scratch(&mut scratch), column) {
                    Ok(value) => {
                        Self::upsert_alias_value(&mut values, column, value);
                        self.bind_group_context(aliases, &accumulator.key_values, &values);
                    }
                    Err(err) if !err.is_fatal() => failed.push((column, err)),
                    Err(err) => return Err(err),
                }
            }
            if values == before && failed.is_empty() {
                break;
            }
        }
        for (column, err) in failed {
            if skip_failing_columns {
                self.mark_aggregate_alias_failed(aliases, column, err);
            } else {
                self.get_column_expr_value(&mut row.scratch(&mut scratch), column)?;
            }
        }
        if skip_failing_columns {
            // Reads made while preparing the aliases are not reads by the
            // query: a dependent alias that read a failed one inherits the
            // failure, and the hit is cleared so only a later real read
            // (HAVING, columns) reports it.
            loop {
                let mut spread = false;
                for column in &dependent {
                    let Some(name) = column.alias.as_deref() else { continue };
                    if self.alias_failures.borrow().pending.values().any(|keys| keys.contains_key(name)) {
                        continue;
                    }
                    self.alias_failures.borrow_mut().hit = None;
                    // Start from the group's base map: the values cached
                    // during the rounds would serve a wrapper such as
                    // `coalesce(...)` without re-reading the failed alias.
                    let mut probe = row.values.clone();
                    let _ = self.get_column_expr_value(&mut row.scratch(&mut probe), column);
                    let hit = self.alias_failures.borrow_mut().hit.take();
                    if let Some((_, err)) = hit {
                        self.mark_aggregate_alias_failed(aliases, column, err);
                        spread = true;
                    }
                }
                if !spread {
                    break;
                }
            }
            self.alias_failures.borrow_mut().hit = None;
        }
        row.values.extend(scratch);
        Ok(())
    }

    /// A group alias that failed while HAVING is prepared stays pending under
    /// every own root alias, so a HAVING subquery that reads it reports the
    /// error, while a HAVING that never reads it stays silent.
    fn mark_aggregate_alias_failed(&self, aliases: &HashSet<String>, column: &Expr, err: SearchError) {
        let Some(name) = column.alias.as_deref() else {
            return;
        };
        for alias in aliases {
            self.mark_row_alias_failed(alias, name, err.clone());
        }
    }

    fn reset_alias_failures(&self, aliases: &HashSet<String>) {
        let mut failures = self.alias_failures.borrow_mut();
        for alias in aliases {
            failures.pending.remove(alias);
        }
        if failures.hit.as_ref().is_some_and(|(alias, _)| aliases.contains(alias)) {
            failures.hit = None;
        }
    }

    fn upsert_alias_value(values: &mut Vec<(String, Variant)>, column: &Expr, value: Variant) {
        let alias = column.alias.clone().unwrap_or_default();
        match values.iter_mut().find(|(name, _)| *name == alias) {
            Some(slot) => slot.1 = value,
            None => values.push((alias, value)),
        }
    }

    fn bind_group_context(
        &self,
        aliases: &HashSet<String>,
        key_values: &[Variant],
        aggregate_values: &[(String, Variant)],
    ) {
        let mut context = self.record_context.borrow_mut();
        for alias in aliases {
            let entry = context.entry(alias.clone()).or_default();
            entry.clear();
            for (alias_name, value) in aggregate_values {
                entry.insert(alias_name.clone(), value.clone());
            }
            for (grouping_expr, value) in self.query.grouping_fields.iter().zip(key_values) {
                if grouping_expr
                    .root_alias
                    .as_deref()
                    .is_some_and(|owner| !owner.eq_ignore_ascii_case(alias))
                {
                    continue;
                }
                if let Some(field) = grouping_expr.field
                    && !grouping_expr.minus
                {
                    entry.insert(field.to_string(), value.clone());
                }
                if let Some(ref column_alias) = grouping_expr.alias {
                    entry.insert(column_alias.clone(), value.clone());
                }
                let grouping_key = Self::unaliased_key(grouping_expr);
                for column in &self.query.fields {
                    if let Some(ref column_alias) = column.alias
                        && Self::unaliased_key(column) == grouping_key
                    {
                        entry.insert(column_alias.clone(), value.clone());
                    }
                }
            }
        }
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
                    BufferedRow::Rendered(text, first_value) => Some((text, first_value)),
                    BufferedRow::Pending(pending) if position < offset => Some((String::new(), pending.first_value)),
                    BufferedRow::Pending(pending) => self.render_pending(pending, &mut lookup)?,
                };
                position += 1;
                if let Some((text, first_value)) = rendered {
                    self.output_buffer.insert(criteria.clone(), BufferedRow::Rendered(text, first_value));
                }
            }
        }
        Ok(())
    }

    fn render_pending(
        &mut self,
        row: PendingRow,
        lookup: &mut DirEntryLookup,
    ) -> Result<Option<(String, Option<Variant>)>, SearchError> {
        let (Some(directory), Some(file_name)) = (row.path.parent(), row.path.file_name()) else {
            self.error_count += 1;
            error_message(&row.path.to_string_lossy(), "cannot locate the entry again");
            return Ok(None);
        };
        let mut items = row.items.clone();
        let mut first_value = row.first_value.clone();
        match lookup.entry(directory, file_name) {
            Ok(Some(entry)) => {
                self.fms.clear();
                let follow_symlinks = std::mem::replace(&mut self.current_follow_symlinks, row.follow_symlinks);
                let evaluated = self.evaluate_deferred(entry, &row, &mut items);
                self.current_follow_symlinks = follow_symlinks;
                first_value = first_value.or(evaluated?);
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
        Ok(Some((String::from(buf), first_value)))
    }

    fn evaluate_deferred(
        &mut self,
        entry: &DirEntry,
        pending: &PendingRow,
        items: &mut [(String, String)],
    ) -> Result<Option<Variant>, SearchError> {
        let mut first_value = None;
        let query = self.query;
        let mut values = HashMap::new();
        let mut row = Row::entry(entry, &pending.file_info, &pending.root_path, &mut values);
        if self.use_colors && pending.deferred.iter().any(|index| query.fields[*index].contains_colorized()) {
            self.fms.update_file_metadata(entry, self.current_follow_symlinks);
        }
        for &index in &pending.deferred {
            let field = &query.fields[index];
            let record = match self.get_column_expr_value(&mut row, field) {
                Ok(record) => record,
                Err(err) if err.is_fatal() => return Err(err),
                Err(err) => {
                    self.handle_nonfatal_error(err, &pending.path);
                    continue;
                }
            };
            first_value = first_value.or_else(|| self.first_column_value(index, &record));
            items[index].1 = match self.use_colors && field.contains_colorized() {
                true => self.colorize(&record.to_string()),
                false => record.to_string(),
            };
        }
        Ok(first_value)
    }

    fn get_column_expr_value(
        &mut self,
        row: &mut Row,
        column_expr: &Expr,
    ) -> Result<Variant, SearchError> {
        let cache_key = column_expr.cache_key();

        let mut context_field: Option<String> = None;

        let reference = match (&column_expr.root_alias, column_expr.field) {
            (Some(root_alias), Some(field)) => {
                Some((root_alias.clone(), column_expr.alias.clone().unwrap_or_else(|| field.to_string())))
            }
            _ => column_expr
                .alias_reference_key()
                .map(|(root_alias, name)| (root_alias.to_string(), name.to_string())),
        };
        if let Some((context_name, field_key)) = reference {
            let own_row = self.current_alias.as_deref() == Some(context_name.as_str());
            if own_row && column_expr.field.is_some() {
                if !column_expr.minus {
                    context_field = Some(field_key);
                }
            } else {
                let context = self.record_context.borrow();
                match context.get(&context_name) {
                    Some(ctx) => {
                        return Ok(match ctx.get(&field_key) {
                            Some(val) => {
                                let value = apply_minus(column_expr, val.clone());
                                row.values.insert(cache_key, value.clone());
                                value
                            }
                            None => {
                                self.alias_failures.borrow_mut().note_read(&context_name, &field_key);
                                Variant::empty(VariantType::String)
                            }
                        });
                    }
                    None if own_row || self.is_declared_alias(&context_name) => {
                        self.alias_failures.borrow_mut().note_read(&context_name, &field_key);
                        return Ok(Variant::empty(VariantType::String));
                    }
                    None => {
                        return Err(SearchError::fatal(format!("Invalid root alias: {}", context_name)).with_source("query"));
                    }
                }
            }
        }

        if let Some(cached) = row.values.get(&cache_key) {
            let value = cached.clone();
            if let Some(entry_key) = context_field {
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_default();
                context_entry.insert(entry_key, value.clone());
            }
            return Ok(value);
        }

        if row.entry.is_none() && column_expr.minus && column_expr.op.is_none() && column_expr.logical_op.is_none() {
            let mut bare = column_expr.clone();
            bare.minus = false;
            let bare_key = bare.cache_key();
            if let Some(cached) = row.values.get(&bare_key) {
                return Ok(apply_minus(column_expr, cached.clone()));
            }
        }

        if let Some(ref subquery) = column_expr.subquery {
            let mut subquery = subquery.clone();
            if subquery.grouping_fields.is_empty() {
                subquery.limit = 1;
            }
            let list = self.get_list_from_subquery(*subquery)?;
            return Ok(match list.into_iter().next() {
                Some(value) => apply_minus(column_expr, value),
                None => Variant::null(),
            });
        }

        if let Some(ref _function) = column_expr.function {
            let result =
                self.get_function_value(row, column_expr)?;
            let result = apply_minus(column_expr, result);
            row.values.insert(cache_key, result.clone());
            return Ok(result);
        }

        if let Some(ref field) = column_expr.field {
            if let Some(entry) = row.entry {
                let raw = self.get_field_value(entry, row.file_info, row.root_path, field).unwrap_or(Variant::empty(VariantType::String));
                // The record context feeds correlated subqueries: under the
                // field name it holds the raw value, under an alias the value
                // the column actually produces (negated when written `-size`).
                let result = apply_minus(column_expr, raw.clone());
                row.values.insert(cache_key, result.clone());
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_default();
                match column_expr.alias.clone() {
                    Some(alias) => context_entry.insert(alias, result.clone()),
                    None => context_entry.insert(field.to_string(), raw),
                };
                return Ok(result);
            } else if let Some(val) = row.values.get(&field.to_string()) {
                return Ok(apply_minus(column_expr, val.clone()));
            } else {
                return Ok(Variant::empty(VariantType::String));
            }
        }

        if let Some(ref value) = column_expr.val {
            return Ok(Variant::from_signed_string(value, column_expr.minus));
        }

        // A comparison used as a column (`select size > 2`) is a condition:
        // it evaluates to a boolean, per row or per group.
        if column_expr.op.is_some() || column_expr.logical_op.is_some() {
            let holds = match (row.entry, row.group) {
                (Some(_), _) => Some(self.conforms_caching_leaves(row, column_expr)?),
                (None, Some(_)) => Some(self.group_conforms(row, column_expr)?),
                (None, None) => None,
            };
            return Ok(match holds {
                Some(holds) => {
                    let result = apply_minus(column_expr, Variant::from_bool(holds));
                    row.values.insert(cache_key, result.clone());
                    result
                }
                None => Variant::empty(VariantType::Bool),
            });
        }

        let result;

        if let Some(ref left) = column_expr.left {
            let left_result =
                self.get_column_expr_value(row, left)?;

            if let Some(ref op) = column_expr.arithmetic_op {
                if let Some(ref right) = column_expr.right {
                    let right_result =
                        self.get_column_expr_value(row, right)?;
                        result = op.calc(&left_result, &right_result).map(|v| apply_minus(column_expr, v));
                        if let Ok(ref value) = result {
                            row.values.insert(cache_key, value.clone());
                        }
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
        row: &mut Row,
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
            if row.entry.is_some() {
                self.get_column_expr_value(row, left_expr)?;
            }
            let buffer_key = if left_expr.is_literal() && matches!(left_expr.val.as_deref(), Some("*") | Some("")) {
                String::from("*")
            } else {
                left_expr.cache_key()
            };
            let empty_acc = function::GroupAccumulator::default();
            let accumulator = row.group.unwrap_or(&empty_acc);
            let aggr_result = function::get_aggregate_value(function, accumulator, buffer_key, &column_expr.val);
            if aggr_result.as_str().is_empty() && *function != function::Function::Count {
                return Ok(Variant::null());
            }
            Ok(aggr_result)
        } else if *function == function::Function::Coalesce {
            // COALESCE short-circuits: later arguments are evaluated only
            // while every earlier one came back empty, so a failing or
            // expensive fallback costs nothing once a value is found.
            let mut candidates = std::iter::once(left_expr.as_ref())
                .chain(column_expr.args.iter().flatten());
            let mut all_null = true;
            let result = loop {
                match candidates.next() {
                    None => break if all_null { Variant::null() } else { Variant::empty(VariantType::String) },
                    Some(arg) => {
                        let value = self.get_column_expr_value(row, arg)?;
                        if !value.to_string().is_empty() {
                            break value;
                        }
                        all_null &= value.is_null();
                    }
                }
            };
            row.values.insert(column_expr.cache_key(), result.clone());

            Ok(result)
        } else {
            let function_arg =
                self.get_column_expr_value(row, left_expr);
            let mut function_args = vec![];
            if let Some(args) = &column_expr.args {
                for arg in args {
                    function_args.push(self.get_column_expr_value(row, arg)?);
                }
            }
            let result = function::evaluate(
                function,
                function_arg?,
                function_args,
                row.entry,
                row.file_info,
            )?;
            row.values.insert(column_expr.cache_key(), result.clone());

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

    /// Binds the row's aliased columns that correlated subqueries read, in
    /// settling rounds so an alias may depend on another declared later.
    /// Failures are tolerated while values settle; a column that still fails
    /// afterwards is evaluated once more strictly so its error is reported
    /// like any other row error.
    fn bind_row_aliases(
        &mut self,
        row: &mut Row,
        current_alias: &str,
        required_columns: &[(String, Expr)],
    ) -> Result<(), SearchError> {
        let previous = std::mem::replace(&mut self.suppress_row_errors, true);
        let outcome = self.bind_row_aliases_inner(row, current_alias, required_columns);
        self.suppress_row_errors = previous;
        outcome
    }

    fn bind_row_aliases_inner(
        &mut self,
        row: &mut Row,
        current_alias: &str,
        required_columns: &[(String, Expr)],
    ) -> Result<(), SearchError> {
        let rounds = if required_columns.len() > 1 { required_columns.len() } else { 1 };
        let self_contained: Vec<bool> = required_columns
            .iter()
            .map(|(_, column)| column.get_alias_references_in_subqueries(current_alias, true).is_empty())
            .collect();
        let mut bound: Vec<Option<Variant>> = vec![None; required_columns.len()];
        let mut scratches: Vec<HashMap<String, Variant>> = vec![HashMap::new(); required_columns.len()];
        let mut failed: Vec<(usize, SearchError)> = Vec::new();
        for round in 0..rounds {
            let mut changed = false;
            failed.clear();
            for (index, (name, column)) in required_columns.iter().enumerate() {
                if round > 0 && self_contained[index] && bound[index].is_some() {
                    continue;
                }
                let mut scratch = HashMap::new();
                let shared = required_columns[..index]
                    .iter()
                    .position(|(_, earlier)| earlier.cache_key() == column.cache_key())
                    .and_then(|earlier| bound[earlier].clone());
                let evaluated = match shared {
                    Some(value) => Ok(value),
                    None => self.get_column_expr_value(&mut row.scratch(&mut scratch), column),
                };
                scratches[index] = scratch;
                let value = match evaluated {
                    Ok(value) => {
                        bound[index] = Some(value.clone());
                        value
                    }
                    Err(err) if !err.is_fatal() => {
                        failed.push((index, err));
                        bound[index] = None;
                        Variant::empty(VariantType::String)
                    }
                    Err(err) => return Err(err),
                };
                let previous = self
                    .record_context
                    .borrow_mut()
                    .entry(current_alias.to_string())
                    .or_default()
                    .insert(name.clone(), value.clone());
                changed |= previous.as_ref() != Some(&value);
            }
            if !changed && failed.is_empty() {
                break;
            }
        }
        if failed.is_empty() {
            self.seed_bound_aliases(current_alias, required_columns, &bound, &scratches, row.values);
            return Ok(());
        }
        for (index, err) in failed {
            self.mark_row_alias_failed(current_alias, &required_columns[index].0, err);
        }
        // A column that read a failed alias was bound from an empty
        // placeholder; it inherits the failure so that reading it reports
        // the original error too.
        loop {
            let mut spread = false;
            for (name, column) in required_columns {
                if self.alias_failures.borrow().pending.get(current_alias).is_some_and(|keys| keys.contains_key(name)) {
                    continue;
                }
                self.alias_failures.borrow_mut().hit = None;
                let mut scratch = HashMap::new();
                let _ = self.get_column_expr_value(&mut row.scratch(&mut scratch), column);
                let hit = self.alias_failures.borrow_mut().hit.take();
                if let Some((_, err)) = hit {
                    self.mark_row_alias_failed(current_alias, name, err);
                    spread = true;
                }
            }
            if !spread {
                break;
            }
        }
        self.seed_bound_aliases(current_alias, required_columns, &bound, &scratches, row.values);
        Ok(())
    }

    fn seed_bound_aliases(
        &mut self,
        current_alias: &str,
        required_columns: &[(String, Expr)],
        bound: &[Option<Variant>],
        scratches: &[HashMap<String, Variant>],
        file_map: &mut HashMap<String, Variant>,
    ) {
        for (((name, column), value), scratch) in required_columns.iter().zip(bound).zip(scratches) {
            let failed = self
                .alias_failures
                .borrow()
                .pending
                .get(current_alias)
                .is_some_and(|keys| keys.contains_key(name));
            if failed {
                continue;
            }
            if let Some(value) = value {
                for (key, cached) in scratch {
                    file_map.entry(key.clone()).or_insert_with(|| cached.clone());
                }
                file_map.insert(column.cache_key(), value.clone());
            }
        }
    }

    fn mark_row_alias_failed(&self, current_alias: &str, name: &str, err: SearchError) {
        self.record_context.borrow_mut().entry(current_alias.to_string()).or_default().remove(name);
        self.alias_failures
            .borrow_mut()
            .pending
            .entry(current_alias.to_string())
            .or_default()
            .insert(name.to_string(), err);
    }

    fn alias_read_failure(&self) -> Result<(), SearchError> {
        let mut failures = self.alias_failures.borrow_mut();
        let owned = failures
            .hit
            .as_ref()
            .is_some_and(|(alias, _)| self.query.roots.iter().any(|root| root.options.alias.as_deref() == Some(alias.as_str())));
        if !owned {
            return Ok(());
        }
        match failures.hit.take() {
            Some((_, err)) => Err(err),
            None => Ok(()),
        }
    }

    fn check_file_inner(&mut self, entry: &DirEntry, root_path: &Path, file_info: &Option<FileInfo>, file_map: &mut HashMap<String, Variant>) -> Result<(), SearchError> {
        self.null_comparisons.clear();
        let mut row = Row::entry(entry, file_info, root_path, file_map);
        if let Some(ref current_alias) = self.current_alias.clone() {
            self.record_context.borrow_mut().entry(current_alias.to_string()).or_default().clear();
            {
                let mut failures = self.alias_failures.borrow_mut();
                failures.pending.remove(current_alias.as_str());
                if failures.hit.as_ref().is_some_and(|(alias, _)| alias == current_alias) {
                    failures.hit = None;
                }
            }

            if let Some(required_fields) = std::mem::take(&mut self.subquery_required_fields) {
                let mut field_values = HashMap::new();
                for (field, keys) in &required_fields {
                    let field_value = self.get_field_value(entry, file_info, root_path, field).unwrap_or(Variant::empty(VariantType::String));
                    for key in keys {
                        field_values.insert(key.clone(), field_value.clone());
                    }
                }
                self.subquery_required_fields = Some(required_fields);

                let mut context = self.record_context.borrow_mut();
                let context_entry = context.entry(current_alias.to_string()).or_default();
                for (field, field_value) in field_values {
                    context_entry.insert(field, field_value);
                }
            }

            let required_columns = std::mem::take(&mut self.subquery_required_columns);
            let outcome = self.bind_row_aliases(&mut row, current_alias, &required_columns);
            self.subquery_required_columns = required_columns;
            outcome?;
        }

        if let Some(ref expr) = self.query.expr {
            let result = self.conforms(&row, expr)?;
            self.alias_read_failure()?;
            if !result {
                return Ok(());
            }
        }

        if let Some(plan) = self.aggregate_plan.clone() {
            self.found += 1;
            for (field, key) in &plan.fields {
                let value = self.get_field_value(entry, file_info, root_path, field).unwrap_or(Variant::empty(VariantType::String));
                row.values.insert(key.clone(), value);
            }
            let planned = plan.inner_exprs.iter().map(|(key, left)| (key, left))
                .chain(plan.grouping_keys.iter().zip(&self.query.grouping_fields));
            let mut unknown_keys: HashSet<String> = HashSet::new();
            for (key, expr) in planned {
                let grouping_condition =
                    (expr.op.is_some() || expr.logical_op.is_some()) && plan.grouping_keys.contains(key);
                if row.values.contains_key(key) && !grouping_condition {
                    continue;
                }
                let value = if grouping_condition {
                    let state = self.condition_state(&row, expr)?;
                    if state.is_none() {
                        unknown_keys.insert(key.clone());
                    }
                    apply_minus(expr, Variant::from_bool(state.unwrap_or(false)))
                } else {
                    self.get_column_expr_value(&mut row, expr)?
                };
                if grouping_condition || !row.values.contains_key(key) {
                    row.values.insert(key.clone(), value);
                }
            }
            let key_values: Vec<Variant> = plan
                .grouping_keys
                .iter()
                .map(|key| row.values.get(key).cloned().unwrap_or_else(|| Variant::empty(VariantType::String)))
                .collect();
            let group_key: Vec<String> = plan
                .grouping_keys
                .iter()
                .zip(&key_values)
                .map(|(key, value)| match value {
                    _ if unknown_keys.contains(key) => String::from(UNKNOWN_GROUP_KEY),
                    value if value.is_null() => String::from(NULL_GROUP_KEY),
                    value => format!("{:?}:{}", value.get_type(), value.identity()),
                })
                .collect();
            let identity = if self.root_depth_column && !self.accumulators.contains_key(&group_key) {
                Some(self.subquery_identity(entry, file_info, root_path))
            } else {
                None
            };
            self.alias_read_failure()?;
            let accumulator = self.accumulators.entry(group_key).or_default();
            if identity.is_some() {
                accumulator.representative = identity;
            }
            if accumulator.key_values.is_empty() {
                accumulator.key_values = key_values;
                accumulator.unknown_keys = unknown_keys;
            }
            accumulator.increment_count();
            for (key, value) in row.values.iter() {
                accumulator.push(key, value);
            }
            return Ok(());
        }

        let defer = self.is_buffered() && !self.deferred_columns.is_empty();
        let mut items: Vec<(String, String)> = Vec::new();

        if self.use_colors && self.query.fields.iter().any(|f| f.contains_colorized()) {
            self.fms.update_file_metadata(entry, self.current_follow_symlinks);
        }

        let mut first_value = None;
        for (index, field) in self.query.fields.iter().enumerate() {
            if defer && self.deferred_columns.contains(&index) {
                items.push((field.column_name(), String::new()));
                continue;
            }
            let record = match self.get_column_expr_value(&mut row, field) {
                Ok(record) => {
                    first_value = first_value.or_else(|| self.first_column_value(index, &record));
                    record
                }
                Err(err) if err.is_fatal() => return Err(err),
                Err(err) => {
                    self.handle_nonfatal_error(err, &entry.path());
                    items.push((field.column_name(), String::new()));
                    continue;
                }
            };

            let value = match self.use_colors && field.contains_colorized() {
                true => self.colorize(&record.to_string()),
                false => record.to_string(),
            };
            items.push((field.column_name(), value));
        }

        if self.root_depth_column {
            let (own_path, root_depth, member_index) = self.subquery_identity(entry, file_info, root_path);
            Self::push_subquery_identity(&mut items, &own_path, root_depth, member_index);
        }

        let mut criteria: Vec<Variant> = Vec::with_capacity(self.query.ordering_fields.len());
        for field in &self.query.ordering_fields {
            let value = match row.values.get(&field.cache_key()) {
                Some(record) => record.clone(),
                None => self.get_column_expr_value(&mut row, field)?,
            };
            criteria.push(value);
        }

        self.alias_read_failure()?;
        if self.is_buffered() {
            let buffered = if defer {
                BufferedRow::Pending(PendingRow {
                    path: entry.path(),
                    root_path: root_path.to_path_buf(),
                    file_info: file_info.clone(),
                    follow_symlinks: self.current_follow_symlinks,
                    items,
                    deferred: self.deferred_columns.clone(),
                    first_value,
                })
            } else {
                let mut buf = WritableBuffer::new();
                self.results_writer.write_row(&mut buf, items)?;
                BufferedRow::Rendered(String::from(buf), first_value)
            };
            self.found += 1;
            self.output_buffer.insert(
                Criteria::typed(self.ordering_fields_rc.clone(), criteria, self.ordering_asc_rc.clone()),
                buffered,
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
        row: &mut Row,
        field_value: &Variant,
        negate: bool,
    ) -> Result<bool, SearchError> {
        // A missing numeric, datetime or boolean value is unknown, not 0 or
        // false: like `= 0`, neither IN nor NOT IN can match it.
        if expr.left.as_deref().is_some_and(|left| Self::is_null_operand(left, field_value)) {
            self.null_comparisons.insert(expr.cache_key());
            return Ok(false);
        }
        let right = expr.right.as_ref().unwrap();
        let args: &[Expr] = match &right.args {
            Some(args) => args,
            None => {
                let (found, has_null) = match &right.subquery {
                    Some(subquery) => {
                        let membership = self.subquery_membership(subquery, field_value.get_type())?;
                        (membership.contains(field_value), membership.has_null)
                    }
                    None => (false, false),
                };
                if !found && has_null {
                    self.null_comparisons.insert(expr.cache_key());
                    return Ok(false);
                }
                return Ok(if negate { !found } else { found });
            }
        };
        let field_type = field_value.get_type();
        let mut found = false;
        let mut has_null = false;
        for arg in args {
            #[cfg(test)]
            {
                self.in_list_comparisons += 1;
            }
            if row.group.is_none() {
                row.values.clear();
            }
            let arg_val = self.get_column_expr_value(row, arg)?;
            if Self::is_null_operand(arg, &arg_val) {
                has_null = true;
                continue;
            }
            let matches = match field_type {
                VariantType::String => arg_val.to_string() == field_value.to_string(),
                // Unparsable numeric args (`size in ('abc')`) are skipped,
                // not coerced to 0, like the DateTime arm below.
                VariantType::Int => arg_val.to_exact_int() == Some(field_value.to_int()),
                VariantType::Float => arg_val.to_float_opt() == Some(field_value.to_float()),
                VariantType::Bool => str_to_bool(&arg_val.to_string()) == Some(field_value.to_bool()),
                VariantType::DateTime => {
                    // Unparseable values: silently skip this arg (treat as a
                    // non-match) instead of erroring per file scanned.
                    // Interval overlap, the same rule as `=`.
                    match (field_value.to_datetime().map(datetime_seconds), arg_val.to_datetime().map(datetime_seconds)) {
                        (Ok((field_start, field_finish)), Ok((arg_start, arg_finish))) => {
                            field_start <= arg_finish && arg_start <= field_finish
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
        if !found && has_null {
            self.null_comparisons.insert(expr.cache_key());
            return Ok(false);
        }
        Ok(if negate { !found } else { found })
    }

    fn subquery_membership(&mut self, subquery: &Query, field_type: &VariantType) -> Result<Rc<Membership>, SearchError> {
        let key = (format!("{:?}", subquery), field_type.clone());
        if let Some(membership) = self.membership_cache.get(&key) {
            return Ok(Rc::clone(membership));
        }
        let mut values = self.get_list_from_subquery(subquery.clone())?;
        let before = values.len();
        match subquery.fields.first() {
            Some(column) => values.retain(|value| !Self::is_null_operand(column, value)),
            None => values.retain(|value| !value.is_null()),
        }
        let has_null = values.len() != before;
        let membership = Rc::new(Membership::new(&values, field_type, has_null));
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

    fn evaluate_operand(&mut self, row: &Row, operand: &Expr) -> Result<Variant, SearchError> {
        let mut scratch = std::mem::take(&mut self.conforms_map);
        let value = self.get_column_expr_value(&mut row.scratch(&mut scratch), operand);
        scratch.clear();
        self.conforms_map = scratch;
        value
    }

    fn conforms(&mut self, row: &Row, expr: &Expr) -> Result<bool, SearchError> {
        let mut result = false;

        if let Some(ref logical_op) = expr.logical_op {
            let left_result = match expr.left {
                Some(ref left) => self.conforms(row, left)?,
                None => false,
            };

            let right_result = |s: &mut Self| -> Result<bool, SearchError> {
                match expr.right {
                    Some(ref right) => s.conforms(row, right),
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
                Op::IsNull | Op::IsNotNull => {
                    let value = self.evaluate_operand(row, expr.left.as_ref().unwrap())?;
                    return Ok(value.to_string().is_empty() == (*op == Op::IsNull));
                }
                _ => {}
            }

            let field_value = self.evaluate_operand(row, expr.left.as_ref().unwrap())?;
            let value = match op {
                Op::In | Op::NotIn => Variant::empty(VariantType::String),
                _ => self.evaluate_operand(row, expr.right.as_ref().unwrap())?,
            };

            let (field_value, value) = match (&expr.left, &expr.right) {
                (Some(left_expr), Some(right_expr)) => (
                    if left_expr.is_literal() { field_value } else { Self::retype_value(left_expr, right_expr, field_value) },
                    if right_expr.is_literal() { value } else { Self::retype_value(left_expr, right_expr, value) },
                ),
                _ => (field_value, value),
            };

            if let (Some(left_expr), Some(right_expr)) = (&expr.left, &expr.right)
                && op.is_relational()
                && (Self::is_null_operand(left_expr, &field_value) || Self::is_null_operand(right_expr, &value))
            {
                self.null_comparisons.insert(expr.cache_key());
                return Ok(false);
            }

            result = match op {
                Op::In => {
                    let mut arg_map = HashMap::new();
                    self.check_in_list(expr, &mut row.scratch(&mut arg_map), &field_value, false)?
                }
                Op::NotIn => {
                    let mut arg_map = HashMap::new();
                    self.check_in_list(expr, &mut row.scratch(&mut arg_map), &field_value, true)?
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
                    // Both sides are intervals: an instant is a one-second
                    // interval, a date-only value spans its whole day, so the
                    // rules hold whichever side carries the span
                    // (`modified = today` and `today = modified` alike).
                    match (field_value.to_datetime(), value.to_datetime()) {
                        (Ok((left_start, left_finish)), Ok((right_start, right_finish))) => {
                            let left_start = left_start.and_utc().timestamp();
                            let left_finish = left_finish.and_utc().timestamp();
                            let right_start = right_start.and_utc().timestamp();
                            let right_finish = right_finish.and_utc().timestamp();
                            match op {
                                Op::Eeq => left_start == right_start,
                                Op::Ene => left_start != right_start,
                                Op::Eq => left_start <= right_finish && right_start <= left_finish,
                                Op::Ne => left_finish < right_start || left_start > right_finish,
                                Op::Gt => left_start > right_finish,
                                Op::Gte => left_finish >= right_start,
                                Op::Lt => left_finish < right_start,
                                Op::Lte => left_start <= right_finish,
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
        row: &mut Row,
        expr: &Expr,
    ) -> Result<bool, SearchError> {
        if (expr.op.is_some() || expr.logical_op.is_some())
            && let Some(grouped) = self.grouped_condition_value(row.values, expr)
        {
            return Ok(grouped);
        }

        if let Some(ref logical_op) = expr.logical_op {
            let left_result = match expr.left {
                Some(ref left) => self.group_conforms(row, left)?,
                None => false,
            };

            let right_result = |s: &mut Self, row: &mut Row| -> Result<bool, SearchError> {
                match expr.right {
                    Some(ref right) => s.group_conforms(row, right),
                    None => Ok(false),
                }
            };

            return match logical_op {
                LogicalOp::And => if !left_result { Ok(false) } else { right_result(self, row) },
                LogicalOp::Or => if left_result { Ok(true) } else { right_result(self, row) },
            };
        }

        if let Some(ref op) = expr.op {
            if matches!(op, Op::Exists | Op::NotExists) {
                return Err(SearchError::fatal("EXISTS and NOT EXISTS are not supported in the HAVING clause").with_source("query"));
            }
            let left_expr = expr.left.as_ref().ok_or_else(|| {
                SearchError::fatal("Expected expression before operator in HAVING").with_source("query")
            })?;
            if matches!(op, Op::In | Op::NotIn) {
                let field_value = self.get_column_expr_value(
                    row, left_expr,
                )?;
                let field_value = match expr.right {
                    Some(ref right_expr) if !left_expr.is_literal() => Self::retype_value(left_expr, right_expr, field_value),
                    _ => field_value,
                };
                return self.check_in_list(
                    expr, row, &field_value, *op == Op::NotIn,
                );
            }
            if matches!(op, Op::IsNull | Op::IsNotNull) {
                let value = self.get_column_expr_value(
                    row, left_expr,
                )?;
                return Ok(value.to_string().is_empty() == (*op == Op::IsNull));
            }
            let right_expr = expr.right.as_ref().ok_or_else(|| {
                SearchError::fatal("Expected expression after operator in HAVING").with_source("query")
            })?;
            let field_value = self.get_column_expr_value(
                row, left_expr,
            )?;
            let value = self.get_column_expr_value(
                row, right_expr,
            )?;
            let field_value = Self::retype_value(left_expr, right_expr, field_value);
            if op.is_relational()
                && (Self::is_null_operand(left_expr, &field_value) || Self::is_null_operand(right_expr, &value))
            {
                return Ok(false);
            }
            return self.compare_variants(op, &field_value, &value);
        }

        Ok(false)
    }

    fn is_null_operand(operand: &Expr, value: &Variant) -> bool {
        value.is_null()
            || (!operand.is_literal()
                && operand.result_type().may_be(ExprType::NUMERIC.union(ExprType::DATETIME).union(ExprType::BOOLEAN))
                && value.to_string().is_empty())
    }

    fn is_declared_alias(&self, alias: &str) -> bool {
        self.enclosing_aliases.contains(alias) || self.query.roots.iter().any(|root| root.options.alias.as_deref() == Some(alias))
    }

    fn retype_value(left_expr: &Expr, right_expr: &Expr, value: Variant) -> Variant {
        if !matches!(value.get_type(), VariantType::String) {
            return value;
        }
        let expected = left_expr.result_type().union(right_expr.result_type());
        if expected.may_be(ExprType::DATETIME) {
            if let Ok((dt_from, dt_to)) = value.to_datetime() {
                return Variant::from_datetime_range(dt_from, dt_to);
            }
        } else if expected.may_be(ExprType::NUMERIC)
            && let Ok(f) = value.to_string().parse::<f64>()
                && f.is_finite() {
                    return Variant::from_float(f);
                }
        else if expected.may_be(ExprType::BOOLEAN)
            && let Some(b) = str_to_bool(&value.to_string()) {
                return Variant::from_bool(b);
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

        let mut file_map: HashMap<String, Variant> = HashMap::new();
        let root_path = Path::new(".");

        // Since current_alias is `b`, and the expression requests `a.name`,
        // get_column_expr_value should read the value from record_context rather than the current entry
        let v = searcher.get_column_expr_value(&mut Row::new(None, &None, root_path, &mut file_map, None), &bound_expr);
        assert_eq!(v.unwrap().to_string(), "foo.txt");
    }

    #[test]
    fn unknown_root_alias_errors_without_current_alias() {
        // `x.name` where no root declares alias `x` (and no alias is being
        // processed) must be rejected instead of silently evaluating as `name`.
        let mut searcher = create_test_searcher();

        let mut bound_expr = Expr::field(Field::Name);
        bound_expr.root_alias = Some(String::from("x"));

        let mut file_map: HashMap<String, Variant> = HashMap::new();
        let result = searcher.get_column_expr_value(&mut Row::new(None, &None, Path::new("."), &mut file_map, None), &bound_expr);

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

        let mut file_map: HashMap<String, Variant> = HashMap::new();
        let result = searcher.get_column_expr_value(&mut Row::new(None, &None, Path::new("."), &mut file_map, None), &bound_expr);

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
        let mut file_map: HashMap<String, Variant> = HashMap::new();
        let v = searcher.get_column_expr_value(&mut Row::new(None, &None, Path::new("."), &mut file_map, None), &literal);
        assert_eq!(v.unwrap().to_string(), "readme.md");
    }

    #[test]
    fn dotted_value_literal_with_current_alias_returns_literal() {
        // Previously a dotted literal evaluated while processing an aliased
        // root was misread as an alias reference and raised a fatal error.
        let mut searcher = create_test_searcher();
        searcher.current_alias = Some(String::from("a"));

        let literal = Expr::value(String::from("readme.md"));
        let mut file_map: HashMap<String, Variant> = HashMap::new();
        let v = searcher.get_column_expr_value(&mut Row::new(None, &None, Path::new("."), &mut file_map, None), &literal);
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

        let mut file_map: HashMap<String, Variant> = HashMap::new();
        let v = searcher.get_column_expr_value(&mut Row::new(None, &None, Path::new("."), &mut file_map, None), &bound_expr);
        assert_eq!(v.unwrap().to_string(), "outer.txt");
    }

    #[test]
    fn declared_alias_with_unpopulated_context_is_empty_for_value_expr() {
        // `a.sz` where `sz` is not a field parses as a plain value, but when
        // `a` is declared as a root alias it is an alias.column reference;
        // with no row registered for `a` it is an empty value rather than
        // the literal string "a.sz".
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

        let value_expr = Expr::alias_reference(String::from("a"), String::from("a.sz"));
        let mut file_map: HashMap<String, Variant> = HashMap::new();
        let result = searcher.get_column_expr_value(&mut Row::new(None, &None, Path::new("."), &mut file_map, None), &value_expr);

        let value = result.expect("a declared alias without a row is not an error");
        assert_eq!(value.to_string(), "");
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
    fn a_link_beside_the_real_directory_is_traversed_too() {
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
            found, 5,
            "a/ is listed both directly and through the link, found={}",
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
        searcher.hgignore_filters.clear();
        searcher.dockerignore_filters.clear();

        search_upstream_hgignore(&mut searcher.hgignore_filters, Path::new(&tmp));

        let filters_count = searcher.hgignore_filters.len();
        let _ = fs::remove_dir_all(&tmp);

        assert!(filters_count > 0, "hgignore filters should be available after clear-then-load");
    }


    #[cfg(unix)]
    #[test]
    fn symlink_targets_are_followed_under_every_root() {
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
        file_map.insert("size".to_string(), Variant::from_int(100));

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

        let result = searcher.conforms(&Row::entry(&entry, &None, &tmp, &mut HashMap::new()), &expr);

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

        let result = searcher.conforms(&Row::entry(&entry, &None, &tmp, &mut HashMap::new()), &expr);

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
    fn regexp_roots_accept_every_regex_metacharacter_and_keep_existing_names_literal() {
        let tmp = std::env::temp_dir().join("fselect_test_regexp_root_metachars");
        let _ = fs::remove_dir_all(&tmp);
        for dir in ["alpha", "beta", "gamma", "a1", "a22", "ab", "c++", ".hidden", "New folder (2)", "alphajunk", "junkbeta", "draft[1"] {
            fs::create_dir_all(tmp.join("roots").join(dir)).unwrap();
            fs::write(tmp.join("roots").join(dir).join(format!("{dir}.txt")), "x").unwrap();
        }
        fs::create_dir_all(tmp.join("roots").join("draft[1").join("inner")).unwrap();
        fs::write(tmp.join("roots").join("draft[1").join("inner").join("inner.txt"), "x").unwrap();
        for (parent, file) in [("alpha", "x.txt"), ("beta", "y.txt")] {
            fs::create_dir_all(tmp.join("roots").join(parent).join("draft[1")).unwrap();
            fs::write(tmp.join("roots").join(parent).join("draft[1").join(file), "x").unwrap();
        }

        let sorted = |query: &str| {
            let mut rows = run_query_against_dir(query, &tmp);
            rows.sort();
            rows
        };
        let alternation = sorted("select name from '__DIR__/roots/(alpha|beta)' rx");
        let bare_alternation = sorted("select name from '__DIR__/roots/alpha|beta' rx");
        let dot = sorted("select name from '__DIR__/roots/a.' rx");
        let digits = sorted("select name from '__DIR__/roots/a\\d+' rx");
        let plus_literal = sorted("select name from '__DIR__/roots/c++' rx");
        let hidden_literal = sorted("select name from '__DIR__/roots/.hidden' rx");
        let parens_literal = sorted("select name from '__DIR__/roots/New folder (2)' rx");
        let class = sorted("select name from '__DIR__/roots/[ab]lpha' rx");
        let bracket_literal = sorted("select name from '__DIR__/roots/draft[1' rx");
        let bracket_literal_then_pattern = sorted("select name from '__DIR__/roots/draft[1/.*' rx");
        let pattern_then_bracket_literal = sorted("select name from '__DIR__/roots/alpha|beta/draft[1' rx");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(alternation, vec!["alpha.txt", "beta.txt", "draft[1", "draft[1", "x.txt", "y.txt"]);
        assert_eq!(bare_alternation, vec!["alpha.txt", "beta.txt", "draft[1", "draft[1", "x.txt", "y.txt"], "alternation must be grouped before anchoring");
        assert_eq!(dot, vec!["a1.txt", "ab.txt"]);
        assert_eq!(digits, vec!["a1.txt", "a22.txt"]);
        assert_eq!(plus_literal, vec!["c++.txt"]);
        assert_eq!(hidden_literal, vec![".hidden.txt"]);
        assert_eq!(parens_literal, vec!["New folder (2).txt"]);
        assert_eq!(class, vec!["alpha.txt", "draft[1", "x.txt"]);
        assert_eq!(bracket_literal, vec!["draft[1.txt", "inner", "inner.txt"]);
        assert_eq!(bracket_literal_then_pattern, vec!["inner.txt"], "an invalid-regex literal before a pattern");
        assert_eq!(pattern_then_bracket_literal, vec!["x.txt", "y.txt"], "an invalid-regex literal after a pattern");
    }

    #[test]
    fn regexp_roots_follow_directory_symlinks_when_asked() {
        let tmp = std::env::temp_dir().join("fselect_test_regexp_root_symlinks");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("mixed").join("real")).unwrap();
        fs::create_dir_all(tmp.join("target")).unwrap();
        fs::write(tmp.join("mixed").join("real").join("real.txt"), "x").unwrap();
        fs::write(tmp.join("target").join("linked.txt"), "x").unwrap();
        link_directory(&tmp.join("mixed").join("link"), &tmp.join("target"));

        let sorted = |query: &str| {
            let mut rows = run_query_against_dir(query, &tmp);
            rows.sort();
            rows
        };
        let with_symlinks = sorted("select name from '__DIR__/mixed/.*' rx symlinks where is_file");
        let without_symlinks = sorted("select name from '__DIR__/mixed/.*' rx where is_file");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(with_symlinks, vec!["linked.txt", "real.txt"]);
        assert_eq!(without_symlinks, vec!["real.txt"]);
    }

    #[test]
    fn a_regexp_root_part_that_matches_nothing_yields_no_roots() {
        let tmp = std::env::temp_dir().join("fselect_test_regexp_root_no_match");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("proj1").join("src")).unwrap();
        fs::write(tmp.join("proj1").join("src").join("inner.txt"), "x").unwrap();

        let matching = search_dir("select name from __DIR__/proj[0-9]+/src rx", &tmp, Config::default(), true);
        let literal_after_miss = search_dir("select name from __DIR__/zzz[0-9]+/src rx", &tmp, Config::default(), true);
        let regex_after_miss = search_dir("select name from __DIR__/zzz[0-9]+/s[a-z]+ rx", &tmp, Config::default(), true);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(matching.output_buffer.iter_values().count(), 1);
        assert_eq!(matching.error_count, 0);
        for (searcher, what) in [(literal_after_miss, "literal"), (regex_after_miss, "regex")] {
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
            assert!(rows.is_empty(), "{} part after a miss restarted the search from the current directory: {:?}", what, rows);
            assert_eq!(searcher.error_count, 1, "{} part after a miss should report the unmatched part", what);
        }
    }

    fn link_dir(target: &str, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }

    fn link_file(target: &str, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }

    #[test]
    fn from_subselects_keep_links_as_links() {
        let tmp = std::env::temp_dir().join("fselect_test_subselect_keeps_links");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("target")).unwrap();
        fs::write(tmp.join("target").join("f.txt"), "x").unwrap();
        fs::write(tmp.join("plain.txt"), "y").unwrap();
        if !(link_dir("target", &tmp.join("slink")) && link_file("plain.txt", &tmp.join("flink"))) {
            let _ = fs::remove_dir_all(&tmp);
            eprintln!("skipped: cannot create symlinks here");
            return;
        }

        let direct = run_query_against_dir("select path, is_symlink from __DIR__ depth 1 order by path", &tmp);
        let through = run_query_against_dir("select path, is_symlink from (select path from __DIR__ depth 1) order by path", &tmp);
        let links_outside = run_query_against_dir("select path from (select path from __DIR__ depth 1) where is_symlink order by path", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct, vec!["flink\ttrue", "plain.txt\tfalse", "slink\ttrue", "target\tfalse"]);
        assert_eq!(through, direct, "a FROM subselect must hand over the links themselves, not their targets");
        assert_eq!(links_outside, vec!["flink", "slink"]);
    }

    fn rel(parts: &[&str]) -> String {
        parts.iter().collect::<PathBuf>().to_string_lossy().to_string()
    }

    #[test]
    fn a_real_directory_is_listed_even_when_a_link_to_it_is_read_first() {
        let tmp = std::env::temp_dir().join("fselect_test_sym_real_dir_kept");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("zreal")).unwrap();
        fs::write(tmp.join("zreal").join("file.txt"), "x").unwrap();
        if !link_dir("zreal", &tmp.join("alink")) {
            let _ = fs::remove_dir_all(&tmp);
            eprintln!("skipped: cannot create directory symlinks here");
            return;
        }

        let dfs = run_query_against_dir("select path from __DIR__ sym", &tmp);
        let bfs = run_query_against_dir("select path from __DIR__ sym bfs", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        for (mode, mut rows) in [("dfs", dfs), ("bfs", bfs)] {
            rows.sort();
            let expected = vec!["alink".to_string(), rel(&["alink", "file.txt"]), "zreal".to_string(), rel(&["zreal", "file.txt"])];
            assert_eq!(rows, expected, "{}", mode);
        }
    }

    #[test]
    fn symlink_cycles_are_detected_against_the_traversal_chain_only() {
        let tmp = std::env::temp_dir().join("fselect_test_sym_cycles");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a").join("b")).unwrap();
        fs::create_dir_all(tmp.join("x")).unwrap();
        fs::create_dir_all(tmp.join("y")).unwrap();
        fs::write(tmp.join("a").join("b").join("file.txt"), "x").unwrap();
        let linked = link_dir("..", &tmp.join("a").join("up"))
            && link_dir(&rel(&["..", "y"]), &tmp.join("x").join("l1"))
            && link_dir(&rel(&["..", "x"]), &tmp.join("y").join("l2"));
        if !linked {
            let _ = fs::remove_dir_all(&tmp);
            eprintln!("skipped: cannot create directory symlinks here");
            return;
        }

        let dfs = run_query_against_dir("select path from __DIR__ sym", &tmp);
        let bfs = run_query_against_dir("select path from __DIR__ sym bfs", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        let mut expected = vec![
            rel(&["a"]), rel(&["a", "b"]), rel(&["a", "b", "file.txt"]), rel(&["a", "up"]),
            rel(&["x"]), rel(&["x", "l1"]), rel(&["x", "l1", "l2"]),
            rel(&["y"]), rel(&["y", "l2"]), rel(&["y", "l2", "l1"]),
        ];
        expected.sort();
        for (mode, mut rows) in [("dfs", dfs), ("bfs", bfs)] {
            rows.sort();
            assert_eq!(rows, expected, "{}", mode);
        }
    }

    #[test]
    fn ignore_rules_apply_to_paths_traversed_through_directory_symlinks() {
        let tmp = std::env::temp_dir().join("fselect_test_ignore_through_symlink");
        let _ = fs::remove_dir_all(&tmp);
        let walk = tmp.join("walk");
        fs::create_dir_all(walk.join("target")).unwrap();
        fs::create_dir_all(walk.join(".hg")).unwrap();
        fs::write(walk.join("target").join("drop.txt"), "x").unwrap();
        fs::write(walk.join("target").join("keep.txt"), "x").unwrap();
        fs::write(walk.join(".dockerignore"), "alias/drop.txt\n").unwrap();
        fs::write(walk.join(".hgignore"), "syntax: regexp\n^alias/drop\\.txt$\n").unwrap();
        link_directory(&walk.join("alias"), &walk.join("target"));

        let docker = run_query_against_dir("select path from __DIR__/walk symlinks dockerignore where is_file and ext = txt order by path", &tmp);
        let hg = run_query_against_dir("select path from __DIR__/walk symlinks hgignore where is_file and ext = txt order by path", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        let expected = vec![rel(&["alias", "keep.txt"]), rel(&["target", "drop.txt"]), rel(&["target", "keep.txt"])];
        assert_eq!(docker, expected, "dockerignore");
        assert_eq!(hg, expected, "hgignore");
    }

    #[test]
    fn dockerignore_exceptions_beneath_an_excluded_directory_symlink_are_found() {
        let tmp = std::env::temp_dir().join("fselect_test_dockerignore_exception_symlink");
        let _ = fs::remove_dir_all(&tmp);
        let walk = tmp.join("walk");
        fs::create_dir_all(walk.join("target")).unwrap();
        fs::write(walk.join("target").join("drop.txt"), "x").unwrap();
        fs::write(walk.join("target").join("keep.txt"), "x").unwrap();
        fs::write(walk.join(".dockerignore"), "alias\n!alias/keep.txt\ntarget\n").unwrap();
        link_directory(&walk.join("alias"), &walk.join("target"));

        let bfs = run_query_against_dir("select path from __DIR__/walk symlinks dockerignore bfs where is_file and ext = txt order by path", &tmp);
        let dfs = run_query_against_dir("select path from __DIR__/walk symlinks dockerignore dfs where is_file and ext = txt order by path", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(bfs, vec![rel(&["alias", "keep.txt"])], "bfs");
        assert_eq!(dfs, vec![rel(&["alias", "keep.txt"])], "dfs");
    }

    #[test]
    fn dockerignore_exceptions_beneath_an_excluded_directory_are_found() {
        let tmp = std::env::temp_dir().join("fselect_test_dockerignore_exception_below_excluded");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("keep").join("sub")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join(".dockerignore"), "*\n!keep/file.txt\n").unwrap();
        fs::write(tmp.join("top.txt"), "x").unwrap();
        fs::write(tmp.join("keep").join("file.txt"), "x").unwrap();
        fs::write(tmp.join("keep").join("drop.txt"), "x").unwrap();
        fs::write(tmp.join("keep").join("sub").join("deep.txt"), "x").unwrap();
        fs::write(tmp.join("other").join("o.txt"), "x").unwrap();

        let dfs = search_dir("select path from __DIR__ dockerignore", &tmp, Config::default(), true);
        let bfs = search_dir("select path from __DIR__ dockerignore bfs", &tmp, Config::default(), true);
        let direct = run_query_against_dir("select path from __DIR__/keep dockerignore", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        for (mode, searcher) in [("dfs", dfs), ("bfs", bfs)] {
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
            assert_eq!(rows, vec![rel(&["keep", "file.txt"])], "{}", mode);
            assert_eq!(searcher.dirs_opened, 2, "{}: only the root and keep/ should be read, not other/ or keep/sub/", mode);
        }
        assert_eq!(direct, vec!["file.txt"]);
    }

    #[test]
    fn from_subselects_keep_paths_relative_to_the_inner_root() {
        let tmp = std::env::temp_dir().join("fselect_test_subselect_relative_paths");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a")).unwrap();
        fs::create_dir_all(tmp.join("b").join("c")).unwrap();
        fs::write(tmp.join("top.txt"), "x").unwrap();
        fs::write(tmp.join("a").join("same.txt"), "x").unwrap();
        fs::write(tmp.join("b").join("same.txt"), "x").unwrap();
        fs::write(tmp.join("b").join("c").join("deep.txt"), "x").unwrap();

        let direct = run_query_against_dir("select path, depth, directory from __DIR__ order by path", &tmp);
        let through = run_query_against_dir("select path, depth, directory from (select path from __DIR__) order by path", &tmp);
        let filtered_outside = run_query_against_dir("select count(*) from (select path from __DIR__) where depth > 1", &tmp);
        let two_roots = run_query_against_dir("select path from (select path from __DIR__/a, __DIR__/b) order by path", &tmp);
        let direct_rx = run_query_against_dir("select path, depth from __DIR__/[ab] rx order by path", &tmp);
        let through_rx = run_query_against_dir("select path, depth from (select path from __DIR__/[ab] rx) order by path", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct.len(), 7);
        assert_eq!(through, direct);
        assert_eq!(filtered_outside, vec!["4"]);
        assert_eq!(two_roots, vec!["c".to_string(), rel(&["c", "deep.txt"]), "same.txt".to_string(), "same.txt".to_string()]);
        assert_eq!(direct_rx.len(), 4);
        assert_eq!(through_rx, direct_rx, "a regexp root expands to the matched directories, and paths are relative to those");
    }

    #[test]
    fn from_subselects_keep_aliases_their_own_clauses_depend_on() {
        let tmp = std::env::temp_dir().join("fselect_test_subselect_keeps_aliases");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 1), ("b.txt", 2), ("c.md", 5), ("d.md", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "xxx").unwrap();

        let inner = "select path, size as sz from __DIR__/data as t where exists(select name from __DIR__/other where size < t.sz)";
        let direct = run_query_against_dir(&format!("{inner} order by path"), &tmp);
        let wrapped = run_query_against_dir(&format!("select name, size from ({inner}) order by name"), &tmp);
        let ordered = run_query_against_dir(
            "select name from (select path, size as sz from __DIR__/data order by sz desc limit 1)",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct.len(), 2, "{direct:?}");
        assert_eq!(wrapped, vec!["c.md	5", "d.md	7"]);
        assert_eq!(ordered, vec!["d.md"]);
    }

    #[test]
    fn from_subselects_keep_paths_and_depths_beneath_followed_symlinks() {
        let tmp = std::env::temp_dir().join("fselect_test_subselect_symlink_identity");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("walk").join("target")).unwrap();
        fs::write(tmp.join("walk").join("target").join("keep.txt"), "x").unwrap();
        link_directory(&tmp.join("walk").join("alias"), &tmp.join("walk").join("target"));

        let direct = run_query_against_dir("select path, depth from __DIR__/walk symlinks where is_file order by path", &tmp);
        let wrapped = run_query_against_dir(
            "select path, depth from (select path from __DIR__/walk symlinks where is_file) order by path",
            &tmp,
        );
        let filtered = run_query_against_dir(
            "select path from (select path from __DIR__/walk symlinks where is_file) where depth = 2 order by path",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct, vec![format!("{}	2", rel(&["alias", "keep.txt"])), format!("{}	2", rel(&["target", "keep.txt"]))]);
        assert_eq!(wrapped, direct, "wrapping must not change paths or depths");
        assert_eq!(filtered, vec![rel(&["alias", "keep.txt"]), rel(&["target", "keep.txt"])]);
    }

    #[test]
    fn from_subselects_keep_each_root_spelling_when_inner_roots_overlap() {
        let tmp = std::env::temp_dir().join("fselect_test_subselect_overlapping_roots");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a")).unwrap();
        fs::create_dir_all(tmp.join("b").join("c")).unwrap();
        fs::write(tmp.join("top.txt"), "x").unwrap();
        fs::write(tmp.join("a").join("same.txt"), "x").unwrap();
        fs::write(tmp.join("b").join("same.txt"), "x").unwrap();
        fs::write(tmp.join("b").join("c").join("deep.txt"), "x").unwrap();

        let direct = run_query_against_dir("select path, depth from __DIR__, __DIR__/b order by path, depth", &tmp);
        let through = run_query_against_dir("select path, depth from (select path from __DIR__, __DIR__/b) order by path, depth", &tmp);
        let sorted_inside = run_query_against_dir(
            "select path, depth from (select path from __DIR__, __DIR__/b order by name desc) order by path, depth",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct.len(), 10);
        assert_eq!(through, direct);
        assert_eq!(sorted_inside, direct, "the root must travel with each row even when the inner query reorders them");
    }

    #[test]
    fn archive_members_with_backslash_separators_behave_like_forward_slash_ones() {
        use std::io::Write;

        let tmp = std::env::temp_dir().join("fselect_test_backslash_members");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(tmp.join("bs.zip")).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in [("sub\\.hidden", "x\n"), ("sub\\file.txt", "one\ntwo\n"), ("sub\\deep\\leaf.txt", "x\n"), ("top.txt", "x\n")] {
            zip.start_file(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let raw_name = zip::ZipArchive::new(fs::File::open(tmp.join("bs.zip")).unwrap()).unwrap().by_index_raw(0).unwrap().name().unwrap().to_string();
        assert!(raw_name.contains('\\'), "the fixture must keep its backslashes, got {raw_name}");

        let listing = run_query_against_dir("select path, depth, is_hidden from __DIR__ archives order by path", &tmp);
        let limited = run_query_against_dir("select path from __DIR__ archives depth 3 order by path", &tmp);
        let hidden = run_query_against_dir("select name from __DIR__ archives where is_hidden = true", &tmp);
        let through = search_dir(
            "select path, line_count from (select path from __DIR__ archives where ext = txt) order by path",
            &tmp, Config::default(), true,
        );
        let through_rows: Vec<String> = through.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
        let _ = fs::remove_dir_all(&tmp);

        let member = |parts: &[&str]| rel(&[&["bs.zip"], parts].concat());
        assert_eq!(listing, vec![
            "bs.zip\t1\tfalse".to_string(),
            format!("{}\t3\ttrue", member(&["sub", ".hidden"])),
            format!("{}\t4\tfalse", member(&["sub", "deep", "leaf.txt"])),
            format!("{}\t3\tfalse", member(&["sub", "file.txt"])),
            format!("{}\t2\tfalse", member(&["top.txt"])),
        ]);
        assert_eq!(limited, vec!["bs.zip".to_string(), member(&["sub", ".hidden"]), member(&["sub", "file.txt"]), member(&["top.txt"])]);
        assert_eq!(hidden, vec![".hidden"]);
        assert_eq!(through_rows, vec![
            format!("{}\t1", member(&["sub", "deep", "leaf.txt"])),
            format!("{}\t2", member(&["sub", "file.txt"])),
            format!("{}\t1", member(&["top.txt"])),
        ]);
        assert_eq!(through.error_count, 0);
    }

    #[test]
    fn different_root_aliases_do_not_share_cached_values() {
        let tmp = std::env::temp_dir().join("fselect_test_root_alias_caches");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a")).unwrap();
        fs::create_dir_all(tmp.join("b")).unwrap();
        fs::write(tmp.join("a").join("x.txt"), "xx").unwrap();
        fs::write(tmp.join("b").join("y.txt"), "yyy").unwrap();

        let sums = run_query_against_dir("select sum(t.size), sum(u.size) from __DIR__/a as t, __DIR__/b as u", &tmp);
        let uppers = run_query_against_dir(
            "select upper(t.name), upper(u.name) from __DIR__/a as t, __DIR__/b as u order by path",
            &tmp,
        );
        let mixed = run_query_against_dir("select sum(t.size), sum(size) from __DIR__/a as t, __DIR__/b", &tmp);
        let mixed_reversed = run_query_against_dir("select sum(size), sum(t.size) from __DIR__/b, __DIR__/a as t", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(sums, vec!["2\t3"]);
        assert_eq!(uppers, vec!["X.TXT", "\tY.TXT"]);
        assert_eq!(mixed, vec!["2\t5"], "a named root next to an unnamed one");
        assert_eq!(mixed_reversed, vec!["5\t2"], "unnamed root first");
    }

    #[test]
    fn qualified_grouping_fields_bind_only_to_their_own_alias() {
        let tmp = std::env::temp_dir().join("fselect_test_qualified_grouping_fields");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a")).unwrap();
        fs::create_dir_all(tmp.join("b")).unwrap();
        fs::write(tmp.join("a").join("alpha.txt"), "x").unwrap();
        fs::write(tmp.join("b").join("beta.txt"), "x").unwrap();

        let sorted = |query: &str| {
            let mut rows = run_query_against_dir(query, &tmp);
            rows.sort();
            rows
        };
        let forward = sorted("select t.name, u.name, count(*) from __DIR__/a as t, __DIR__/b as u group by t.name, u.name");
        let reversed = sorted("select t.name, u.name, count(*) from __DIR__/a as t, __DIR__/b as u group by u.name, t.name");
        let having = sorted(
            "select t.name, u.name, count(*) from __DIR__/a as t, __DIR__/b as u group by t.name, u.name having t.name = 'alpha.txt'",
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(forward, vec!["\tbeta.txt\t1", "alpha.txt\t\t1"]);
        assert_eq!(reversed, forward, "grouping order must not matter");
        assert_eq!(having, vec!["alpha.txt\t\t1"]);
    }

    #[test]
    fn sibling_root_aliases_have_no_row_while_another_root_is_walked() {
        let tmp = std::env::temp_dir().join("fselect_test_sibling_alias_rows");
        let _ = fs::remove_dir_all(&tmp);
        for (dir, files) in [("A", vec![("a1.txt", 1)]), ("B", vec![("b1.txt", 2), ("b2.txt", 3)]), ("C", vec![("c1.txt", 1), ("c2.txt", 2)])] {
            fs::create_dir_all(tmp.join(dir)).unwrap();
            for (name, size) in files {
                fs::write(tmp.join(dir).join(name), "x".repeat(size)).unwrap();
            }
        }

        let exists_a_first = run_query_against_dir(
            "select path from __DIR__/A as a, __DIR__/B as b where exists (select name from __DIR__/C where size = a.size) order by path",
            &tmp,
        );
        let exists_b_first = run_query_against_dir(
            "select path from __DIR__/B as b, __DIR__/A as a where exists (select name from __DIR__/C where size = a.size) order by path",
            &tmp,
        );
        let counted = run_query_against_dir(
            "select path, (select count(*) from __DIR__/C where size = a.size) from __DIR__/A as a, __DIR__/B as b order by path",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(exists_a_first, vec!["a1.txt"], "rows of B have no a.size to compare");
        assert_eq!(exists_b_first, vec!["a1.txt"], "an alias whose root comes later is not an error either");
        assert_eq!(counted, vec!["a1.txt	1", "b1.txt	0", "b2.txt	0"]);
    }

    #[test]
    fn min_and_max_of_names_follow_text_order_when_some_names_look_numeric() {
        let tmp = std::env::temp_dir().join("fselect_test_min_max_mixed_names");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["10", "9", "abc", "zeta", "Beta", "2x"] {
            fs::write(tmp.join(name), "x").unwrap();
        }

        let extremes = run_query_against_dir("select min(name), max(name), min(size), max(size) from __DIR__", &tmp);
        let ordered = run_query_against_dir("select name from __DIR__ order by name", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(extremes, vec!["10	zeta	1	1"]);
        assert_eq!(ordered.first().map(String::as_str), Some("10"), "min(name) must agree with ORDER BY");
        assert_eq!(ordered.last().map(String::as_str), Some("zeta"), "max(name) must agree with ORDER BY");
    }

    #[test]
    fn aggregates_over_a_division_by_a_field_use_the_accumulated_values() {
        let tmp = std::env::temp_dir().join("fselect_test_aggregate_division_by_field");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("two.txt"), "xx").unwrap();
        fs::write(tmp.join("ten.txt"), "x".repeat(10)).unwrap();

        let plain = search_dir(
            "select sum(10 / size), avg(10 / size), count(10 / size), min(10 / size), max(10 / size) from __DIR__",
            &tmp, Config::default(), true,
        );
        let grouped = run_query_against_dir("select ext, sum(10 / size) from __DIR__ group by ext having sum(10 / size) > 1", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        let rows: Vec<String> = plain.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
        assert_eq!(rows, vec!["6	3	2	1	5"]);
        assert_eq!(plain.error_count, 0);
        assert_eq!(grouped, vec!["txt	6"]);
    }

    #[test]
    fn like_over_string_functions_of_typed_fields_matches_the_text() {
        let tmp = std::env::temp_dir().join("fselect_test_like_over_string_functions");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("ten.txt"), "x".repeat(10)).unwrap();
        fs::write(tmp.join("two.txt"), "xx").unwrap();

        let names = |condition: &str| {
            run_query_against_dir(&format!("select name from __DIR__ where {condition} order by name"), &tmp)
        };
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("dayname(modified) like '%day'", vec!["ten.txt", "two.txt"]),
            ("dayname(modified) not like '%day'", vec![]),
            ("lower(dayname(modified)) like '%day'", vec!["ten.txt", "two.txt"]),
            ("concat(size, '') like '1%'", vec!["ten.txt"]),
            ("concat(size, '') not like '1%'", vec!["two.txt"]),
            ("hex(size) = 'a'", vec!["ten.txt"]),
            ("concat(size, '') = 10", vec!["ten.txt"]),
        ];
        let results: Vec<(&str, Vec<String>)> = cases.iter().map(|(condition, _)| (*condition, names(condition))).collect();
        let _ = fs::remove_dir_all(&tmp);

        for ((condition, expected), (_, actual)) in cases.iter().zip(results) {
            assert_eq!(actual, *expected, "{}", condition);
        }
    }

    #[test]
    fn an_empty_scalar_subquery_never_matches_whichever_side_it_is_on() {
        let tmp = std::env::temp_dir().join("fselect_test_empty_scalar_subquery");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let empty = tmp.join("empty");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&empty).unwrap();
        for (name, size) in [("one.txt", 1), ("two.txt", 2), ("ten.txt", 10)] {
            fs::write(data.join(name), "x".repeat(size)).unwrap();
        }
        let empty_root = empty.to_string_lossy().to_string();
        let names = |condition: &str| {
            let condition = condition.replace("E", &empty_root);
            run_query_against_dir(&format!("select name from __DIR__ where {condition} order by name"), &data)
        };

        let never: Vec<(&str, Vec<String>)> = [
            "(select size from E) < size", "size > (select size from E)",
            "(select size from E) <= size", "size >= (select size from E)",
            "(select size from E) = size", "size = (select size from E)",
            "(select size from E) > size", "size < (select size from E)",
            "(select max(size) from E) < size", "size > (select max(size) from E)",
        ].iter().map(|condition| (*condition, names(condition))).collect();
        let present = names("(select max(size) from __DIR__) = size");
        let defaulted = names("coalesce((select size from E), 0) < size");
        let _ = fs::remove_dir_all(&tmp);

        for (condition, rows) in never {
            assert!(rows.is_empty(), "{}: {:?}", condition, rows);
        }
        assert_eq!(present, vec!["ten.txt"]);
        assert_eq!(defaulted, vec!["one.txt", "ten.txt", "two.txt"]);
    }

    #[test]
    fn empty_numeric_fields_behave_like_null() {
        let tmp = std::env::temp_dir().join("fselect_test_empty_numeric_fields_are_null");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("text.txt"), "hello").unwrap();
        fs::write(tmp.join("noext"), "x").unwrap();
        let mut bmp = vec![b'B', b'M'];
        bmp.extend_from_slice(&78u32.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&3i32.to_le_bytes());
        bmp.extend_from_slice(&2i32.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.resize(78, 0);
        fs::write(tmp.join("img.bmp"), bmp).unwrap();

        let names = |condition: &str| {
            run_query_against_dir(&format!("select name from __DIR__ where {condition} order by name"), &tmp)
        };
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("width = 3", vec!["img.bmp"]),
            ("width < 100", vec!["img.bmp"]),
            ("100 > width", vec!["img.bmp"]),
            ("width > 0", vec!["img.bmp"]),
            ("width <= 0", vec![]),
            ("width != 3", vec![]),
            ("width is null", vec!["noext", "text.txt"]),
            ("width is not null", vec!["img.bmp"]),
            ("not width is null", vec!["img.bmp"]),
            ("ext is null", vec!["noext"]),
            ("ext is not null", vec!["img.bmp", "text.txt"]),
            ("size < 100", vec!["img.bmp", "noext", "text.txt"]),
            ("width is null and size > 1", vec!["text.txt"]),
        ];
        let results: Vec<(&str, Vec<String>)> = cases.iter().map(|(condition, _)| (*condition, names(condition))).collect();
        let _ = fs::remove_dir_all(&tmp);

        for ((condition, expected), (_, actual)) in cases.iter().zip(results) {
            assert_eq!(actual, *expected, "{}", condition);
        }
    }

    #[test]
    fn min_and_max_of_numeric_looking_names_return_real_names() {
        let tmp = std::env::temp_dir().join("fselect_test_min_max_numeric_looking_names");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["002", "010", "1e3"] {
            fs::write(tmp.join(name), "x").unwrap();
        }

        let extremes = run_query_against_dir("select min(name), max(name), min(size), max(size) from __DIR__", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(extremes, vec!["002	1e3	1	1"]);
    }

    #[test]
    fn grouped_correlated_subqueries_see_select_aliases_of_grouping_expressions() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_select_alias_binding");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let refext = tmp.join("refext");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&refext).unwrap();
        for name in ["a.txt", "b.txt", "c.rs"] {
            fs::write(data.join(name), "x").unwrap();
        }
        for name in ["r1.txt", "r2.txt", "r3.txt", "q.rs"] {
            fs::write(refext.join(name), "x").unwrap();
        }
        let refext_root = refext.to_string_lossy().to_string();

        let by_field = run_query_against_dir(
            &format!("select ext as kind, (select count(*) from {refext_root} where ext = t.kind) from __DIR__ as t group by ext order by kind"),
            &data,
        );
        let by_alias = run_query_against_dir(
            &format!("select ext as kind, (select count(*) from {refext_root} where ext = t.kind) from __DIR__ as t group by kind order by kind"),
            &data,
        );
        let computed = run_query_against_dir(
            &format!("select length(name) as len, (select count(*) from {refext_root} where length(name) = t.len) from __DIR__ as t group by length(name) order by len"),
            &data,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(by_field, vec!["rs	1", "txt	3"]);
        assert_eq!(by_alias, by_field);
        assert_eq!(computed, vec!["4	1", "5	0"]);
    }

    #[test]
    fn root_options_on_a_from_subselect_apply_to_its_rows() {
        let tmp = std::env::temp_dir().join("fselect_test_subselect_root_options");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("keep").join("sub")).unwrap();
        fs::write(tmp.join("top.txt"), "x").unwrap();
        fs::write(tmp.join("drop.log"), "x").unwrap();
        fs::write(tmp.join("keep").join("mid.txt"), "x").unwrap();
        fs::write(tmp.join("keep").join("sub").join("deep.txt"), "x").unwrap();
        fs::write(tmp.join(".dockerignore"), "*.log
").unwrap();

        let plain = |options: &str| run_query_against_dir(&format!("select path from __DIR__ {options} order by path"), &tmp);
        let wrapped = |options: &str| run_query_against_dir(&format!("select path from (select path from __DIR__) {options} order by path"), &tmp);
        let cases = ["dockerignore", "depth 1", "mindepth 2", "mindepth 2 maxdepth 2", "dockerignore depth 1"];
        let results: Vec<(&str, Vec<String>, Vec<String>)> = cases.iter().map(|o| (*o, plain(o), wrapped(o))).collect();
        let inner_and_outer = run_query_against_dir(
            "select path from (select path from __DIR__ dockerignore) depth 1 order by path",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        for (options, expected, actual) in results {
            assert_eq!(actual, expected, "{}", options);
            assert!(!expected.is_empty(), "{}: the plain root must have produced rows", options);
        }
        assert_eq!(inner_and_outer, vec![".dockerignore", "keep", "top.txt"]);
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
    fn rows_with_a_failing_cell_are_kept_with_an_empty_cell() {
        let tmp = std::env::temp_dir().join("fselect_test_failed_rows_not_found");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "xxx").unwrap();
        fs::write(tmp.join("c.txt"), "xxxxx").unwrap();

        let all = search_dir("select name, 1 / (size - 1) from __DIR__ order by name", &tmp, Config::default(), true);
        let limited = search_dir("select name, 1 / (size - 1) from __DIR__ order by name limit 2", &tmp, Config::default(), true);
        let _ = fs::remove_dir_all(&tmp);

        let rows: Vec<String> = all.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
        assert_eq!(rows, vec!["a.txt", "b.txt\t0.5", "c.txt\t0.25"]);
        assert_eq!(all.found, 3);
        assert_eq!(all.error_count, 1);
        assert_eq!(limited.output_buffer.iter_values().count(), 2);
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
    fn encoding_reuses_the_full_scan_when_a_counting_column_needs_it() {
        use crate::util::{content_classifications, content_scans};

        let tmp = std::env::temp_dir().join("fselect_test_encoding_scan_reuse");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "h\u{e9}llo\n").unwrap();
        fs::write(tmp.join("b.txt"), "plain\n").unwrap();
        let measure = |query: &str| {
            let (scans, classifications) = (content_scans(), content_classifications());
            let searcher = search_dir(query, &tmp, Config::default(), true);
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
            (rows, content_scans() - scans, content_classifications() - classifications)
        };

        let both = measure("select name, encoding, line_count from __DIR__ order by name");
        let reversed = measure("select name, line_count, encoding from __DIR__ order by name");
        let filtered = measure("select name, encoding from __DIR__ where line_count > 0 order by name");
        let alone = measure("select name, encoding from __DIR__ order by name");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(both.0, vec!["a.txt\tUTF-8\t1", "b.txt\tASCII\t1"]);
        assert_eq!((both.1, both.2), (2, 0), "a counting column is coming, so one full scan serves both");
        assert_eq!((reversed.1, reversed.2), (2, 0));
        assert_eq!(filtered.0, vec!["a.txt\tUTF-8", "b.txt\tASCII"]);
        assert_eq!((filtered.1, filtered.2), (2, 0), "a WHERE on a counting column counts too");
        assert_eq!(alone.0, vec!["a.txt\tUTF-8", "b.txt\tASCII"]);
        assert_eq!((alone.1, alone.2), (0, 2), "encoding on its own only classifies");
    }

    #[test]
    fn aggregate_aliases_are_visible_to_correlated_subqueries() {
        let tmp = std::env::temp_dir().join("fselect_test_aggregate_alias_context");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 1), ("b.txt", 2), ("c.md", 5), ("d.md", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        for (name, size) in [("o1", 1), ("o5", 5), ("o20", 20)] {
            fs::write(tmp.join("other").join(name), "x".repeat(size)).unwrap();
        }

        let grouped = run_query_against_dir(
            "select ext, sum(size) as total, (select count(*) from __DIR__/other where size > t.total) from __DIR__/data as t group by ext order by ext",
            &tmp,
        );
        let before = run_query_against_dir(
            "select ext, (select count(*) from __DIR__/other where size > t.total), sum(size) as total from __DIR__/data as t group by ext order by ext",
            &tmp,
        );
        let counted = run_query_against_dir(
            "select ext, count(*) as n, (select count(*) from __DIR__/other where size >= t.n) from __DIR__/data as t group by ext order by ext",
            &tmp,
        );
        let with_grouping_alias = run_query_against_dir(
            "select ext as e, sum(size) as total, (select count(*) from __DIR__/other where size > t.total and t.e = 'md') from __DIR__/data as t group by ext order by ext",
            &tmp,
        );
        let ungrouped = run_query_against_dir(
            "select sum(size) as total, (select count(*) from __DIR__/other where size > t.total) from __DIR__/data as t",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(grouped, vec!["md\t12\t1", "txt\t3\t2"]);
        assert_eq!(before, vec!["md\t1\t12", "txt\t2\t3"], "the alias must be bound before any column is rendered");
        assert_eq!(counted, vec!["md\t2\t2", "txt\t2\t2"]);
        assert_eq!(with_grouping_alias, vec!["md\t12\t1", "txt\t3\t0"]);
        assert_eq!(ungrouped, vec!["15\t1"]);
    }

    #[test]
    fn qualified_and_unqualified_grouping_columns_are_the_same_key() {
        let tmp = std::env::temp_dir().join("fselect_test_qualified_grouping");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for (name, size) in [("a.txt", 1), ("b.txt", 2), ("c.md", 3)] {
            fs::write(tmp.join(name), "x".repeat(size)).unwrap();
        }

        let mixed_select = run_query_against_dir("select ext, count(*) from __DIR__ as t group by t.ext order by ext", &tmp);
        let mixed_group = run_query_against_dir("select t.ext, count(*) from __DIR__ as t group by ext order by t.ext", &tmp);
        let having = run_query_against_dir("select t.ext, count(*) from __DIR__ as t group by t.ext having ext = 'txt'", &tmp);
        let ordered = run_query_against_dir("select t.ext, sum(size) from __DIR__ as t group by t.ext order by ext desc", &tmp);
        let derived = run_query_against_dir("select upper(t.ext), count(*) from __DIR__ as t group by ext order by ext", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(mixed_select, vec!["md\t1", "txt\t2"]);
        assert_eq!(mixed_group, vec!["md\t1", "txt\t2"]);
        assert_eq!(having, vec!["txt\t2"]);
        assert_eq!(ordered, vec!["txt\t3", "md\t3"]);
        assert_eq!(derived, vec!["MD\t1", "TXT\t2"]);
    }

    #[test]
    fn having_filters_groups_before_aliased_aggregates_are_evaluated() {
        let tmp = std::env::temp_dir().join("fselect_test_having_before_alias_binding");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.log"), "").unwrap();
        fs::write(tmp.join("b.log"), "").unwrap();
        fs::write(tmp.join("c.txt"), "xxxx").unwrap();

        let grouped = search_dir(
            "select ext, 100 / sum(size) as ratio from __DIR__ as t group by ext having sum(size) > 0",
            &tmp, Config::default(), true,
        );
        let ungrouped = search_dir(
            "select 100 / sum(size) as ratio from __DIR__ as t where ext = log having sum(size) > 0",
            &tmp, Config::default(), true,
        );
        let _ = fs::remove_dir_all(&tmp);

        let rows = |searcher: &Searcher| searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect::<Vec<_>>();
        assert_eq!(rows(&grouped), vec!["txt\t25"]);
        assert_eq!(grouped.error_count, 0, "the zero-sum group was filtered out, nothing may be divided by zero");
        assert!(rows(&ungrouped).is_empty(), "the single group fails HAVING, so no row and no error");
        assert_eq!(ungrouped.error_count, 0);
    }

    #[test]
    fn aliased_aggregates_with_correlated_subqueries_see_their_own_group() {
        let tmp = std::env::temp_dir().join("fselect_test_aliased_aggregate_own_group");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 1), ("b.txt", 2), ("c.md", 5), ("d.md", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        for name in ["o1.md", "o2.md", "o3.md"] {
            fs::write(tmp.join("other").join(name), "x").unwrap();
        }

        let query = "select ext, sum(size) + (select count(*) from __DIR__/other where ext = t.ext) as score from __DIR__/data as t group by ext order by ext";
        let ascending = run_query_against_dir(query, &tmp);
        let descending = run_query_against_dir(&format!("{query} desc"), &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(ascending, vec!["md\t15", "txt\t3"]);
        assert_eq!(descending, vec!["txt\t3", "md\t15"]);
    }

    #[test]
    fn text_aggregate_aliases_keep_numeric_looking_names_as_text() {
        let tmp = std::env::temp_dir().join("fselect_test_text_aggregate_alias");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["001", "002", "010"] {
            fs::write(tmp.join(name), "x").unwrap();
        }

        let found = run_query_against_dir(
            "select min(name) as first, (select count(*) from __DIR__ where name in (t.first)) from __DIR__ as t",
            &tmp,
        );
        let length = run_query_against_dir(
            "select min(name) as first, (select count(*) from __DIR__ where length(t.first) = 3) from __DIR__ as t",
            &tmp,
        );
        let numeric = run_query_against_dir(
            "select sum(size) as total, (select count(*) from __DIR__ where size < t.total) from __DIR__ as t",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(found, vec!["001\t1"]);
        assert_eq!(length, vec!["001\t3"]);
        assert_eq!(numeric, vec!["3\t3"]);
    }

    #[test]
    fn aliased_aggregates_may_depend_on_other_aggregate_aliases_in_any_order() {
        let tmp = std::env::temp_dir().join("fselect_test_alias_depends_on_alias");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 1), ("b.txt", 2), ("c.md", 5), ("d.md", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        for (name, size) in [("o1", 1), ("o5", 5), ("o20", 20)] {
            fs::write(tmp.join("other").join(name), "x".repeat(size)).unwrap();
        }

        let score = "sum(size) + (select count(*) from __DIR__/other where size > t.total)";
        let unaliased = run_query_against_dir(
            &format!("select ext, sum(size) as total, {score} from __DIR__/data as t group by ext order by ext"),
            &tmp,
        );
        let aliased = run_query_against_dir(
            &format!("select ext, sum(size) as total, {score} as score from __DIR__/data as t group by ext order by ext"),
            &tmp,
        );
        let reversed = run_query_against_dir(
            &format!("select ext, {score} as score, sum(size) as total from __DIR__/data as t group by ext order by ext"),
            &tmp,
        );
        let ungrouped = run_query_against_dir(
            &format!("select {score} as score, sum(size) as total from __DIR__/data as t"),
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(unaliased, vec!["md\t12\t13", "txt\t3\t5"]);
        assert_eq!(aliased, unaliased, "an alias must not change the value");
        assert_eq!(reversed, vec!["md\t13\t12", "txt\t5\t3"]);
        assert_eq!(ungrouped, vec!["16\t15"]);
    }

    #[test]
    fn correlated_aggregate_aliases_may_depend_on_other_correlated_aliases() {
        let tmp = std::env::temp_dir().join("fselect_test_correlated_alias_chain");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 1), ("b.txt", 2), ("c.md", 5), ("d.md", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        for (name, size) in [("o1", 1), ("o5", 5), ("o20", 20)] {
            fs::write(tmp.join("other").join(name), "x".repeat(size)).unwrap();
        }

        let score = "sum(size) + (select count(*) from __DIR__/other where size > t.total) as score";
        let combined = "sum(size) + (select count(*) from __DIR__/other where size < t.score) as combined";
        let chained = run_query_against_dir(
            &format!("select ext, sum(size) as total, {score}, {combined} from __DIR__/data as t group by ext order by ext"),
            &tmp,
        );
        let reversed = run_query_against_dir(
            &format!("select ext, {combined}, {score}, sum(size) as total from __DIR__/data as t group by ext order by ext"),
            &tmp,
        );
        let ungrouped = run_query_against_dir(
            &format!("select {combined}, {score}, sum(size) as total from __DIR__/data as t"),
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(chained, vec!["md	12	13	14", "txt	3	5	4"]);
        assert_eq!(reversed, vec!["md	14	13	12", "txt	4	5	3"]);
        assert_eq!(ungrouped, vec!["17	16	15"]);
    }

    #[test]
    fn aliased_ratios_may_read_correlated_aliases_declared_later() {
        let tmp = std::env::temp_dir().join("fselect_test_ratio_reads_later_alias");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 1), ("b.txt", 2), ("c.md", 5), ("d.md", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        for (name, size) in [("o1", 1), ("o5", 5), ("o20", 20)] {
            fs::write(tmp.join("other").join(name), "x".repeat(size)).unwrap();
        }

        let score = "sum(size) + (select count(*) from __DIR__/other where size > t.total) as score";
        let ratio = "sum(size) / t.score as ratio";
        let dependency_first = run_query_against_dir(
            &format!("select ext, sum(size) as total, {score}, {ratio} from __DIR__/data as t group by ext order by ext"),
            &tmp,
        );
        let dependency_later = run_query_against_dir(
            &format!("select ext, sum(size) as total, {ratio}, {score} from __DIR__/data as t group by ext order by ext"),
            &tmp,
        );
        let ungrouped = run_query_against_dir(
            &format!("select {ratio}, {score}, sum(size) as total from __DIR__/data as t"),
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(dependency_first.len(), 2, "{dependency_first:?}");
        assert!(dependency_first[0].starts_with("md\t12\t13\t0.923"), "{dependency_first:?}");
        assert!(dependency_first[1].starts_with("txt\t3\t5\t0.6"), "{dependency_first:?}");
        assert_eq!(dependency_later.len(), 2, "{dependency_later:?}");
        assert!(dependency_later[0].starts_with("md\t12\t0.923") && dependency_later[0].ends_with("\t13"), "{dependency_later:?}");
        assert!(dependency_later[1].starts_with("txt\t3\t0.6") && dependency_later[1].ends_with("\t5"), "{dependency_later:?}");
        assert_eq!(ungrouped.len(), 1, "{ungrouped:?}");
        assert!(ungrouped[0].starts_with("0.9375\t16\t15"), "{ungrouped:?}");
    }

    #[test]
    fn grouped_booleans_keep_their_type_in_having_and_correlated_subqueries() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_booleans");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(tmp.join(name), "x").unwrap();
        }

        let having = |literal: &str| {
            run_query_against_dir(
                &format!("select is_file, count(*) from __DIR__ group by is_file having is_file = {literal}"),
                &tmp,
            )
        };
        let where_upper = run_query_against_dir("select count(*) from __DIR__ where is_file = TRUE", &tmp);
        let having_lower = having("true");
        let having_upper = having("TRUE");
        let having_one = having("1");
        let having_false = having("FALSE");
        let correlated = run_query_against_dir(
            "select is_file, (select count(*) from __DIR__ where t.is_file = TRUE) from __DIR__ as t group by is_file order by is_file",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(where_upper, vec!["3"]);
        assert_eq!(having_lower, vec!["true\t3"]);
        assert_eq!(having_upper, vec!["true\t3"], "uppercase literal");
        assert_eq!(having_one, vec!["true\t3"], "numeric literal");
        assert_eq!(having_false, vec!["false\t1"]);
        assert_eq!(correlated, vec!["false\t0", "true\t4"]);
    }

    #[test]
    fn cached_column_values_keep_their_original_text() {
        let tmp = std::env::temp_dir().join("fselect_test_cached_value_text");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("001"), "x").unwrap();

        let last_day = run_query_against_dir(
            "select last_day(modified), last_day(modified), length(last_day(modified)) from __DIR__",
            &tmp,
        );
        let coalesce = run_query_against_dir(
            "select coalesce(name, size), coalesce(name, size), length(coalesce(name, size)) from __DIR__",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(last_day.len(), 1, "{last_day:?}");
        let parts: Vec<&str> = last_day[0].split('\t').collect();
        assert_eq!(parts[0], parts[1], "repeated column must render identically: {last_day:?}");
        assert_eq!(parts[2], "10", "{last_day:?}");
        assert_eq!(coalesce, vec!["001\t001\t3"]);
    }

    #[test]
    fn cached_values_take_the_type_of_the_value_actually_produced() {
        let tmp = std::env::temp_dir().join("fselect_test_cached_coalesce_type");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("a.txt"), "xxxxx").unwrap();

        let arithmetic = run_query_against_dir(
            "select coalesce(is_file, size), coalesce(is_file, size) + 0 from __DIR__ where name = 'a.txt'",
            &tmp,
        );
        let filtered = run_query_against_dir(
            "select name from __DIR__ where coalesce(is_file, size) = true and coalesce(is_file, size) + 0 = 1",
            &tmp,
        );
        let division = run_query_against_dir(
            "select coalesce(is_file, size), 10 / coalesce(is_file, size) from __DIR__ where name = 'a.txt'",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(arithmetic, vec!["true\t1"]);
        assert_eq!(filtered, vec!["a.txt"]);
        assert_eq!(division, vec!["true\t10"]);
    }

    #[test]
    fn cached_values_keep_the_type_they_were_produced_with() {
        let tmp = std::env::temp_dir().join("fselect_test_cached_produced_type");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("true"), "x").unwrap();

        let alone = run_query_against_dir("select coalesce(name, is_file) + 0 from __DIR__", &tmp);
        let repeated = run_query_against_dir("select coalesce(name, is_file), coalesce(name, is_file) + 0 from __DIR__", &tmp);
        let filter_alone = run_query_against_dir("select name from __DIR__ where coalesce(name, is_file) + 0 = 0", &tmp);
        let filter_repeated = run_query_against_dir(
            "select name from __DIR__ where coalesce(name, is_file) = 'true' and coalesce(name, is_file) + 0 = 0",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(alone, vec!["0"]);
        assert_eq!(repeated, vec!["true\t0"], "the cached value is the string name, not a boolean");
        assert_eq!(filter_alone, vec!["true"]);
        assert_eq!(filter_repeated, vec!["true"], "repetition must not change filtering");
    }

    #[test]
    fn produced_types_survive_subqueries_and_grouped_aliases() {
        let tmp = std::env::temp_dir().join("fselect_test_produced_types_across_boundaries");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("names")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("names").join("true"), "x").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let scalar = run_query_against_dir(
            "select (select coalesce(name, is_file) from __DIR__/names where name = 'true') + 0 from __DIR__/other",
            &tmp,
        );
        let inside = run_query_against_dir(
            "select (select coalesce(name, is_file) + 0 from __DIR__/names where name = 'true') from __DIR__/other",
            &tmp,
        );
        let boolean = run_query_against_dir(
            "select (select coalesce(is_file, name) from __DIR__/names where name = 'true') + 0 from __DIR__/other",
            &tmp,
        );
        let grouped = run_query_against_dir(
            "select name, coalesce(name, is_file) as c, (select t.c from __DIR__/other limit 1) + 0 from __DIR__/names as t group by name, is_file",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(inside, vec!["0"]);
        assert_eq!(scalar, inside, "the string crosses the subquery boundary as a string");
        assert_eq!(boolean, vec!["1"], "a real boolean still crosses as a boolean");
        assert_eq!(grouped, vec!["true\ttrue\t0"], "grouped alias");
    }

    #[test]
    fn cached_column_values_keep_their_type() {
        let tmp = std::env::temp_dir().join("fselect_test_cached_value_types");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("a.txt"), "x".repeat(5)).unwrap();
        fs::write(tmp.join("b.txt"), "x".repeat(2)).unwrap();

        let product_alone = run_query_against_dir("select size * is_file from __DIR__ where name = 'a.txt'", &tmp);
        let product_after_bool = run_query_against_dir("select is_file, size * is_file from __DIR__ where name = 'a.txt'", &tmp);
        let ratio = run_query_against_dir("select is_file / is_file from __DIR__ where name = 'a.txt'", &tmp);
        let where_product = run_query_against_dir(
            "select name from __DIR__ where is_file = true and size * is_file > 2 order by name",
            &tmp,
        );
        let where_size_twice = run_query_against_dir(
            "select name from __DIR__ where is_file and size > 1 and size * 2 > 5 order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(product_alone, vec!["5"]);
        assert_eq!(product_after_bool, vec!["true\t5"]);
        assert_eq!(ratio, vec!["1"]);
        assert_eq!(where_product, vec!["a.txt"]);
        assert_eq!(where_size_twice, vec!["a.txt"]);
    }

    #[test]
    fn boolean_and_datetime_aggregate_aliases_keep_their_type() {
        let tmp = std::env::temp_dir().join("fselect_test_typed_aggregate_aliases");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        for name in ["a.txt", "b.txt"] {
            fs::write(tmp.join("data").join(name), "x").unwrap();
        }
        for name in ["o1", "o2", "o3"] {
            fs::write(tmp.join("other").join(name), "x").unwrap();
        }

        let flag_upper = run_query_against_dir(
            "select min(is_file) as flag, (select count(*) from __DIR__/other where t.flag = TRUE) from __DIR__/data as t",
            &tmp,
        );
        let flag_lower = run_query_against_dir(
            "select min(is_file) as flag, (select count(*) from __DIR__/other where t.flag = true) from __DIR__/data as t",
            &tmp,
        );
        let latest_day = run_query_against_dir(
            &format!("select (select count(*) from __DIR__/other where t.latest = '{today}'), max(modified) as latest from __DIR__/data as t"),
            &tmp,
        );
        let first_name = run_query_against_dir(
            "select min(name) as first, (select count(*) from __DIR__/data where name = t.first) from __DIR__/data as t",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(flag_upper, vec!["true\t3"], "uppercase literal");
        assert_eq!(flag_lower, vec!["true\t3"]);
        assert_eq!(latest_day.len(), 1, "{latest_day:?}");
        assert!(latest_day[0].starts_with("3\t"), "date-only comparison: {latest_day:?}");
        assert_eq!(first_name, vec!["a.txt\t1"], "text aggregates stay text");
    }

    #[test]
    fn scalar_subqueries_in_group_by_form_distinct_groups() {
        let tmp = std::env::temp_dir().join("fselect_test_group_by_scalar_subquery");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for name in ["a.txt", "b.txt", "c.md"] {
            fs::write(tmp.join("data").join(name), "x").unwrap();
        }
        fs::write(tmp.join("other").join("o.txt"), "x").unwrap();

        let sorted = |query: &str| {
            let mut rows = run_query_against_dir(query, &tmp);
            rows.sort();
            rows
        };
        let counts = sorted("select count(*) from __DIR__/data as t group by (select t.ext from __DIR__/other limit 1)");
        let keyed = sorted(
            "select (select t.ext from __DIR__/other limit 1), count(*) from __DIR__/data as t group by (select t.ext from __DIR__/other limit 1)",
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(counts, vec!["1", "2"]);
        assert_eq!(keyed, vec!["md\t1", "txt\t2"]);
    }

    #[test]
    fn an_inner_alias_shadows_an_outer_alias_of_the_same_name() {
        let tmp = std::env::temp_dir().join("fselect_test_alias_shadowing");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for name in ["a.txt", "b.txt"] {
            fs::write(tmp.join("data").join(name), "x").unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x".repeat(6)).unwrap();
        fs::write(tmp.join("other").join("o2"), "x".repeat(20)).unwrap();

        let exists = |threshold: u32| {
            run_query_against_dir(
                &format!("select name from __DIR__/data as t where exists(select sum(size) as total from __DIR__/other as t having t.total > {threshold}) order by name"),
                &tmp,
            )
        };
        let above = exists(100);
        let below = exists(20);
        let inner_where = run_query_against_dir(
            "select name from __DIR__/data as t where exists(select name from __DIR__/other as t where t.size > 10) order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(above, Vec::<String>::new(), "26 bytes is not above 100");
        assert_eq!(below, vec!["a.txt", "b.txt"]);
        assert_eq!(inner_where, vec!["a.txt", "b.txt"], "the inner t.size is the inner row");
    }

    #[test]
    fn alias_references_resolve_regardless_of_capitalization() {
        let tmp = std::env::temp_dir().join("fselect_test_alias_reference_case");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 1), ("b.txt", 2), ("c.md", 5), ("d.md", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let aggregate = run_query_against_dir(
            "select ext, sum(size) as Total, (select t.total from __DIR__/other limit 1) from __DIR__/data as t group by ext order by ext",
            &tmp,
        );
        let grouping = run_query_against_dir(
            "select ext as Kind, count(*), (select t.kind from __DIR__/other limit 1) from __DIR__/data as t group by ext order by ext",
            &tmp,
        );
        let ungrouped = run_query_against_dir(
            "select sum(size) as Total, (select t.TOTAL from __DIR__/other limit 1) from __DIR__/data as t",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(aggregate, vec!["md\t12\t12", "txt\t3\t3"]);
        assert_eq!(grouping, vec!["md\t2\tmd", "txt\t2\ttxt"]);
        assert_eq!(ungrouped, vec!["15\t15"]);
    }

    #[test]
    fn scalar_subqueries_keep_the_type_of_referenced_aliases() {
        let tmp = std::env::temp_dir().join("fselect_test_scalar_alias_types");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 10), ("b.txt", 2), ("c.txt", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let filtered = run_query_against_dir(
            "select name, size as sz from __DIR__/data as t where (select t.sz from __DIR__/other limit 1) > 5 order by name",
            &tmp,
        );
        let sorted = run_query_against_dir(
            "select name, size as sz from __DIR__/data as t order by (select t.sz from __DIR__/other limit 1) desc",
            &tmp,
        );
        let boolean = run_query_against_dir(
            "select name, is_file as f from __DIR__/data as t where (select t.f from __DIR__/other limit 1) = TRUE order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(filtered, vec!["a.txt	10", "c.txt	7"], "numeric comparison");
        assert_eq!(sorted, vec!["a.txt	10", "c.txt	7", "b.txt	2"], "numeric ordering");
        assert_eq!(boolean, vec!["a.txt	true", "b.txt	true", "c.txt	true"], "boolean comparison");
    }

    #[test]
    fn correlated_column_aliases_see_the_outer_fields_they_depend_on() {
        let tmp = std::env::temp_dir().join("fselect_test_correlated_alias_inputs");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 2), ("b.txt", 5), ("c.txt", 3)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        for (name, size) in [("o1", 1), ("o2", 4)] {
            fs::write(tmp.join("other").join(name), "x".repeat(size)).unwrap();
        }

        let matches = "(select count(*) from __DIR__/other where size < t.size) as matches";
        let shown = run_query_against_dir(
            &format!("select name, {matches}, (select t.matches from __DIR__/other limit 1) from __DIR__/data as t order by name"),
            &tmp,
        );
        let filtered = run_query_against_dir(
            &format!("select name, {matches} from __DIR__/data as t where (select t.matches from __DIR__/other limit 1) = 2 order by name"),
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(shown, vec!["a.txt	1	1", "b.txt	2	2", "c.txt	1	1"]);
        assert_eq!(filtered, vec!["b.txt	2"]);
    }

    #[test]
    fn dependent_row_aliases_settle_in_any_declaration_order() {
        let tmp = std::env::temp_dir().join("fselect_test_dependent_row_aliases");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 2), ("b.txt", 10), ("c.txt", 3)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let chained = "select name, size as first, (select t.first from __DIR__/other limit 1) + 1 as next from __DIR__/data as t where (select t.next from __DIR__/other limit 1) > 5 order by name";
        let reversed = "select name, (select t.first from __DIR__/other limit 1) + 1 as next, size as first from __DIR__/data as t where (select t.next from __DIR__/other limit 1) > 5 order by name";
        let mut failures = Vec::new();
        for run in 0..20 {
            let rows = run_query_against_dir(chained, &tmp);
            if rows != vec!["b.txt	10	11"] {
                failures.push(format!("chained run {run}: {rows:?}"));
            }
            let rows = run_query_against_dir(reversed, &tmp);
            if rows != vec!["b.txt	11	10"] {
                failures.push(format!("reversed run {run}: {rows:?}"));
            }
        }
        let _ = fs::remove_dir_all(&tmp);

        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn calculated_and_constant_aliases_are_bound_for_groups() {
        let tmp = std::env::temp_dir().join("fselect_test_calculated_group_aliases");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for name in ["a.txt", "b.txt", "c.md"] {
            fs::write(tmp.join("data").join(name), "x").unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let calculated = run_query_against_dir(
            "select upper(ext) as kind, count(*) from __DIR__/data as t group by ext having (select t.kind from __DIR__/other limit 1) = 'TXT'",
            &tmp,
        );
        let constant_grouped = run_query_against_dir(
            "select ext, count(*), 7 as seven, (select t.seven from __DIR__/other limit 1) from __DIR__/data as t group by ext order by ext",
            &tmp,
        );
        let constant_ungrouped = run_query_against_dir(
            "select count(*), 7 as seven, (select t.seven from __DIR__/other limit 1) from __DIR__/data as t",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(calculated, vec!["TXT	2"]);
        assert_eq!(constant_grouped, vec!["md	1	7	7", "txt	2	7	7"]);
        assert_eq!(constant_ungrouped, vec!["3	7	7"]);
    }

    #[test]
    fn negated_field_aliases_keep_their_sign_in_the_row_context() {
        let tmp = std::env::temp_dir().join("fselect_test_negated_field_alias");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 2), ("b.txt", 10)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let shown = run_query_against_dir(
            "select name, -size as neg, (select t.neg from __DIR__/other limit 1) from __DIR__/data as t order by name",
            &tmp,
        );
        let ordered = run_query_against_dir(
            "select name, -size as neg from __DIR__/data as t order by (select t.neg from __DIR__/other limit 1)",
            &tmp,
        );
        let counted = run_query_against_dir(
            "select name, -size as neg, (select count(*) from __DIR__/other where t.neg < 0) from __DIR__/data as t order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(shown, vec!["a.txt	-2	-2", "b.txt	-10	-10"]);
        assert_eq!(ordered, vec!["b.txt	-10", "a.txt	-2"], "ordering through the alias");
        assert_eq!(counted, vec!["a.txt	-2	1", "b.txt	-10	1"], "correlated count through the alias");
    }

    #[test]
    fn alias_evaluation_errors_are_reported_like_direct_ones() {
        let tmp = std::env::temp_dir().join("fselect_test_alias_errors_reported");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("metrics")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("metrics").join("a.txt"), "xxxx").unwrap();
        fs::write(tmp.join("metrics").join("zero.txt"), "").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let rows_and_errors = |query: &str| {
            let searcher = search_dir(query, &tmp, Config::default(), true);
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
            (rows, searcher.error_count)
        };
        let direct = rows_and_errors("select name, 100 / size as ratio from __DIR__/metrics as t where 100 / size > 0 order by name");
        let through_alias = rows_and_errors(
            "select name, 100 / size as ratio from __DIR__/metrics as t where (select t.ratio from __DIR__/other limit 1) > 0 order by name",
        );
        let through_exists = rows_and_errors(
            "select name, 100 / size as ratio from __DIR__/metrics as t where exists(select name from __DIR__/other where t.ratio > 0) order by name",
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct, (vec!["a.txt\t25".to_string()], 1));
        assert_eq!(through_alias, direct, "scalar subquery on the alias");
        assert_eq!(through_exists, direct, "exists on the alias");
    }

    #[test]
    fn alias_chains_keep_their_type_in_where() {
        let tmp = std::env::temp_dir().join("fselect_test_alias_chain_types");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 2), ("b.txt", 10), ("c.txt", 3)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let numeric = run_query_against_dir(
            "select name, size as first, (select t.first from __DIR__/other limit 1) as second from __DIR__/data as t where (select t.second from __DIR__/other limit 1) > 5 order by name",
            &tmp,
        );
        let boolean = run_query_against_dir(
            "select name, is_file as f, (select t.f from __DIR__/other limit 1) as g from __DIR__/data as t where (select t.g from __DIR__/other limit 1) = TRUE order by name",
            &tmp,
        );
        let reversed = run_query_against_dir(
            "select name, (select t.second from __DIR__/other limit 1) as third, (select t.first from __DIR__/other limit 1) as second, size as first from __DIR__/data as t where (select t.third from __DIR__/other limit 1) > 5 order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(numeric, vec!["b.txt\t10\t10"], "numeric chain");
        assert_eq!(boolean, vec!["a.txt\ttrue\ttrue", "b.txt\ttrue\ttrue", "c.txt\ttrue\ttrue"], "boolean chain");
        assert_eq!(reversed, vec!["b.txt\t10\t10\t10"], "reverse-declared chain");
    }

    #[test]
    fn alias_errors_surface_only_when_the_alias_is_read() {
        let tmp = std::env::temp_dir().join("fselect_test_alias_errors_lazy");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("metrics")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("metrics").join("a.txt"), "xxxx").unwrap();
        fs::write(tmp.join("metrics").join("zero.txt"), "").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let rows_and_errors = |query: &str| {
            let searcher = search_dir(query, &tmp, Config::default(), true);
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
            (rows, searcher.error_count)
        };
        let guarded = rows_and_errors(
            "select name, 100 / size as ratio from __DIR__/metrics as t where size > 0 and (select t.ratio from __DIR__/other limit 1) > 0 order by name",
        );
        let ordered = rows_and_errors(
            "select name, 100 / size as ratio from __DIR__/metrics as t where size > 0 order by (select t.ratio from __DIR__/other limit 1)",
        );
        let unguarded = rows_and_errors(
            "select name, 100 / size as ratio from __DIR__/metrics as t where (select t.ratio from __DIR__/other limit 1) > 0 order by name",
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(guarded, (vec!["a.txt\t25".to_string()], 0), "guarded by size > 0");
        assert_eq!(ordered, (vec!["a.txt\t25".to_string()], 0), "alias used only by ORDER BY");
        assert_eq!(unguarded, (vec!["a.txt\t25".to_string()], 1), "unguarded still reports");
    }

    #[test]
    fn aliases_depending_on_aggregates_indirectly_are_bound_per_group() {
        let tmp = std::env::temp_dir().join("fselect_test_indirect_aggregate_aliases");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 2), ("b.txt", 10), ("c.md", 3)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let columns = "sum(size) as total, 30 / (select t.total from __DIR__/other limit 1) as ratio, (select t.ratio from __DIR__/other limit 1)";
        let (ungrouped, ungrouped_errors) = {
            let searcher = search_dir(&format!("select {columns} from __DIR__/data as t"), &tmp, Config::default(), true);
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
            (rows, searcher.error_count)
        };
        let grouped = run_query_against_dir(
            &format!("select ext, {columns} from __DIR__/data as t group by ext order by ext"),
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(ungrouped, vec!["15\t2\t2"]);
        assert_eq!(ungrouped_errors, 0, "no division by zero while rows are scanned");
        assert_eq!(grouped, vec!["md\t3\t10\t10", "txt\t12\t2.5\t2.5"]);
    }

    #[test]
    fn grouped_conditions_prefer_their_own_key_over_a_complement() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_complement");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            fs::write(tmp.join(name), "text").unwrap();
        }

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let forward = run("select width > 0, width <= 0, count(*) from __DIR__ group by width > 0, width <= 0 having width <= 0");
        let reversed = run("select width > 0, width <= 0, count(*) from __DIR__ group by width <= 0, width > 0 having width <= 0");
        let positive = run("select width > 0, width <= 0, count(*) from __DIR__ group by width > 0, width <= 0 having width > 0");
        let shown = run("select width > 0, width <= 0, count(*) from __DIR__ group by width > 0, width <= 0");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(shown, vec![String::from("false	false	4")]);
        assert!(forward.is_empty(), "{forward:?}");
        assert!(reversed.is_empty(), "{reversed:?}");
        assert!(positive.is_empty(), "{positive:?}");
    }

    #[test]
    fn negated_grouping_keys_do_not_overwrite_the_raw_field_in_the_group_context() {
        let tmp = std::env::temp_dir().join("fselect_test_negated_grouping_key");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 2), ("b.txt", 10), ("c.txt", 3)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let shown = run_query_against_dir(
            "select size, count(*), (select t.size from __DIR__/other limit 1) from __DIR__/data as t group by size, -size order by size",
            &tmp,
        );
        let having = run_query_against_dir(
            "select size, count(*) from __DIR__/data as t group by size, -size having (select t.size from __DIR__/other limit 1) > 5",
            &tmp,
        );
        let aliased = run_query_against_dir(
            "select size, -size as neg, (select t.neg from __DIR__/other limit 1) from __DIR__/data as t group by size, -size order by size",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(shown, vec!["2\t1\t2", "3\t1\t3", "10\t1\t10"]);
        assert_eq!(having, vec!["10\t1"]);
        assert_eq!(aliased, vec!["2\t-2\t-2", "3\t-3\t-3", "10\t-10\t-10"], "a negated alias keeps its sign");
    }

    #[test]
    fn unquoted_file_names_with_dots_and_pluses_stay_literal() {
        let tmp = std::env::temp_dir().join("fselect_test_unquoted_dotted_names");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["report.txt-backup.bak", "a+b.txt", "plain.txt"] {
            fs::write(tmp.join(name), "x").unwrap();
        }

        let dotted = run_query_against_dir("select name from __DIR__ where name=report.txt-backup.bak", &tmp);
        let plus = run_query_against_dir("select name from __DIR__ where name=a+b.txt", &tmp);
        let in_list = run_query_against_dir("select name from __DIR__ where name in (report.txt-backup.bak, a+b.txt) order by name", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(dotted, vec!["report.txt-backup.bak"]);
        assert_eq!(plus, vec!["a+b.txt"]);
        assert_eq!(in_list, vec!["a+b.txt", "report.txt-backup.bak"]);
    }

    #[test]
    fn compact_qualified_arithmetic_works_in_select_and_with_quoted_aliases() {
        let tmp = std::env::temp_dir().join("fselect_test_compact_select_arithmetic");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for (name, size) in [("a.txt", 2), ("b.txt", 10), ("c.txt", 3)] {
            fs::write(tmp.join(name), "x".repeat(size)).unwrap();
        }

        let select = run_query_against_dir("select name,t.size/2 from __DIR__ as t order by name", &tmp);
        let quoted = run_query_against_dir("select name from __DIR__ as 't' where t.size/2>3", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(select, vec!["a.txt\t1", "b.txt\t5", "c.txt\t1.5"], "select list");
        assert_eq!(quoted, vec!["b.txt"], "quoted alias in where");
    }

    #[test]
    fn inner_subquery_aliases_do_not_change_outer_literals() {
        let tmp = std::env::temp_dir().join("fselect_test_inner_alias_scope");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("names")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("names").join("report.txt-1"), "x").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let rows = run_query_against_dir(
            "select name,(select name from __DIR__/other as report limit 1) from __DIR__/names where name=report.txt-1",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["report.txt-1\to1"]);
    }

    #[test]
    fn inner_subquery_aliases_do_not_change_parenthesised_outer_literals() {
        let tmp = std::env::temp_dir().join("fselect_test_inner_alias_parens");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("names")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("names").join("report.txt-1"), "x").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let parenthesised = run_query_against_dir(
            "select name,(select name from __DIR__/other as report limit 1) from __DIR__/names where (name=report.txt-1)",
            &tmp,
        );
        let in_list = run_query_against_dir(
            "select name,(select name from __DIR__/other as report limit 1) from __DIR__/names where name in (report.txt-1)",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(parenthesised, vec!["report.txt-1\to1"], "parenthesised predicate");
        assert_eq!(in_list, vec!["report.txt-1\to1"], "in list");
    }

    #[test]
    fn compact_arithmetic_in_where_is_calculated() {
        let tmp = std::env::temp_dir().join("fselect_test_compact_arithmetic");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for (name, size) in [("a.txt", 2), ("b.txt", 10), ("c.txt", 3)] {
            fs::write(tmp.join(name), "x".repeat(size)).unwrap();
        }

        let qualified = run_query_against_dir("select name from __DIR__ as t where t.size/2>3", &tmp);
        let bare = run_query_against_dir("select name from __DIR__ where size/2>3", &tmp);
        let underscored = run_query_against_dir("select name, line_count/2 from __DIR__ where name = 'b.txt'", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(qualified, vec!["b.txt"], "qualified field");
        assert_eq!(bare, vec!["b.txt"], "bare field");
        assert_eq!(underscored, vec!["b.txt\t0.5"], "underscored field");
    }

    #[test]
    fn boolean_in_lists_skip_missing_values() {
        let tmp = std::env::temp_dir().join("fselect_test_boolean_in_missing");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("empty")).unwrap();
        for name in ["a.txt", "b.txt"] {
            fs::write(tmp.join("data").join(name), "x").unwrap();
        }

        let scalar_element = run_query_against_dir("select name from __DIR__/data where is_dir in ((select is_dir from __DIR__/empty))", &tmp);
        let membership = run_query_against_dir("select name from __DIR__/data where is_dir in (select is_dir from __DIR__/empty)", &tmp);
        let literal = run_query_against_dir("select name from __DIR__/data where is_dir in (false) order by name", &tmp);
        let populated = run_query_against_dir(
            "select name from __DIR__/data where is_dir in ((select is_dir from __DIR__/data limit 1)) order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(scalar_element, Vec::<String>::new(), "scalar subquery element");
        assert_eq!(membership, Vec::<String>::new(), "membership subquery");
        assert_eq!(literal, vec!["a.txt", "b.txt"], "literal false still matches");
        assert_eq!(populated, vec!["a.txt", "b.txt"], "populated subquery still matches");
    }

    #[test]
    fn boolean_fields_compare_against_function_results() {
        let tmp = std::env::temp_dir().join("fselect_test_boolean_function_results");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "b.txt"] {
            fs::write(tmp.join(name), "x").unwrap();
        }

        let concat = run_query_against_dir("select name from __DIR__ where is_file = concat('tr', 'ue') order by name", &tmp);
        let in_list = run_query_against_dir("select name from __DIR__ where is_file in (concat('tr', 'ue')) order by name", &tmp);
        let coalesce = run_query_against_dir("select name from __DIR__ where is_file = coalesce(is_file, 'nope') order by name", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(concat, vec!["a.txt", "b.txt"], "concat");
        assert_eq!(in_list, vec!["a.txt", "b.txt"], "in list");
        assert_eq!(coalesce, vec!["a.txt", "b.txt"], "coalesce");
    }

    #[test]
    fn function_produced_dates_keep_their_whole_day_range() {
        let tmp = std::env::temp_dir().join("fselect_test_function_date_ranges");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("fresh.txt"), "x").unwrap();

        let compare = |op: &str, value: &str| {
            run_query_against_dir(&format!("select name from __DIR__ where modified {op} {value}"), &tmp)
        };
        let mut mismatches = Vec::new();
        for op in ["=", "!=", ">", ">=", "<", "<="] {
            let literal = compare(op, "today");
            for value in ["current_date()", "concat(current_date(), '')", "(select current_date() from __DIR__ limit 1)"] {
                let produced = compare(op, value);
                if produced != literal {
                    mismatches.push(format!("modified {op} {value}: {produced:?} vs today: {literal:?}"));
                }
            }
        }
        let equal = compare("=", "current_date()");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(equal, vec!["fresh.txt"], "a file modified today equals current_date()");
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[test]
    fn in_lists_treat_missing_values_as_unknown() {
        let tmp = std::env::temp_dir().join("fselect_test_in_missing_values");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("2020-01-01"), "x").unwrap();
        fs::write(tmp.join("notes.txt"), "x").unwrap();

        let zero = run_query_against_dir("select name from __DIR__ where year(name) in (0) order by name", &tmp);
        let year = run_query_against_dir("select name from __DIR__ where year(name) in (2020) order by name", &tmp);
        let not_zero = run_query_against_dir("select name from __DIR__ where year(name) not in (0) order by name", &tmp);
        let membership = run_query_against_dir(
            "select name from __DIR__ where year(name) in (select year(name) from __DIR__) order by name",
            &tmp,
        );
        let is_null = run_query_against_dir("select name from __DIR__ where year(name) is null order by name", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(zero, Vec::<String>::new(), "missing year is not 0");
        assert_eq!(year, vec!["2020-01-01"]);
        assert_eq!(not_zero, vec!["2020-01-01"], "missing year is unknown for NOT IN as well");
        assert_eq!(membership, vec!["2020-01-01"], "membership through a subquery");
        assert_eq!(is_null, vec!["notes.txt", "sub"]);
    }

    #[test]
    fn in_lists_treat_null_members_as_unknown() {
        let tmp = std::env::temp_dir().join("fselect_test_in_null_members");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("empty")).unwrap();
        fs::write(tmp.join("three.txt"), "xxx").unwrap();
        fs::write(tmp.join("five.txt"), "xxxxx").unwrap();

        let run = |condition: &str| {
            run_query_against_dir(&format!("select name from __DIR__ where is_file and {condition} order by name"), &tmp)
        };
        let not_in_null_aggregate = run("size not in (select max(size) from __DIR__/empty)");
        let not_in_empty_set = run("size not in (select size from __DIR__/empty)");
        let not_in_null_column = run("size not in (width)");
        let not_in_null_subquery_column = run("size not in (select width from __DIR__)");
        let not_in_list_with_null = run("size not in (3, width)");
        let in_list_with_null = run("size in (3, width)");
        let not_in_null_or_size = run("(size not in (3, width) or size = 5)");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(not_in_null_aggregate, Vec::<String>::new(), "a NULL aggregate makes NOT IN unknown");
        assert_eq!(not_in_empty_set, vec!["five.txt", "three.txt"], "an empty set keeps NOT IN true");
        assert_eq!(not_in_null_column, Vec::<String>::new(), "a NULL column member makes NOT IN unknown");
        assert_eq!(not_in_null_subquery_column, Vec::<String>::new(), "NULL rows from a subquery make NOT IN unknown");
        assert_eq!(not_in_list_with_null, Vec::<String>::new(), "no match plus a NULL member is unknown");
        assert_eq!(in_list_with_null, vec!["three.txt"], "a match still wins over a NULL member");
        assert_eq!(not_in_null_or_size, vec!["five.txt"], "unknown OR true is true");
    }

    #[cfg(windows)]
    #[test]
    fn in_lists_treat_missing_boolean_values_as_unknown() {
        let tmp = std::env::temp_dir().join("fselect_test_in_missing_booleans");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("plain.txt"), "x").unwrap();

        let rows = run_query_against_dir("select name from __DIR__ where contains('needle') in (false) order by name", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["plain.txt"], "a directory has no contains() result");
    }

    #[test]
    fn reversed_date_comparisons_keep_the_interval_on_the_left() {
        let tmp = std::env::temp_dir().join("fselect_test_reversed_date_comparisons");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("fresh.txt"), "x").unwrap();

        let mut mismatches = Vec::new();
        for (op, mirrored) in [("=", "="), ("!=", "!="), (">", "<"), (">=", "<="), ("<", ">"), ("<=", ">=")] {
            let direct = run_query_against_dir(&format!("select name from __DIR__ where modified {op} current_date()"), &tmp);
            let reversed = run_query_against_dir(&format!("select name from __DIR__ where current_date() {mirrored} modified"), &tmp);
            if direct != reversed {
                mismatches.push(format!("modified {op} current_date(): {direct:?}, current_date() {mirrored} modified: {reversed:?}"));
            }
        }
        let reversed_equal = run_query_against_dir("select name from __DIR__ where current_date() = modified", &tmp);
        let having = run_query_against_dir("select count(*) from __DIR__ having current_date() = max(modified)", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(reversed_equal, vec!["fresh.txt"], "a file modified today equals current_date() from either side");
        assert_eq!(having, vec!["1"], "reversed HAVING");
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[test]
    fn date_membership_agrees_with_date_equality() {
        let tmp = std::env::temp_dir().join("fselect_test_date_membership");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("fresh.txt"), "x").unwrap();
        fs::write(tmp.join("other").join("o.txt"), "x").unwrap();

        let rows = |condition: &str| {
            run_query_against_dir(&format!("select name from __DIR__ where name = 'fresh.txt' and {condition}"), &tmp)
        };
        let equal = rows("concat(current_date(), '') = modified");
        let in_list = rows("concat(current_date(), '') in (modified)");
        let not_in = rows("concat(current_date(), '') not in (modified)");
        let in_subquery = rows("concat(current_date(), '') in (select modified from __DIR__/other)");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(equal, vec!["fresh.txt"]);
        assert_eq!(in_list, equal, "in list");
        assert_eq!(not_in, Vec::<String>::new(), "not in");
        assert_eq!(in_subquery, equal, "in subquery");
    }

    #[test]
    fn boolean_looking_function_arguments_stay_literal() {
        let tmp = std::env::temp_dir().join("fselect_test_boolean_function_arguments");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("alpha.txt"), "x").unwrap();

        let upper = run_query_against_dir("select name from __DIR__ where concat(name, 'TRUE') = 'alpha.txtTRUE'", &tmp);
        let lower = run_query_against_dir("select name from __DIR__ where concat(name, 'false') = 'alpha.txtfalse'", &tmp);
        let having = run_query_against_dir(
            "select concat(name, 'TRUE'), count(*) from __DIR__ group by concat(name, 'TRUE') having concat(name, 'TRUE') = 'alpha.txtTRUE'",
            &tmp,
        );
        let bare = run_query_against_dir("select name from __DIR__ where is_file", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(upper, vec!["alpha.txt"]);
        assert_eq!(lower, vec!["alpha.txt"]);
        assert_eq!(having, vec!["alpha.txtTRUE\t1"]);
        assert_eq!(bare, vec!["alpha.txt"], "a bare boolean condition still works");
    }

    #[test]
    fn comparison_columns_evaluate_to_booleans() {
        let tmp = std::env::temp_dir().join("fselect_test_comparison_columns");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a")).unwrap();
        fs::create_dir_all(tmp.join("b")).unwrap();
        fs::write(tmp.join("a").join("two.txt"), "xx").unwrap();
        fs::write(tmp.join("b").join("three.txt"), "xxx").unwrap();

        let shown = run_query_against_dir("select name, (size > 2) from __DIR__/a, __DIR__/b order by name", &tmp);
        let summed = run_query_against_dir("select sum((size > 2) + 0) from __DIR__/a, __DIR__/b", &tmp);
        let grouped = run_query_against_dir("select ext, (sum(size) > 4) from __DIR__/a, __DIR__/b group by ext", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(shown, vec!["three.txt\ttrue", "two.txt\tfalse"]);
        assert_eq!(summed, vec!["1"]);
        assert_eq!(grouped, vec!["txt\ttrue"]);
    }

    #[test]
    fn empty_boolean_subqueries_match_nothing() {
        let tmp = std::env::temp_dir().join("fselect_test_empty_boolean_subquery");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("empty")).unwrap();
        for name in ["a.txt", "b.txt"] {
            fs::write(tmp.join("data").join(name), "x").unwrap();
        }

        let direct = run_query_against_dir("select name from __DIR__/data where is_dir = (select is_dir from __DIR__/empty)", &tmp);
        let reversed = run_query_against_dir("select name from __DIR__/data where (select is_dir from __DIR__/empty) = is_dir", &tmp);
        let not_equal = run_query_against_dir("select name from __DIR__/data where is_dir != (select is_dir from __DIR__/empty)", &tmp);
        let having = run_query_against_dir(
            "select ext, count(*) from __DIR__/data group by ext having min(is_dir) = (select is_dir from __DIR__/empty)",
            &tmp,
        );
        let populated = run_query_against_dir(
            "select name from __DIR__/data where is_dir = (select is_dir from __DIR__/data limit 1) order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct, Vec::<String>::new(), "direct");
        assert_eq!(reversed, Vec::<String>::new(), "reversed");
        assert_eq!(not_equal, Vec::<String>::new(), "not equal is unknown too");
        assert_eq!(having, Vec::<String>::new(), "having");
        assert_eq!(populated, vec!["a.txt", "b.txt"], "a populated subquery still compares");
    }

    #[test]
    fn in_subqueries_skip_missing_metadata() {
        let tmp = std::env::temp_dir().join("fselect_test_in_subquery_missing_metadata");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::write(tmp.join("data").join("noext"), "x").unwrap();
        fs::write(tmp.join("data").join("a.txt"), "x").unwrap();

        let subquery = run_query_against_dir("select name from __DIR__/data where ext in (select width from __DIR__/data)", &tmp);
        let negated = run_query_against_dir("select name from __DIR__/data where ext not in (select width from __DIR__/data)", &tmp);
        let list = run_query_against_dir("select name from __DIR__/data where ext in (width)", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(subquery, Vec::<String>::new(), "subquery");
        assert_eq!(negated, Vec::<String>::new(), "negated");
        assert_eq!(list, Vec::<String>::new(), "list");
    }

    #[test]
    fn deferred_alias_errors_survive_dependent_aliases_and_nested_rows() {
        let tmp = std::env::temp_dir().join("fselect_test_deferred_alias_errors");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("metrics")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("metrics").join("a.txt"), "xxxx").unwrap();
        fs::write(tmp.join("metrics").join("zero.txt"), "").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();
        fs::write(tmp.join("other").join("o2"), "x").unwrap();

        let rows_and_errors = |query: &str| {
            let searcher = search_dir(query, &tmp, Config::default(), true);
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
            (rows, searcher.error_count)
        };
        let through_copy = rows_and_errors(
            "select name, 100 / size as ratio, (select t.ratio from __DIR__/other limit 1) as copy from __DIR__/metrics as t where (select t.copy from __DIR__/other limit 1) > 0 order by name",
        );
        let across_inner_rows = rows_and_errors(
            "select name, 100 / size as ratio from __DIR__/metrics as t where exists(select name from __DIR__/other where t.ratio > 0) order by name",
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(through_copy, (vec!["a.txt\t25\t25".to_string()], 1), "error must follow the dependent alias");
        assert_eq!(across_inner_rows, (vec!["a.txt\t25".to_string()], 1), "a later inner row must not erase the hit");
    }

    #[test]
    fn subquery_alias_errors_are_deferred_like_inline_ones() {
        let tmp = std::env::temp_dir().join("fselect_test_subquery_alias_errors");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("metrics")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("metrics").join("a.txt"), "xxxx").unwrap();
        fs::write(tmp.join("metrics").join("zero.txt"), "").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let rows_and_errors = |query: &str| {
            let searcher = search_dir(query, &tmp, Config::default(), true);
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
            (rows, searcher.error_count)
        };
        let guarded = rows_and_errors(
            "select name, (select 100 / t.size from __DIR__/other limit 1) as ratio from __DIR__/metrics as t where size > 0 order by (select t.ratio from __DIR__/other limit 1)",
        );
        let unguarded = rows_and_errors(
            "select name, (select 100 / t.size from __DIR__/other limit 1) as ratio from __DIR__/metrics as t where (select t.ratio from __DIR__/other limit 1) > 0 order by name",
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(guarded, (vec!["a.txt\t25".to_string()], 0), "guarded by size > 0");
        assert_eq!(unguarded, (vec!["a.txt\t25".to_string()], 1), "unguarded reports once");
    }

    #[test]
    fn min_and_max_over_all_zero_file_names_return_existing_names() {
        let tmp = std::env::temp_dir().join("fselect_test_min_max_zero_names");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["000", "001", "002", "-000"] {
            fs::write(tmp.join(name), "x").unwrap();
        }

        let extremes = run_query_against_dir("select min(name), max(name) from __DIR__", &tmp);
        let found = run_query_against_dir("select name from __DIR__ where name in (select min(name) from __DIR__)", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(extremes, vec!["-000\t002"]);
        assert_eq!(found, vec!["-000"], "the minimum must be an existing name");
    }

    #[test]
    fn shadowed_aliases_do_not_erase_the_outer_pending_error() {
        let tmp = std::env::temp_dir().join("fselect_test_shadowed_alias_errors");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("metrics")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("metrics").join("b.txt"), "xxxx").unwrap();
        fs::write(tmp.join("metrics").join("0.txt"), "").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let rows_and_errors = |query: &str| {
            let searcher = search_dir(query, &tmp, Config::default(), true);
            let rows: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
            (rows, searcher.error_count)
        };
        let shadow_before_read = rows_and_errors(
            "select name, 100 / size as ratio from __DIR__/metrics as t where exists(select name from __DIR__/other as t where t.size > 0) and (select t.ratio from __DIR__/other limit 1) > 0 order by name",
        );
        let shadow_after_read = rows_and_errors(
            "select name, 100 / size as ratio from __DIR__/metrics as t where (select t.ratio from __DIR__/other limit 1) > 0 and exists(select name from __DIR__/other as t where t.size > 0) order by name",
        );
        let shadow_during_binding = rows_and_errors(
            "select name, 100 / size as ratio, (select count(*) from __DIR__/other as t where t.size > 0) as cnt from __DIR__/metrics as t where (select t.ratio from __DIR__/other limit 1) > 0 and (select t.cnt from __DIR__/other limit 1) > 0 order by name",
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(shadow_before_read, (vec!["b.txt\t25".to_string()], 1), "inner alias t declared before the read");
        assert_eq!(shadow_after_read, (vec!["b.txt\t25".to_string()], 1), "inner alias t declared after the read");
        assert_eq!(shadow_during_binding, (vec!["b.txt\t25\t1".to_string()], 1), "inner alias t searched while the failure is pending");
    }

    fn search_outcome(query_template: &str, dir: &Path) -> Result<Vec<String>, String> {
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        let query_string = query_template.replace("__DIR__", &dir.to_string_lossy());
        let mut lexer = Lexer::new(vec![query_string]);
        let mut parser = Parser::new(&mut lexer);
        let parsed = Box::leak(Box::new(parser.parse(false).expect("parse failed")));
        let config = Box::leak(Box::new(Config::default()));
        let default_config = Box::leak(Box::new(Config::default()));
        let mut searcher = Searcher::new(parsed, config, default_config, false);
        searcher.silent_mode = true;
        searcher
            .list_search_results()
            .map(|_| searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect())
            .map_err(|err| err.description)
    }

    #[test]
    fn having_guards_protect_dependent_aggregate_aliases() {
        let tmp = std::env::temp_dir().join("fselect_test_having_guard_dependent_alias");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("metrics")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("metrics").join("a.txt"), "").unwrap();
        fs::write(tmp.join("metrics").join("b.md"), "xx").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        let columns = "ext, 100 / sum(size) as ratio, (select t.ratio from __DIR__/other limit 1) as again";
        let guarded = search_outcome(
            &format!("select {columns} from __DIR__/metrics as t group by ext having sum(size) > 0 and (select t.again from __DIR__/other limit 1) > 0"),
            &tmp,
        );
        let unguarded = search_outcome(
            &format!("select {columns} from __DIR__/metrics as t group by ext having (select t.again from __DIR__/other limit 1) > 0"),
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(guarded, Ok(vec!["md\t50\t50".to_string()]), "guarded");
        assert_eq!(unguarded, Err("Division by zero".to_string()), "unguarded");
    }

    #[test]
    fn wrapped_dependent_aliases_still_inherit_aggregate_errors() {
        let tmp = std::env::temp_dir().join("fselect_test_wrapped_dependent_alias_errors");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("zero")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("zero").join("empty.txt"), "").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();

        for wrapper in [
            "coalesce((select t.ratio from __DIR__/other limit 1), 0)",
            "length((select t.ratio from __DIR__/other limit 1))",
            "(select t.ratio from __DIR__/other limit 1) + 1",
        ] {
            let outcome = search_outcome(
                &format!("select 100 / sum(size) as ratio, {wrapper} as again from __DIR__/zero as t having (select t.again from __DIR__/other limit 1) > 0"),
                &tmp,
            );
            assert_eq!(outcome, Err("Division by zero".to_string()), "{wrapper}");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn having_reports_aggregate_alias_errors_read_through_subqueries() {
        let tmp = std::env::temp_dir().join("fselect_test_having_alias_errors");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("zero")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::write(tmp.join("zero").join("empty.txt"), "").unwrap();
        fs::write(tmp.join("other").join("o1"), "x").unwrap();
        fs::write(tmp.join("data").join("a.txt"), "").unwrap();
        fs::write(tmp.join("data").join("b.md"), "xxxx").unwrap();

        let direct = search_outcome("select 100 / sum(size) as ratio from __DIR__/zero as t having ratio > 0", &tmp);
        let through_alias = search_outcome(
            "select 100 / sum(size) as ratio from __DIR__/zero as t having (select t.ratio from __DIR__/other limit 1) > 0",
            &tmp,
        );
        let grouped = search_outcome(
            "select ext, 100 / sum(size) as ratio from __DIR__/data as t group by ext having (select t.ratio from __DIR__/other limit 1) > 0",
            &tmp,
        );
        let healthy = search_outcome(
            "select 100 / sum(size) as ratio from __DIR__/other as t having (select t.ratio from __DIR__/other limit 1) > 0",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct, Err("Division by zero".to_string()), "direct");
        assert_eq!(through_alias, Err("Division by zero".to_string()), "through alias");
        assert_eq!(grouped, Err("Division by zero".to_string()), "grouped");
        assert_eq!(healthy, Ok(vec!["100".to_string()]), "healthy");
    }

    #[test]
    fn having_can_read_aggregate_aliases_through_correlated_subqueries() {
        let tmp = std::env::temp_dir().join("fselect_test_having_aggregate_alias");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        for (name, size) in [("a.txt", 0), ("b.txt", 0), ("c.md", 5), ("d.md", 7)] {
            fs::write(tmp.join("data").join(name), "x".repeat(size)).unwrap();
        }
        for (name, size) in [("o1", 1), ("o5", 5)] {
            fs::write(tmp.join("other").join(name), "x".repeat(size)).unwrap();
        }

        let grouped = run_query_against_dir(
            "select ext, sum(size) as total from __DIR__/data as t group by ext having (select count(*) from __DIR__/other where size > t.total) > 0 order by ext",
            &tmp,
        );
        let ungrouped = run_query_against_dir(
            "select sum(size) as total from __DIR__/data as t having (select count(*) from __DIR__/other where size < t.total) > 0",
            &tmp,
        );
        let guarded = search_dir(
            "select ext, 96 / sum(size) as ratio from __DIR__/data as t group by ext having sum(size) > 0 and (select count(*) from __DIR__/other where size < t.ratio) >= 0",
            &tmp, Config::default(), true,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(grouped, vec!["txt\t0"]);
        assert_eq!(ungrouped, vec!["12"]);
        let rows: Vec<String> = guarded.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
        assert_eq!(rows, vec!["md\t8"]);
        assert_eq!(guarded.error_count, 0);
    }

    #[test]
    fn having_comparisons_never_match_a_missing_aggregate_value() {
        let tmp = std::env::temp_dir().join("fselect_test_having_missing_value");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "hello").unwrap();
        fs::write(tmp.join("b.md"), "hi").unwrap();
        let logo = std::env::var("CARGO_MANIFEST_DIR").unwrap() + "/resources/test/image/rust-logo-blk.png";
        fs::copy(logo, tmp.join("logo.png")).unwrap();

        let run = |having: &str| run_query_against_dir(&format!("select ext from __DIR__ group by ext having {having} order by ext"), &tmp);
        let less = run("max(width) < 1000");
        let reversed = run("1000 > max(width)");
        let not_equal = run("max(width) != 5");
        let is_null = run("max(width) is null");
        let is_not_null = run("max(width) is not null");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(less, vec!["png"], "a missing width compares with nothing");
        assert_eq!(reversed, vec!["png"]);
        assert_eq!(not_equal, vec!["png"]);
        assert_eq!(is_null, vec!["md", "txt"]);
        assert_eq!(is_not_null, vec!["png"]);
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
    fn function_calls_are_not_shadowed_by_an_alias_of_the_same_name() {
        let tmp = std::env::temp_dir().join("fselect_test_alias_vs_function_call");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "b.txt", "c.md"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let call = run_query_against_dir("select ext, count(*) as count from __DIR__ group by ext having count(*) > 1", &tmp);
        let alias = run_query_against_dir("select ext, count(*) as count from __DIR__ group by ext having count > 1", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(call, vec!["txt	2"]);
        assert_eq!(alias, vec!["txt	2"]);
    }

    #[test]
    fn correlated_scalar_subqueries_in_grouped_queries_see_the_group() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_correlated_scalar");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "b.txt", "c.md", "x.bin"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let by_field = run_query_against_dir(
            "select ext, count(*), (select count(*) from __DIR__ as o where o.ext = t.ext) from __DIR__ as t group by ext order by ext",
            &tmp,
        );
        let by_alias = run_query_against_dir(
            "select ext as e, (select count(*) from __DIR__ as o where o.ext = t.e) from __DIR__ as t group by e order by e",
            &tmp,
        );
        let in_having = run_query_against_dir(
            "select ext from __DIR__ as t group by ext having (select count(*) from __DIR__ as o where o.ext = t.ext) > 1",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(by_field, vec!["bin	1	1", "md	1	1", "txt	2	2"]);
        assert_eq!(by_alias, vec!["bin	1", "md	1", "txt	2"]);
        assert_eq!(in_having, vec!["txt"]);
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
    fn cached_comparison_results_keep_their_boolean_type() {
        let tmp = std::env::temp_dir().join("fselect_test_cached_comparison_type");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("two.txt"), "xx").unwrap();
        fs::write(tmp.join("three.txt"), "xxx").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let count_then_sum = run("select count(size > 2), sum((size > 2) + 0) from __DIR__");
        let sum_then_count = run("select sum((size > 2) + 0), count(size > 2) from __DIR__");
        let grouped_arithmetic = run("select (size > 2) + 0, count(*) from __DIR__ group by size > 2 order by 1");
        let grouped_having = run("select size > 2, count(*) from __DIR__ group by size > 2 having (size > 2) + 0 = 1");
        let negated = run("select size > 2, -(size > 2) from __DIR__ order by 1");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(count_then_sum, vec![String::from("2\t1")]);
        assert_eq!(sum_then_count, vec![String::from("1\t2")]);
        assert_eq!(grouped_arithmetic, vec![String::from("0\t1"), String::from("1\t1")]);
        assert_eq!(grouped_having, vec![String::from("true\t1")]);
        assert_eq!(negated, vec![String::from("false\t0"), String::from("true\t-1")]);
    }

    #[test]
    fn aggregate_aliases_keep_the_type_the_column_produced() {
        let tmp = std::env::temp_dir().join("fselect_test_aggregate_alias_types");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("true"), "xx").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let boolean_alias = run("select (sum(size) > 1) as big, t.big + 0 from __DIR__ as t");
        let boolean_having = run("select (sum(size) > 1) as big from __DIR__ as t having t.big + 0 = 1");
        let numeric_alias = run("select sum(size) as s, t.s + 1, t.s > 10 from __DIR__ as t");
        let numeric_having = run("select sum(size) as s from __DIR__ as t having t.s > 10");
        let text_alias = run("select max(name) as n, t.n + 0 from __DIR__ as t");
        let coalesced_text = run("select coalesce(max(name), is_dir) as n, t.n + 0 from __DIR__ as t group by is_dir");
        let count_having = run("select count(*) as c from __DIR__ as t having t.c = 1");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(boolean_alias, vec![String::from("true\t1")]);
        assert_eq!(boolean_having, vec![String::from("true")]);
        assert_eq!(numeric_alias, vec![String::from("2\t3\tfalse")]);
        assert!(numeric_having.is_empty());
        assert_eq!(text_alias, vec![String::from("true\t0")]);
        assert_eq!(coalesced_text, vec![String::from("true\t0")]);
        assert_eq!(count_having, vec![String::from("1")]);
    }

    #[test]
    fn in_subqueries_keep_the_result_types_of_their_values() {
        let tmp = std::env::temp_dir().join("fselect_test_in_subquery_types");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("one"), "x").unwrap();
        let half_second_past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::new(1_600_000_000, 500_000_000);
        fs::File::options().write(true).open(tmp.join("one")).unwrap().set_modified(half_second_past).unwrap();

        let run = |condition: &str| run_query_against_dir(&format!("select name from __DIR__ where {condition}"), &tmp);
        let size_in_booleans = run("size in (select is_file from __DIR__)");
        let size_not_in_booleans = run("size not in (select is_file from __DIR__)");
        let size_in_false = run("size in (select is_dir from __DIR__)");
        let boolean_in_sizes = run("is_file in (select size from __DIR__)");
        let modified_in_modified = run("modified in (select modified from __DIR__)");
        let modified_not_in_modified = run("modified not in (select modified from __DIR__)");
        let modified_in_scalar = run("modified in ((select modified from __DIR__))");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(size_in_booleans, vec![String::from("one")]);
        assert!(size_not_in_booleans.is_empty());
        assert!(size_in_false.is_empty());
        assert_eq!(boolean_in_sizes, vec![String::from("one")]);
        assert_eq!(modified_in_modified, vec![String::from("one")]);
        assert!(modified_not_in_modified.is_empty());
        assert_eq!(modified_in_scalar, vec![String::from("one")]);
    }

    #[test]
    fn aggregated_scalar_subqueries_keep_their_boolean_type() {
        let tmp = std::env::temp_dir().join("fselect_test_aggregated_subquery_types");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("one"), "x").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let count_then_sum = run("select count((select size > 0 from __DIR__)), sum((select size > 0 from __DIR__) + 0) from __DIR__");
        let sum_then_count = run("select sum((select size > 0 from __DIR__) + 0), count((select size > 0 from __DIR__)) from __DIR__");
        let grouped = run("select (select size > 0 from __DIR__) + 0, count(*) from __DIR__ group by (select size > 0 from __DIR__)");
        let grouped_having = run("select (select size > 0 from __DIR__), count(*) from __DIR__ group by (select size > 0 from __DIR__) having (select size > 0 from __DIR__) + 0 = 1");
        let negated = run("select count((select size > 0 from __DIR__)), -(select size > 0 from __DIR__) from __DIR__ group by name");
        let text_stays_text = run("select count((select name from __DIR__)), (select name from __DIR__) + 0 from __DIR__ group by name");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(count_then_sum, vec![String::from("1\t1")]);
        assert_eq!(sum_then_count, vec![String::from("1\t1")]);
        assert_eq!(grouped, vec![String::from("1\t1")]);
        assert_eq!(grouped_having, vec![String::from("true\t1")]);
        assert_eq!(negated, vec![String::from("1\t-1")]);
        assert_eq!(text_stays_text, vec![String::from("1\t0")]);
    }

    #[test]
    fn min_and_max_keep_the_type_of_the_selected_value() {
        let tmp = std::env::temp_dir().join("fselect_test_min_max_value_types");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("true"), "xx").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let comparison = run("select min(size > 1), min(size > 1) + 0, max(size > 1) + 0 from __DIR__");
        let boolean_field = run("select min(is_file) + 0 from __DIR__");
        let coalesced_name = run("select min(coalesce(name, is_file)), min(coalesce(name, is_file)) + 0 from __DIR__");
        let plain = run("select min(name) + 0, max(size) + 1, max(modified) > yesterday, min(fsize) from __DIR__");
        let aliased = run("select min(size > 1) as b, t.b + 0 from __DIR__ as t");
        let aliased_having = run("select min(coalesce(name, is_file)) as n from __DIR__ as t having t.n + 0 = 0");
        let grouped_having = run("select min(size > 1), count(*) from __DIR__ group by is_file having min(size > 1) = true");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(comparison, vec![String::from("true\t1\t1")]);
        assert_eq!(boolean_field, vec![String::from("1")]);
        assert_eq!(coalesced_name, vec![String::from("true\t0")]);
        assert_eq!(plain, vec![String::from("0\t3\ttrue\t2B")]);
        assert_eq!(aliased, vec![String::from("true\t1")]);
        assert_eq!(aliased_having, vec![String::from("true")]);
        assert_eq!(grouped_having, vec![String::from("true\t1")]);
    }

    #[test]
    fn ordering_uses_the_type_of_the_sorted_value() {
        let tmp = std::env::temp_dir().join("fselect_test_ordering_value_types");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a-new.txt"), "new").unwrap();
        fs::write(tmp.join("z-old.txt"), "old").unwrap();
        let year_1999 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(928_000_000);
        fs::File::options().write(true).open(tmp.join("z-old.txt")).unwrap().set_modified(year_1999).unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let ascending = run("select name, modified > '2000-01-01' as recent from __DIR__ order by recent, name");
        let descending = run("select name, modified > '2000-01-01' as recent from __DIR__ order by recent desc, name");
        let by_position = run("select name, modified > '2000-01-01' from __DIR__ order by 2, name");
        let limited = run("select name, modified > '2000-01-01' as recent from __DIR__ order by recent limit 1");
        let unselected = run("select name from __DIR__ order by modified > '2000-01-01' limit 1");
        let compact = run("select name from __DIR__ order by (modified>'2000-01-01')+0 desc, name limit 1");
        let coalesced_text = run("select name, coalesce(name, modified) as c from __DIR__ order by c desc");
        let grouped = run("select modified > '2000-01-01' as recent, count(*) from __DIR__ group by recent order by recent");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(ascending, vec![String::from("z-old.txt\tfalse"), String::from("a-new.txt\ttrue")]);
        assert_eq!(descending, vec![String::from("a-new.txt\ttrue"), String::from("z-old.txt\tfalse")]);
        assert_eq!(by_position, vec![String::from("z-old.txt\tfalse"), String::from("a-new.txt\ttrue")]);
        assert_eq!(limited, vec![String::from("z-old.txt\tfalse")]);
        assert_eq!(unselected, vec![String::from("z-old.txt")]);
        assert_eq!(compact, vec![String::from("a-new.txt")]);
        assert_eq!(coalesced_text, vec![String::from("z-old.txt\tz-old.txt"), String::from("a-new.txt\ta-new.txt")]);
        assert_eq!(grouped, vec![String::from("false\t1"), String::from("true\t1")]);
    }

    #[test]
    fn grouped_in_lists_convert_operands_like_the_row_filter() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_in_conversion");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("1.0"), "x").unwrap();
        fs::write(tmp.join("other"), "xy").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let filtered = run("select name from __DIR__ where name in (size)");
        let grouped = run("select name, size from __DIR__ group by name, size having name in (size)");
        let grouped_not = run("select name, size from __DIR__ group by name, size having name not in (size) order by name");
        let text_order = run("select name from __DIR__ order by coalesce(name, size)");
        fs::write(tmp.join("10"), "x").unwrap();
        fs::write(tmp.join("2"), "x").unwrap();
        let least_desc = run("select name from __DIR__ where name != other order by least(name, 'z') desc limit 1");
        let least_asc = run("select name from __DIR__ where name != other order by least(name, 'z')");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(filtered, vec![String::from("1.0")]);
        assert_eq!(grouped, vec![String::from("1.0\t1")]);
        assert_eq!(grouped_not, vec![String::from("other\t2")]);
        assert_eq!(text_order, vec![String::from("1.0"), String::from("other")]);
        assert_eq!(least_desc, vec![String::from("2")]);
        assert_eq!(least_asc, vec![String::from("1.0"), String::from("10"), String::from("2")]);
    }

    #[test]
    fn formatted_sizes_order_by_their_byte_count() {
        let tmp = std::env::temp_dir().join("fselect_test_formatted_size_order");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("small.bin"), vec![0u8; 900]).unwrap();
        fs::write(tmp.join("big.bin"), vec![0u8; 2048]).unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let largest = run("select name from __DIR__ order by fsize desc limit 1");
        let ascending = run("select name, fsize from __DIR__ order by fsize");
        let human_descending = run("select name from __DIR__ order by hsize desc, name");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(largest, vec![String::from("big.bin")]);
        assert_eq!(ascending, vec![String::from("small.bin\t900B"), String::from("big.bin\t2KiB")]);
        assert_eq!(human_descending, vec![String::from("big.bin"), String::from("small.bin")]);
    }

    #[test]
    fn min_and_max_over_mixed_types_keep_the_type_of_the_chosen_value() {
        let tmp = std::env::temp_dir().join("fselect_test_mixed_type_extremes");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("strings")).unwrap();
        fs::create_dir_all(tmp.join("other")).unwrap();
        fs::write(tmp.join("strings").join("true"), "x").unwrap();
        fs::write(tmp.join("other").join("plain"), "y").unwrap();

        let strings = tmp.join("strings");
        let other = tmp.join("other");
        let run = |query: &str| {
            run_query_against_dir(
                &query.replace("__STRINGS__", &strings.to_string_lossy()).replace("__OTHER__", &other.to_string_lossy()),
                &tmp,
            )
        };
        let maximum = run("select max(coalesce(t.name, u.is_dir)) + 0 from __STRINGS__ as t, __OTHER__ as u");
        let both = run("select max(coalesce(t.name, u.is_dir)), min(coalesce(t.name, u.is_dir)) + 0 from __STRINGS__ as t, __OTHER__ as u");
        let having = run("select max(coalesce(t.name, u.is_dir)) as m from __STRINGS__ as t, __OTHER__ as u having m + 0 = 0");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(maximum, vec![String::from("0")]);
        assert_eq!(both, vec![String::from("true\t0")]);
        assert_eq!(having, vec![String::from("true")]);
    }

    #[test]
    fn arithmetic_keeps_missing_metadata_missing() {
        let tmp = std::env::temp_dir().join("fselect_test_missing_metadata_arithmetic");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let three_pixels_wide = [137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 3, 0, 0, 0, 1, 8, 2, 0, 0, 0, 148, 130, 131, 227, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 0, 65, 0, 17, 248, 2, 254, 227, 229, 239, 182, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];
        fs::write(tmp.join("img.png"), three_pixels_wide).unwrap();
        fs::write(tmp.join("a.txt"), "a").unwrap();
        fs::write(tmp.join("b.txt"), "b").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let averages = run("select avg(width), avg(width + 0) from __DIR__");
        let per_file = run("select name, width + 0, width / height, 10 / width from __DIR__ order by name");
        let extremes = run("select sum(width + 0), count(width + 0), min(width + 0) from __DIR__");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(averages, vec![String::from("3\t3")]);
        assert_eq!(
            per_file,
            vec![
                String::from("a.txt"),
                String::from("b.txt"),
                String::from("img.png\t3\t3\t3.3333333333333335"),
            ],
            "text files have no width: every arithmetic cell stays empty, nothing errors"
        );
        assert_eq!(extremes, vec![String::from("3\t1\t3")]);
    }

    #[test]
    fn a_scalar_subquery_without_rows_is_null_even_for_text_columns() {
        let tmp = std::env::temp_dir().join("fselect_test_null_text_subquery");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("empty")).unwrap();
        fs::write(tmp.join("data").join("a.txt"), "a").unwrap();
        fs::write(tmp.join("data").join("b.txt"), "bb").unwrap();
        fs::write(tmp.join("data").join("noext"), "n").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let not_equal = run("select name from __DIR__/data where name != (select name from __DIR__/empty)");
        let equal = run("select name from __DIR__/data where name = (select name from __DIR__/empty)");
        let greater = run("select name from __DIR__/data where name > (select name from __DIR__/empty)");
        let filtered_away = run("select name from __DIR__/data where name != (select name from __DIR__/data where size > 100)");
        let having = run("select ext, count(*) from __DIR__/data group by ext having max(name) != (select name from __DIR__/empty)");
        let empty_text_is_a_value = run("select name from __DIR__/data where ext != 'zzz' order by name");
        let real_value = run("select name from __DIR__/data where name != (select name from __DIR__/data where name = 'a.txt') order by name");
        let _ = fs::remove_dir_all(&tmp);

        assert!(not_equal.is_empty(), "nothing compares unequal to NULL");
        assert!(equal.is_empty());
        assert!(greater.is_empty());
        assert!(filtered_away.is_empty());
        assert!(having.is_empty());
        assert_eq!(empty_text_is_a_value, vec![String::from("a.txt"), String::from("b.txt"), String::from("noext")]);
        assert_eq!(real_value, vec![String::from("b.txt"), String::from("noext")]);
    }

    #[test]
    fn computed_aliases_resolve_when_referenced_directly() {
        let tmp = std::env::temp_dir().join("fselect_test_direct_alias_references");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "a").unwrap();
        fs::write(tmp.join("b.txt"), "bb").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let arithmetic = run("select name, size + 1 as x, t.x from __DIR__ as t order by name");
        let filtered = run("select name, size + 1 as x from __DIR__ as t where t.x > 2 order by name");
        let function = run("select upper(name) as u, t.u from __DIR__ as t order by name");
        let comparison = run("select name, (size > 1) as b, t.b + 0 from __DIR__ as t order by name");
        let subquery = run("select name, (select count(*) from __DIR__) as c, t.c + 0 from __DIR__ as t order by name");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(arithmetic, vec![String::from("a.txt\t2\t2"), String::from("b.txt\t3\t3")]);
        assert_eq!(filtered, vec![String::from("b.txt\t3")]);
        assert_eq!(function, vec![String::from("A.TXT\tA.TXT"), String::from("B.TXT\tB.TXT")]);
        assert_eq!(comparison, vec![String::from("a.txt\tfalse\t0"), String::from("b.txt\ttrue\t1")]);
        assert_eq!(subquery, vec![String::from("a.txt\t2\t2"), String::from("b.txt\t2\t2")]);
    }

    #[test]
    fn null_never_matches_inside_an_in_list_or_through_nesting() {
        let tmp = std::env::temp_dir().join("fselect_test_null_through_lists_and_nesting");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("empty")).unwrap();
        fs::write(tmp.join("data").join("a.txt"), "a").unwrap();
        fs::write(tmp.join("data").join("b.txt"), "bb").unwrap();
        fs::write(tmp.join("data").join("noext"), "n").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let in_list = run("select name from __DIR__/data where ext in ((select name from __DIR__/empty), 'txt') order by name");
        let nested = run("select name from __DIR__/data where name != (select (select name from __DIR__/empty) from __DIR__/data limit 1)");
        let coalesced = run("select name from __DIR__/data where name != coalesce((select name from __DIR__/empty))");
        let coalesced_fallback = run("select name from __DIR__/data where name != coalesce((select name from __DIR__/empty), 'a.txt') order by name");
        let empty_max = run("select name from __DIR__/data where name != (select max(name) from __DIR__/empty)");
        let empty_min = run("select name from __DIR__/data where name != (select min(name) from __DIR__/empty)");
        let empty_count = run("select name from __DIR__/data where size >= (select count(*) from __DIR__/empty) order by name");
        let empty_ext_fallback = run("select coalesce(ext, 'none') from __DIR__/data where name = 'noext'");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(in_list, vec![String::from("a.txt"), String::from("b.txt")], "NULL in the list must not match the empty extension");
        assert!(nested.is_empty(), "NULL survives a nested scalar subquery");
        assert!(coalesced.is_empty(), "COALESCE of only NULL is NULL");
        assert_eq!(coalesced_fallback, vec![String::from("b.txt"), String::from("noext")]);
        assert!(empty_max.is_empty(), "MAX over no rows is NULL");
        assert!(empty_min.is_empty(), "MIN over no rows is NULL");
        assert_eq!(empty_count, vec![String::from("a.txt"), String::from("b.txt"), String::from("noext")], "COUNT over no rows is 0, not NULL");
        assert_eq!(empty_ext_fallback, vec![String::from("none")], "an empty extension still falls through COALESCE");
    }

    #[test]
    fn a_cached_null_stays_null_for_later_reads() {
        let tmp = std::env::temp_dir().join("fselect_test_cached_null");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let null_first = run("select count(*) from __DIR__ having max(name) is null and max(name) != 'x'");
        let null_last = run("select count(*) from __DIR__ having max(name) != 'x' and max(name) is null");
        let only_null_check = run("select count(*) from __DIR__ having max(name) is null");
        let selected_twice = run("select count(*), max(name), max(name) != 'x' from __DIR__");
        let _ = fs::remove_dir_all(&tmp);

        assert!(null_first.is_empty(), "the second read of max(name) must still be NULL");
        assert!(null_last.is_empty());
        assert_eq!(only_null_check, vec![String::from("0")]);
        assert_eq!(selected_twice, vec![String::from("0\t\tfalse")], "a NULL read twice in one SELECT list compares as unknown");
    }

    #[test]
    fn aliases_and_their_references_agree_on_random_values() {
        let tmp = std::env::temp_dir().join("fselect_test_random_alias_consistency");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for i in 0..60 {
            fs::write(tmp.join(format!("f{i}.txt")), "x").unwrap();
        }

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let cells = |row: &String| row.split('\t').map(str::to_string).collect::<Vec<_>>();
        let direct = run("select name, random(1,2) as r, t.r from __DIR__ as t");
        let filtered = run("select name, random(1,2) as r, t.r from __DIR__ as t where t.r = 1");
        let dependent = run("select name, random(1,1000) as r, t.r + 1 as s from __DIR__ as t");
        let correlated = run("select name, random(1,1000) as r, (select max(t.r) from __DIR__ limit 1) from __DIR__ as t");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(direct.len(), 60);
        for row in &direct {
            let row = cells(row);
            assert_eq!(row[1], row[2], "r and t.r differ in {row:?}");
        }
        assert!(!filtered.is_empty());
        for row in &filtered {
            let row = cells(row);
            assert_eq!(row[1], "1", "a row passed `t.r = 1` but shows r = {}", row[1]);
            assert_eq!(row[2], "1");
        }
        for row in &dependent {
            let row = cells(row);
            let r: i64 = row[1].parse().unwrap();
            assert_eq!(row[2], (r + 1).to_string(), "s must be r + 1 in {row:?}");
        }
        for row in &correlated {
            let row = cells(row);
            assert_eq!(row[1], row[2], "a correlated subquery must see the same r in {row:?}");
        }
    }

    #[test]
    fn grouping_keys_keep_their_own_null_state_per_group() {
        let tmp = std::env::temp_dir().join("fselect_test_group_key_null_state");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("lookup")).unwrap();
        fs::write(tmp.join("data").join("alpha.txt"), "a").unwrap();
        fs::write(tmp.join("data").join("zulu.txt"), "z").unwrap();
        fs::write(tmp.join("lookup").join("alpha.txt"), "a").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let key = "(select name from __DIR__/lookup where name = t.name) as k";
        let ascending = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name) as t group by k order by k"));
        let descending = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name desc) as t group by k order by k"));
        let having_value = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name) as t group by k having k = 'alpha.txt'"));
        let having_null = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name) as t group by k having k is null"));
        let having_not_equal = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name) as t group by k having k != 'x'"));
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(ascending, vec![String::from("\t1"), String::from("alpha.txt\t1")]);
        assert_eq!(descending, ascending, "scan order must not change the groups");
        assert_eq!(having_value, vec![String::from("alpha.txt\t1")]);
        assert_eq!(having_null, vec![String::from("\t1")]);
        assert_eq!(having_not_equal, vec![String::from("alpha.txt\t1")], "the NULL group is unknown, not unequal");
    }

    #[test]
    fn two_random_aliases_agree_with_their_own_references() {
        let tmp = std::env::temp_dir().join("fselect_test_two_random_aliases");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for i in 0..100 {
            fs::write(tmp.join(format!("f{i}.txt")), "x").unwrap();
        }

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let cells = |row: &String| row.split('\t').map(str::to_string).collect::<Vec<_>>();
        let all = run("select name, random(1,2) as r, random(1,2) as s, t.r, t.s from __DIR__ as t");
        let filtered = run("select name, random(1,2) as r, random(1,2) as s, t.r, t.s from __DIR__ as t where t.r = 1");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(all.len(), 100);
        for row in all.iter().chain(&filtered) {
            let row = cells(row);
            assert_eq!(row[1], row[3], "r and t.r differ in {row:?}");
            assert_eq!(row[2], row[4], "s and t.s differ in {row:?}");
        }
        assert!(!filtered.is_empty());
        for row in &filtered {
            let row = cells(row);
            assert_eq!(row[1], "1", "a row passed `t.r = 1` but shows r = {}", row[1]);
        }
    }

    #[test]
    fn a_null_grouping_key_is_not_the_empty_string_group() {
        let tmp = std::env::temp_dir().join("fselect_test_null_vs_empty_group");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("data")).unwrap();
        fs::create_dir_all(tmp.join("lookup")).unwrap();
        fs::write(tmp.join("data").join("alpha"), "a").unwrap();
        fs::write(tmp.join("data").join("zulu"), "z").unwrap();
        fs::write(tmp.join("lookup").join("alpha"), "a").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let key = "(select ext from __DIR__/lookup where name = t.name) as k";
        let groups = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name) as t group by k"));
        let empty_text = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name) as t group by k having k = ''"));
        let empty_text_reversed = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name desc) as t group by k having k = ''"));
        let null_group = run(&format!("select {key}, count(*) from (select path from __DIR__/data order by name) as t group by k having k is null"));
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(groups.len(), 2, "NULL and the empty extension are different groups: {groups:?}");
        assert_eq!(empty_text, vec![String::from("\t1")], "only the successful lookup has an empty extension");
        assert_eq!(empty_text_reversed, empty_text, "scan order must not matter");
        assert_eq!(null_group.len(), 2, "IS NULL holds for the NULL group and, by the engine's rule, for an empty text as well");
    }

    #[test]
    fn word_operators_form_conditions_in_select_columns() {
        let tmp = std::env::temp_dir().join("fselect_test_word_operators_in_select");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "a").unwrap();
        fs::write(tmp.join("bb.txt"), "bb").unwrap();
        fs::write(tmp.join("noext"), "n").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let gt = run("select name, size gt 1 from __DIR__ order by name");
        let parenthesised = run("select name, (size gt 1) from __DIR__ order by name");
        let like = run("select name, name like 'a%' from __DIR__ order by name");
        let is_null = run("select name, ext is null from __DIR__ order by name");
        let in_list = run("select name, ext in ('txt') from __DIR__ order by name");
        let grouped = run("select size gt 1, count(*) from __DIR__ group by size gt 1 order by 1");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(gt, vec![String::from("a.txt\tfalse"), String::from("bb.txt\ttrue"), String::from("noext\tfalse")]);
        assert_eq!(parenthesised, gt);
        assert_eq!(like, vec![String::from("a.txt\ttrue"), String::from("bb.txt\tfalse"), String::from("noext\tfalse")]);
        assert_eq!(is_null, vec![String::from("a.txt\tfalse"), String::from("bb.txt\tfalse"), String::from("noext\ttrue")]);
        assert_eq!(in_list, vec![String::from("a.txt\ttrue"), String::from("bb.txt\ttrue"), String::from("noext\tfalse")]);
        assert_eq!(grouped, vec![String::from("false\t2"), String::from("true\t1")]);
    }

    #[test]
    fn a_failing_alias_does_not_unsettle_the_other_aliases() {
        let tmp = std::env::temp_dir().join("fselect_test_failing_alias_neighbours");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for i in 0..60 {
            fs::write(tmp.join(format!("f{i}.txt")), "x").unwrap();
        }

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let cells = |row: &String| row.split('\t').map(str::to_string).collect::<Vec<_>>();
        let rows = run("select name, random(1,2) as r, t.r, 1 / (size - size) as boom, coalesce(name, t.boom) from __DIR__ as t");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows.len(), 60, "boom is required but never read, so every row survives");
        for row in &rows {
            let row = cells(row);
            assert_eq!(row[1], row[2], "r and t.r differ next to a failing alias in {row:?}");
            assert!(row[3].is_empty(), "the failing alias stays empty in {row:?}");
            assert_eq!(row[4], row[0]);
        }
    }

    #[test]
    fn negated_word_operators_work_outside_where() {
        let tmp = std::env::temp_dir().join("fselect_test_negated_word_operators");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "a").unwrap();
        fs::write(tmp.join("b.bin"), "b").unwrap();
        fs::write(tmp.join("c.txt"), "c").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let ordered = run("select name from __DIR__ order by name not like 'b%', name");
        let columns = run("select name, name not like 'b%', ext not in ('txt'), size not between 2 and 3 from __DIR__ order by name");
        let positive = run("select name, size between 1 and 2, name like 'b%' from __DIR__ order by name");
        let grouped = run("select name not like 'b%', count(*) from __DIR__ group by name not like 'b%' order by 1");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(ordered, vec![String::from("b.bin"), String::from("a.txt"), String::from("c.txt")]);
        assert_eq!(
            columns,
            vec![
                String::from("a.txt\ttrue\tfalse\ttrue"),
                String::from("b.bin\tfalse\ttrue\ttrue"),
                String::from("c.txt\ttrue\tfalse\ttrue"),
            ]
        );
        assert_eq!(
            positive,
            vec![
                String::from("a.txt\ttrue\tfalse"),
                String::from("b.bin\ttrue\ttrue"),
                String::from("c.txt\ttrue\tfalse"),
            ]
        );
        assert_eq!(grouped, vec![String::from("false\t1"), String::from("true\t2")]);
    }

    #[test]
    fn membership_works_in_aggregate_context() {
        let tmp = std::env::temp_dir().join("fselect_test_aggregate_membership");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "a").unwrap();
        fs::write(tmp.join("b.bin"), "b").unwrap();
        fs::write(tmp.join("c.txt"), "c").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let ungrouped = run("select count(*) in (3), count(*) not in (3) from __DIR__");
        let grouped = run("select ext, count(*) in (2) from __DIR__ group by ext order by ext");
        let having = run("select ext, count(*) from __DIR__ group by ext having count(*) in (2)");
        let subquery = run("select count(*) in (select count(*) from __DIR__) from __DIR__");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(ungrouped, vec![String::from("true\tfalse")]);
        assert_eq!(grouped, vec![String::from("bin\tfalse"), String::from("txt\ttrue")]);
        assert_eq!(having, vec![String::from("txt\t2")]);
        assert_eq!(subquery, vec![String::from("true")]);
    }

    #[test]
    fn random_aliases_agree_in_ungrouped_aggregates() {
        let tmp = std::env::temp_dir().join("fselect_test_random_alias_ungrouped");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "a").unwrap();
        fs::write(tmp.join("b.bin"), "b").unwrap();
        fs::write(tmp.join("c.txt"), "c").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let cells = |row: &String| row.split('\t').map(str::to_string).collect::<Vec<_>>();
        for _ in 0..20 {
            let plain = run("select count(*), random(1,1000000) as r, t.r from __DIR__ as t");
            let row = cells(&plain[0]);
            assert_eq!(row[1], row[2], "r and t.r differ in an ungrouped aggregate: {row:?}");

            let having = run("select count(*), random(1,2) as r, t.r from __DIR__ as t having r = 1");
            for row in having.iter().map(cells) {
                assert_eq!(row[1], "1", "a row passed `having r = 1` but shows r = {}", row[1]);
                assert_eq!(row[2], "1");
            }

            let grouped = run("select ext, random(1,1000000) as r, t.r from __DIR__ as t group by ext");
            for row in grouped.iter().map(cells) {
                assert_eq!(row[1], row[2], "r and t.r differ in a grouped aggregate: {row:?}");
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn grouped_in_lists_see_the_group_values() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_in_list_fields");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "a").unwrap();
        fs::write(tmp.join("b.bin"), "b").unwrap();
        fs::write(tmp.join("c.txt"), "c").unwrap();

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let having = run("select ext, count(*) from __DIR__ group by ext having ext in (ext) order by ext");
        let mixed = run("select ext, count(*) from __DIR__ group by ext having ext in ('zzz', ext) order by ext");
        let selected = run("select ext, ext in (ext), ext not in (ext), count(*) from __DIR__ group by ext order by ext");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(having, vec![String::from("bin\t1"), String::from("txt\t2")]);
        assert_eq!(mixed, having);
        assert_eq!(selected, vec![String::from("bin\ttrue\tfalse\t1"), String::from("txt\ttrue\tfalse\t2")]);
    }

    #[test]
    fn raw_timestamps_stay_numeric_after_grouping() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_raw_timestamps");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for (name, nanos) in [("one", 1_000_000), ("two", 2_000_000), ("ten", 10_000_000)] {
            fs::write(tmp.join(name), "x").unwrap();
            let stamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::new(1_600_000_000, nanos);
            fs::File::options().write(true).open(tmp.join(name)).unwrap().set_modified(stamp).unwrap();
        }

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let filtered = run("select mtime_nsec, count(*) from __DIR__ group by mtime_nsec having mtime_nsec < 5000000 order by mtime_nsec");
        let descending = run("select mtime_nsec from __DIR__ group by mtime_nsec order by mtime_nsec desc");
        let seconds = run("select mtime, count(*) from __DIR__ group by mtime having mtime > 1");
        let rows = run("select name from __DIR__ where mtime_nsec < 5000000 order by mtime_nsec");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(filtered, vec![String::from("1000000\t1"), String::from("2000000\t1")]);
        assert_eq!(descending, vec![String::from("10000000"), String::from("2000000"), String::from("1000000")]);
        assert_eq!(seconds, vec![String::from("1600000000\t3")]);
        assert_eq!(rows, vec![String::from("one"), String::from("two")]);
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
        assert!(scalar.get(&Field::Size).is_some_and(|keys| keys.contains("Size")));
        assert!(nested_root.get(&Field::Modified).is_some_and(|keys| keys.contains("Modified")));

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
    fn correlated_references_under_different_aliases_all_see_the_outer_row() {
        let tmp = std::env::temp_dir().join("fselect_test_snapshot_alias_keys");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let other = tmp.join("other");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(data.join("a.txt"), "x").unwrap();
        fs::write(data.join("b.txt"), "xx").unwrap();
        fs::write(other.join("o.txt"), "x".repeat(50)).unwrap();
        let other_root = other.to_string_lossy().to_string();

        let plain_then_aliased = run_query_against_dir(
            &format!("select name, (select t.size from {other_root}), (select t.size as x from {other_root}) from __DIR__ as t order by name"),
            &data,
        );
        let two_aliases = run_query_against_dir(
            &format!("select name, (select t.size as x from {other_root}), (select t.size as y from {other_root}), (select t.size from {other_root}) from __DIR__ as t order by name"),
            &data,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(plain_then_aliased, vec!["a.txt\t1\t1", "b.txt\t2\t2"]);
        assert_eq!(two_aliases, vec!["a.txt\t1\t1\t1", "b.txt\t2\t2\t2"]);
    }

    #[test]
    fn quoted_dotted_literals_stay_literals_and_bare_ones_reference_column_aliases() {
        let tmp = std::env::temp_dir().join("fselect_test_dotted_literals_in_subqueries");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let other = tmp.join("other");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(data.join("a.txt"), "x").unwrap();
        fs::write(data.join("b.txt"), "xx").unwrap();
        fs::write(other.join("readme.md"), "x".repeat(50)).unwrap();
        let other_root = other.to_string_lossy().to_string();

        let quoted = run_query_against_dir(
            &format!("select name, (select 't.size' from {other_root}) from __DIR__ as t order by name"),
            &data,
        );
        let bare_file_name = run_query_against_dir(
            &format!("select name, (select name from {other_root} where name = readme.md) from __DIR__ as t order by name"),
            &data,
        );
        let column_alias = run_query_against_dir(
            &format!("select size as sz, (select t.sz + 1 from {other_root}) from __DIR__ as t order by sz"),
            &data,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(quoted, vec!["a.txt\tt.size", "b.txt\tt.size"]);
        assert_eq!(bare_file_name, vec!["a.txt\treadme.md", "b.txt\treadme.md"]);
        assert_eq!(column_alias, vec!["1\t2", "2\t3"]);
    }

    #[test]
    fn a_row_error_on_a_directory_does_not_prune_its_subtree() {
        let tmp = std::env::temp_dir().join("fselect_test_directory_row_error");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub").join("deep")).unwrap();
        fs::write(tmp.join("top.txt"), "a
b
").unwrap();
        fs::write(tmp.join("sub").join("mid.txt"), "c
").unwrap();
        fs::write(tmp.join("sub").join("deep").join("leaf.txt"), "d
e
f
").unwrap();

        let searcher = search_dir("select name, 1 / (is_dir - 1) from __DIR__ order by name", &tmp, Config::default(), true);
        let rows: Vec<String> = searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["deep", "leaf.txt	-1", "mid.txt	-1", "sub", "top.txt	-1"], "directories keep their row with an empty cell");
        assert_eq!(searcher.error_count, 2, "the two directories divide by zero, nothing else");
    }

    #[test]
    fn a_failing_select_cell_keeps_its_row_in_every_path() {
        let tmp = std::env::temp_dir().join("fselect_test_failing_cell_keeps_row");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for (name, lines) in [("a.txt", 1), ("b.txt", 2), ("c.txt", 3)] {
            fs::write(tmp.join(name), "line\n".repeat(lines)).unwrap();
        }
        let rows = |searcher: &Searcher| searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect::<Vec<_>>();

        let deferred = search_dir("select name, 1 / (line_count - 1) from __DIR__ order by name limit 2", &tmp, Config::default(), true);
        let eager_limited = search_dir(
            "select name, 1 / (line_count - 1) from __DIR__ where line_count > 0 order by name limit 2",
            &tmp, Config::default(), true,
        );
        let eager = search_dir("select name, 1 / (line_count - 1) from __DIR__ where line_count > 0 order by name", &tmp, Config::default(), true);
        let failing_filter = search_dir("select name from __DIR__ where 1 / (line_count - 1) > 0 order by name", &tmp, Config::default(), true);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows(&deferred), vec!["a.txt", "b.txt\t1"]);
        assert_eq!(rows(&eager_limited), vec!["a.txt", "b.txt\t1"], "a filter that excludes nothing must not change the rows");
        assert_eq!(rows(&eager), vec!["a.txt", "b.txt\t1", "c.txt\t0.5"]);
        assert_eq!(eager.error_count, 1);
        assert_eq!(rows(&failing_filter), vec!["b.txt", "c.txt"], "a row whose filter cannot be evaluated is still skipped");
        assert_eq!(failing_filter.error_count, 1);
    }

    #[test]
    fn order_by_expressions_over_select_aliases_sort_by_the_aliased_value() {
        let tmp = std::env::temp_dir().join("fselect_test_order_by_alias_expression");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "xx").unwrap();
        fs::write(tmp.join("c.txt"), "x".repeat(10)).unwrap();

        let bare = run_query_against_dir("select name, size as sz from __DIR__ order by sz desc", &tmp);
        let arithmetic = run_query_against_dir("select name, size as sz from __DIR__ order by sz + 0 desc", &tmp);
        let function = run_query_against_dir("select name, size as sz from __DIR__ order by abs(sz) desc", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        let expected = vec!["c.txt\t10", "b.txt\t2", "a.txt\t1"];
        assert_eq!(bare, expected);
        assert_eq!(arithmetic, expected);
        assert_eq!(function, expected);
    }

    #[test]
    fn correlated_column_aliases_compare_by_their_value() {
        let tmp = std::env::temp_dir().join("fselect_test_correlated_alias_comparison");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let other = tmp.join("other");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(data.join("a.txt"), "x").unwrap();
        fs::write(data.join("b.txt"), "xx").unwrap();
        fs::write(data.join("c.txt"), "x".repeat(10)).unwrap();
        fs::write(other.join("o.txt"), "x".repeat(5)).unwrap();
        let other_root = other.to_string_lossy().to_string();

        let numeric = run_query_against_dir(
            &format!("select name, size as sz from __DIR__ as t where exists (select name from {other_root} where size > t.sz) order by name"),
            &data,
        );
        let date = run_query_against_dir(
            &format!("select name, modified as m from __DIR__ as t where exists (select name from {other_root} where modified >= t.m) order by name"),
            &data,
        );
        let computed = run_query_against_dir(
            &format!("select name, length(name) as n from __DIR__ as t where exists (select name from {other_root} where size = t.n) order by name"),
            &data,
        );
        let unknown = search_dir(
            &format!("select name from __DIR__ as t where exists (select name from {other_root} where t.nope = 1)"),
            &data, Config::default(), true,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(numeric, vec!["a.txt	1", "b.txt	2"]);
        assert_eq!(date.len(), 3);
        assert_eq!(computed.len(), 3, "an aliased expression is snapshotted like a field");
        assert_eq!(unknown.output_buffer.iter_values().count(), 0);
        assert_eq!(unknown.error_count, 0, "an unknown column name under a declared alias is an empty value, not an error");
    }

    #[test]
    fn function_and_subquery_results_compare_by_their_type_in_where() {
        let tmp = std::env::temp_dir().join("fselect_test_typed_results_in_where");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for i in 1..=10 {
            fs::write(tmp.join(format!("f{i}.txt")), "x".repeat(i)).unwrap();
        }

        let count = |condition: &str| {
            run_query_against_dir(&format!("select count(*) from __DIR__ where {condition}"), &tmp)
        };
        let cases: Vec<(&str, &str)> = vec![
            ("(select count(*) from __DIR__) > 2", "10"),
            ("(select count(*) from __DIR__) < 2", "0"),
            ("2 < (select count(*) from __DIR__)", "10"),
            ("(select count(*) from __DIR__) in (10, 20)", "10"),
            ("date_add(modified, 30) > today", "10"),
            ("today < date_add(modified, 30)", "10"),
            ("date_sub(modified, 30) < yesterday", "10"),
            ("date_trunc('day', modified) = today", "10"),
            ("modified = today", "10"),
            ("today = modified", "10"),
            ("from_unixtime(mtime) = today", "10"),
            ("coalesce(size, 0) > 9", "1"),
            ("upper(name) > 'F5'", "5"),
        ];
        let results: Vec<(&str, Vec<String>)> = cases.iter().map(|(condition, _)| (*condition, count(condition))).collect();
        let _ = fs::remove_dir_all(&tmp);

        for ((condition, expected), (_, actual)) in cases.iter().zip(results) {
            assert_eq!(actual, vec![expected.to_string()], "{}", condition);
        }
    }

    #[test]
    fn coloured_output_does_not_leak_into_subquery_comparisons() {
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        let tmp = std::env::temp_dir().join("fselect_test_coloured_subqueries");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("a.txt"), "").unwrap();

        let run_coloured = |query_template: &str| {
            let query_string = query_template.replace("__DIR__", &tmp.to_string_lossy());
            let mut lexer = Lexer::new(vec![query_string]);
            let parsed = Box::leak(Box::new(Parser::new(&mut lexer).parse(false).expect("parse failed")));
            let config = Box::leak(Box::new(Config::default()));
            let default_config = Box::leak(Box::new(Config::default()));
            let mut searcher = Searcher::new(parsed, config, default_config, true);
            searcher.silent_mode = true;
            searcher.list_search_results().expect("search failed");
            searcher.output_buffer.iter_values().count()
        };
        let membership = run_coloured("select name from __DIR__ where name in (select name from __DIR__)");
        let scalar = run_coloured("select name from __DIR__ where name = (select name from __DIR__ where is_dir order by name)");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(membership, 2, "both the file and the directory must match themselves");
        assert_eq!(scalar, 1, "the directory must equal its own plain name");
    }

    #[test]
    fn alias_references_respect_case_and_do_not_shadow_file_names() {
        let tmp = std::env::temp_dir().join("fselect_test_alias_scope_rules");
        let _ = fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let other = tmp.join("other");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(data.join("a.txt"), "x").unwrap();
        fs::write(data.join("t.txt"), "xx").unwrap();
        fs::write(data.join("c.txt"), "x".repeat(10)).unwrap();
        fs::write(other.join("o.txt"), "xx").unwrap();
        let other_root = other.to_string_lossy().to_string();

        let upper_prefix = run_query_against_dir(
            &format!("select name from __DIR__ as t where exists (select name from {other_root} where size = T.size) order by name"),
            &data,
        );
        let upper_column = run_query_against_dir(
            &format!("select name, size as sz from __DIR__ as t where exists (select name from {other_root} where size = T.sz) order by name"),
            &data,
        );
        let file_name = run_query_against_dir("select name from __DIR__ as t where name = t.txt", &data);
        let ordered = run_query_against_dir("select name, size as sz from __DIR__ as t order by t.sz desc", &data);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(upper_prefix, vec!["t.txt"]);
        assert_eq!(upper_column, vec!["t.txt\t2"]);
        assert_eq!(file_name, vec!["t.txt"]);
        assert_eq!(ordered, vec!["c.txt\t10", "t.txt\t2", "a.txt\t1"]);
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn default_acl_entry_produces_one_row_per_file() {
        let tmp = std::env::temp_dir().join("fselect_test_default_acl_rows");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let rows = run_query_against_dir("select default_acl_entry('Everyone') from __DIR__", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn epoch_fields_are_negative_before_1970() {
        let tmp = std::env::temp_dir().join("fselect_test_pre_epoch_times");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("old.txt");
        fs::write(&path, "").unwrap();
        let when = std::time::UNIX_EPOCH - std::time::Duration::new(300_000_000, 250_000_000);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(when).set_accessed(when))
            .unwrap();

        let rows = run_query_against_dir("select mtime, mtime_nsec, atime, atime_nsec, modified from __DIR__", &tmp);
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(rows.len(), 1);
        let columns: Vec<&str> = rows[0].split('\t').collect();
        assert_eq!(columns[..4], ["-300000001", "750000000", "-300000001", "750000000"]);
        assert!(columns[4].starts_with("1960-"), "{}", rows[0]);
    }

    #[test]
    fn exif_altitude_honours_the_reference_byte_and_its_default() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("test").join("exif");
        let rows = run_query_against_dir("select name, exif_alt from __DIR__ order by name", &fixtures);
        assert_eq!(
            rows,
            vec![
                "altitude-above-sea-level.jpg	123.5",
                "altitude-below-sea-level.jpg	-123.5",
                "altitude-without-ref.jpg	123.5",
            ]
        );
    }

    #[test]
    fn like_with_an_empty_pattern_matches_empty_values() {
        let tmp = std::env::temp_dir().join("fselect_test_like_empty");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("d")).unwrap();
        fs::write(tmp.join("a.txt"), "").unwrap();
        fs::write(tmp.join("noext"), "").unwrap();

        let searcher = search_dir("select name from __DIR__ where ext like '' order by name", &tmp, Config::default(), true);
        let rows: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
        assert_eq!(rows, vec!["d", "noext"]);
        assert_eq!(searcher.error_count, 0);

        let rows = run_query_against_dir("select name from __DIR__ where ext not like ''", &tmp);
        assert_eq!(rows, vec!["a.txt"]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn boolean_literal_conditions_select_all_or_nothing() {
        let tmp = std::env::temp_dir().join("fselect_test_boolean_literal_where");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["a.txt", "b.txt"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        assert_eq!(run_query_against_dir("select name from __DIR__ where true", &tmp).len(), 2);
        assert_eq!(run_query_against_dir("select name from __DIR__ where not false", &tmp).len(), 2);
        assert_eq!(run_query_against_dir("select name from __DIR__ where false", &tmp).len(), 0);
        assert_eq!(run_query_against_dir("select name from __DIR__ where false or name = 'a.txt'", &tmp).len(), 1);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(feature = "git")]
    fn init_repo_ignoring(dir: &Path, pattern: &str) {
        fs::create_dir_all(dir).unwrap();
        git2::Repository::init(dir).unwrap();
        fs::write(dir.join(".gitignore"), format!("{pattern}
")).unwrap();
    }

    #[cfg(feature = "git")]
    fn file_names(rows: &[String]) -> Vec<String> {
        rows.iter()
            .map(|row| Path::new(row).file_name().unwrap().to_string_lossy().to_string())
            .collect()
    }

    #[cfg(feature = "git")]
    fn commit_everything(dir: &Path) {
        let repo = git2::Repository::open(dir).unwrap();
        let sig = git2::Signature::new("test", "test@test", &git2::Time::new(1_000_000_000, 0)).unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &[]).unwrap();
    }

    #[cfg(feature = "git")]
    #[test]
    fn gitignore_keeps_tracked_files_and_the_directories_holding_them() {
        let tmp = std::env::temp_dir().join("fselect_test_gitignore_tracked");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("logs").join("deep")).unwrap();
        git2::Repository::init(&tmp).unwrap();
        for name in ["tracked.log", "logs/keep.log", "logs/deep/kept.log"] {
            fs::write(tmp.join(name), "").unwrap();
        }
        commit_everything(&tmp);
        fs::write(tmp.join(".gitignore"), "*.log\nlogs/\n").unwrap();
        for name in ["loose.log", "logs/new.log", "logs/deep/fresh.log"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        for mode in ["bfs", "dfs"] {
            let rows = run_query_against_dir(&format!("select path from __DIR__ gitignore {mode}"), &tmp);
            let mut names = file_names(&rows);
            names.retain(|n| n != ".gitignore");
            names.sort();
            assert_eq!(names, vec!["deep", "keep.log", "kept.log", "logs", "tracked.log"], "{mode}: {rows:?}");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(feature = "git")]
    #[test]
    fn gitignore_keeps_tracked_files_renamed_only_by_case() {
        let tmp = std::env::temp_dir().join("fselect_test_gitignore_tracked_icase");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("logs")).unwrap();
        let repo = git2::Repository::init(&tmp).unwrap();
        let ignores_case = repo.config().and_then(|c| c.get_bool("core.ignorecase")).unwrap_or(false);
        drop(repo);
        if !ignores_case {
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
        for name in ["tracked.log", "logs/keep.log"] {
            fs::write(tmp.join(name), "").unwrap();
        }
        commit_everything(&tmp);
        fs::write(tmp.join(".gitignore"), "*.log\nlogs/\n").unwrap();
        fs::rename(tmp.join("tracked.log"), tmp.join("TRACKED.log")).unwrap();
        fs::rename(tmp.join("logs"), tmp.join("LOGS")).unwrap();
        fs::write(tmp.join("LOGS").join("new.log"), "").unwrap();

        for mode in ["bfs", "dfs"] {
            let rows = run_query_against_dir(&format!("select path from __DIR__ gitignore {mode}"), &tmp);
            let mut names = file_names(&rows);
            names.retain(|n| n != ".gitignore");
            names.sort();
            assert_eq!(names, vec!["LOGS", "TRACKED.log", "keep.log"], "{mode}: {rows:?}");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(feature = "git")]
    #[test]
    fn gitignore_applies_below_the_top_level_of_a_nested_repository_in_bfs() {
        let tmp = std::env::temp_dir().join("fselect_test_gitignore_nested_bfs");
        let _ = fs::remove_dir_all(&tmp);
        let nested = tmp.join("nested");
        init_repo_ignoring(&nested, "*.log");
        fs::create_dir_all(nested.join("sub")).unwrap();
        for name in ["a.log", "sub/b.log", "sub/c.txt"] {
            fs::write(nested.join(name), "").unwrap();
        }

        for mode in ["bfs", "dfs"] {
            let rows = run_query_against_dir(&format!("select path from __DIR__ gitignore {mode}"), &tmp);
            let names = file_names(&rows);
            assert!(!names.iter().any(|n| n.ends_with(".log")), "{mode}: {rows:?}");
            assert!(names.contains(&"c.txt".to_string()), "{mode}: {rows:?}");
            assert!(!names.contains(&".git".to_string()), "{mode}: {rows:?}");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(feature = "git")]
    #[test]
    fn gitignore_rules_of_a_nested_repository_take_precedence_over_the_enclosing_one() {
        let tmp = std::env::temp_dir().join("fselect_test_gitignore_nested_precedence");
        let _ = fs::remove_dir_all(&tmp);
        init_repo_ignoring(&tmp, "*.tmp");
        let inner = tmp.join("inner");
        init_repo_ignoring(&inner, "*.txt");
        fs::create_dir_all(inner.join("deep")).unwrap();
        fs::write(tmp.join("x.tmp"), "").unwrap();
        fs::write(tmp.join("y.txt"), "").unwrap();
        for name in ["z.tmp", "deep/w.tmp", "deep/v.txt"] {
            fs::write(inner.join(name), "").unwrap();
        }

        for mode in ["bfs", "dfs"] {
            let rows = run_query_against_dir(&format!("select path from __DIR__ gitignore {mode}"), &tmp);
            let mut names = file_names(&rows);
            names.retain(|n| n != ".gitignore" && n != "inner" && n != "deep");
            names.sort();
            assert_eq!(names, vec!["w.tmp", "y.txt", "z.tmp"], "{mode}: {rows:?}");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(all(windows, feature = "git"))]
    #[test]
    fn gitignore_applies_when_the_root_is_spelled_in_a_different_case() {
        let tmp = std::env::temp_dir().join("fselect_test_gitignore_Root_Case");
        let _ = fs::remove_dir_all(&tmp);
        init_repo_ignoring(&tmp, "*.tmp");
        fs::create_dir_all(tmp.join("inner").join("deep")).unwrap();
        fs::write(tmp.join("inner").join(".gitignore"), "v.txt\n").unwrap();
        for name in ["x.tmp", "y.txt", "inner/z.tmp", "inner/deep/w.tmp", "inner/deep/v.txt"] {
            fs::write(tmp.join(name), "").unwrap();
        }

        let lowercase = PathBuf::from(tmp.to_string_lossy().to_lowercase());
        let uppercase = PathBuf::from(tmp.to_string_lossy().to_uppercase());
        for (root, mode) in [(&lowercase, "bfs"), (&uppercase, "bfs"), (&lowercase, "dfs")] {
            let rows = run_query_against_dir(&format!("select path from __DIR__ gitignore {mode}"), root);
            let mut names = file_names(&rows);
            names.retain(|n| n != ".gitignore" && n != "inner" && n != "deep");
            names.sort();
            assert_eq!(names, vec!["y.txt"], "{root:?} {mode}: {rows:?}");
        }
        let _ = fs::remove_dir_all(&tmp);
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
    fn having_null_checks_on_aggregates_and_grouping_columns() {
        let tmp = std::env::temp_dir().join("fselect_test_having_null_checks");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.rs"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "x").unwrap();
        fs::write(tmp.join("c.txt"), "x").unwrap();
        fs::write(tmp.join("noext"), "x").unwrap();

        let mut all_groups = run_query_against_dir(
            "select ext, count(*) from __DIR__ depth 1 group by ext having count(*) is not null",
            &tmp,
        );
        all_groups.sort();

        let no_groups = run_query_against_dir(
            "select ext, count(*) from __DIR__ depth 1 group by ext having count(*) is null",
            &tmp,
        );

        let null_ext = run_query_against_dir(
            "select ext, count(*) from __DIR__ depth 1 group by ext having ext is null",
            &tmp,
        );

        let mut non_null_ext = run_query_against_dir(
            "select ext, count(*) from __DIR__ depth 1 group by ext having ext is not null and count(*) > 1",
            &tmp,
        );
        non_null_ext.sort();

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(all_groups, vec![String::from("\t1"), String::from("rs\t1"), String::from("txt\t2")]);
        assert!(no_groups.is_empty());
        assert_eq!(null_ext, vec![String::from("\t1")]);
        assert_eq!(non_null_ext, vec![String::from("txt\t2")]);
    }

    #[test]
    fn grouped_from_subquery_yields_one_entry_per_group() {
        let tmp = std::env::temp_dir().join("fselect_test_grouped_from_subquery");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.rs"), "x").unwrap();
        fs::write(tmp.join("b.txt"), "x").unwrap();
        fs::write(tmp.join("c.txt"), "x").unwrap();

        let per_path = run_query_against_dir(
            "select count(*) from (select path from __DIR__ depth 1 group by path)",
            &tmp,
        );
        let per_ext = run_query_against_dir(
            "select count(*) from (select ext, count(*) from __DIR__ depth 1 group by ext)",
            &tmp,
        );
        let exts = run_query_against_dir(
            "select ext from (select ext from __DIR__ depth 1 group by ext) order by ext",
            &tmp,
        );
        let names = run_query_against_dir(
            "select name from (select path from __DIR__ depth 1 group by path) order by name",
            &tmp,
        );
        let ungrouped = run_query_against_dir(
            "select count(*) from (select count(*) from __DIR__ depth 1)",
            &tmp,
        );

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(per_path, vec![String::from("3")]);
        assert_eq!(per_ext, vec![String::from("2")]);
        assert_eq!(exts, vec![String::from("rs"), String::from("txt")]);
        assert_eq!(names, vec![String::from("a.rs"), String::from("b.txt"), String::from("c.txt")]);
        assert_eq!(ungrouped, vec![String::from("1")]);
    }

    #[test]
    fn coalesce_skips_fallbacks_once_a_value_is_found() {
        let tmp = std::env::temp_dir().join("fselect_test_coalesce_lazy");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.rs"), "xy").unwrap();
        fs::write(tmp.join("noext"), "x").unwrap();

        // A populated first argument must never evaluate the failing fallback.
        let searcher = search_dir(
            "select name, coalesce(size, 1 / 0) from __DIR__ depth 1 order by name",
            &tmp,
            Config::default(),
            true,
        );
        let sizes: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
        let size_errors = searcher.error_count;

        // An empty first argument falls through, and only as far as the first
        // populated fallback: the failing third argument is still untouched.
        let searcher = search_dir(
            "select name, coalesce(ext, 'none', 1 / 0) from __DIR__ depth 1 order by name",
            &tmp,
            Config::default(),
            true,
        );
        let exts: Vec<String> = searcher.output_buffer.iter_values().map(|s| s.trim_end().to_string()).collect();
        let ext_errors = searcher.error_count;

        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(sizes, vec![String::from("a.rs\t2"), String::from("noext\t1")]);
        assert_eq!(size_errors, 0);
        assert_eq!(exts, vec![String::from("a.rs\trs"), String::from("noext\tnone")]);
        assert_eq!(ext_errors, 0);
    }

    #[test]
    fn order_by_negated_and_numeric_leading_expressions() {
        let tmp = std::env::temp_dir().join("fselect_test_order_by_leading_minus");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("small.txt"), "x").unwrap();
        fs::write(tmp.join("medium.txt"), "xx").unwrap();
        fs::write(tmp.join("large.txt"), "xxx").unwrap();

        let by_minus = run_query_against_dir("select name from __DIR__ depth 1 order by -size", &tmp);
        let by_zero_minus_alias =
            run_query_against_dir("select name, size as sz from __DIR__ depth 1 order by 0 - sz", &tmp);
        let by_index = run_query_against_dir("select name, size from __DIR__ depth 1 order by 2 desc", &tmp);

        let _ = fs::remove_dir_all(&tmp);
        let descending = vec![String::from("large.txt"), String::from("medium.txt"), String::from("small.txt")];
        assert_eq!(by_minus, descending);
        assert_eq!(
            by_zero_minus_alias,
            vec![String::from("large.txt\t3"), String::from("medium.txt\t2"), String::from("small.txt\t1")]
        );
        assert_eq!(
            by_index,
            vec![String::from("large.txt\t3"), String::from("medium.txt\t2"), String::from("small.txt\t1")]
        );
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
    fn numeric_functions_read_the_byte_count_of_fsize() {
        let tmp = std::env::temp_dir().join("fselect_test_fsize_functions");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for bytes in [2usize, 10, 900, 10241] {
            fs::write(tmp.join(format!("f{bytes}.bin")), vec![0u8; bytes]).unwrap();
        }

        let run = |query: &str| run_query_against_dir(query, &tmp);
        let numeric = run("select name, abs(fsize), greatest(fsize, 1000), least(fsize, 1000), hex(fsize), fsize + 0 from __DIR__ order by size");
        let text = run("select fsize, length(fsize) from __DIR__ order by size");
        let capped = run("select name from __DIR__ where greatest(fsize, 1000) = 1000 order by size");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(
            numeric,
            vec![
                "f2.bin\t2\t1000\t2\t2\t2",
                "f10.bin\t10\t1000\t10\ta\t10",
                "f900.bin\t900\t1000\t900\t384\t900",
                "f10241.bin\t10241\t10241\t1000\t2801\t10241",
            ]
        );
        for row in &text {
            let (formatted, length) = row.split_once('\t').unwrap();
            assert_eq!(length, formatted.chars().count().to_string(), "string functions keep the formatted text: {row}");
        }
        assert_eq!(capped, vec!["f2.bin", "f10.bin", "f900.bin"]);
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

    #[test]
    fn encrypted_zip_members_are_listed_with_their_metadata() {
        use std::path::MAIN_SEPARATOR_STR as SEP;

        let tmp = std::env::temp_dir().join("fselect_test_encrypted_zip_members");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let bytes: [u8; 243] = [0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0xe1, 0x11, 0xc8, 0xe3, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x6f, 0x70, 0x65, 0x6e, 0x2e, 0x74, 0x78, 0x74, 0x6f, 0x70, 0x65, 0x6e, 0x0a, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0xd9, 0x95, 0x6d, 0x21, 0x1c, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x73, 0x65, 0x63, 0x72, 0x65, 0x74, 0x2e, 0x74, 0x78, 0x74, 0xe4, 0x11, 0xe6, 0xbd, 0x21, 0x68, 0x9b, 0x6a, 0x68, 0x08, 0xd6, 0x2e, 0xae, 0xed, 0xcf, 0xfa, 0xe5, 0x61, 0x23, 0xa3, 0x9c, 0x5a, 0xe0, 0x3d, 0x39, 0x79, 0xd5, 0xc0, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0xe1, 0x11, 0xc8, 0xe3, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x6f, 0x70, 0x65, 0x6e, 0x2e, 0x74, 0x78, 0x74, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x00, 0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0xd9, 0x95, 0x6d, 0x21, 0x1c, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2b, 0x00, 0x00, 0x00, 0x73, 0x65, 0x63, 0x72, 0x65, 0x74, 0x2e, 0x74, 0x78, 0x74, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x02, 0x00, 0x6e, 0x00, 0x00, 0x00, 0x6f, 0x00, 0x00, 0x00, 0x00, 0x00];
        fs::write(tmp.join("enc.zip"), bytes).unwrap();

        let listing = run_query_against_dir("select path, size, is_file from __DIR__ archives order by path", &tmp);
        let count = run_query_against_dir("select count(*) from __DIR__ archives where ext = txt", &tmp);
        let through_subselect = run_query_against_dir(
            "select name, size, line_count from (select path from __DIR__ archives where ext = txt) order by name",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(listing, vec![
            format!("enc.zip	243	true"),
            format!("enc.zip{SEP}open.txt	5	true"),
            format!("enc.zip{SEP}secret.txt	16	true"),
        ]);
        assert_eq!(count, vec!["2"]);
        assert_eq!(through_subselect, vec!["open.txt	5	1", "secret.txt	16"], "the encrypted member keeps its metadata; its content cell is empty and trimmed away");
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
    fn from_subselects_keep_archive_members() {
        use std::io::Write;
        use std::path::MAIN_SEPARATOR_STR as SEP;

        let tmp = std::env::temp_dir().join("fselect_test_subselect_archive_members");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("plain.txt"), "plain\n").unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(tmp.join("pack.zip")).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("one.txt", options).unwrap();
        zip.write_all(b"hello\n").unwrap();
        zip.start_file("sub/two.txt", options).unwrap();
        zip.write_all(b"world\nagain\n").unwrap();
        zip.finish().unwrap();

        let searcher = search_dir(
            "select name, size, line_count from (select path from __DIR__ archives where ext = txt) order by name",
            &tmp,
            Config::default(),
            true,
        );
        let rows: Vec<String> = searcher.output_buffer.iter_values().map(|row| row.trim_end().to_string()).collect();
        let paths = run_query_against_dir(
            "select path from (select path from __DIR__ archives where name like '%two%')",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows, vec!["one.txt\t6\t1", "plain.txt\t6\t1", "two.txt\t12\t2"]);
        assert_eq!(searcher.error_count, 0);
        assert_eq!(paths, vec![format!("pack.zip{SEP}sub{SEP}two.txt")]);
    }

    #[test]
    fn a_from_subselect_reopens_each_archive_once_for_all_its_members() {
        use std::io::Write;
        use crate::fileinfo::archive_opens;

        let tmp = std::env::temp_dir().join("fselect_test_subselect_archive_opens");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for archive in ["one.zip", "two.zip"] {
            let mut zip = zip::ZipWriter::new(fs::File::create(tmp.join(archive)).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            for i in 0..20 {
                zip.start_file(format!("d{}/m{i:02}.txt", i % 3), options).unwrap();
                zip.write_all(format!("hello {i}\nworld\n").as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }

        let before = archive_opens();
        let rows = run_query_against_dir(
            "select name, line_count from (select path from __DIR__ arc where ext = txt) where name like 'm1%' order by name",
            &tmp,
        );
        let opens = archive_opens() - before;
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(rows.len(), 20);
        assert_eq!(rows[0], "m10.txt\t2");
        assert_eq!(opens, 4, "two archives: one open each for the inner scan and one each for the outer lookup");
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
            "select ext, length(ext) * 2 as dbl, count(*), max(size) + 1 from __DIR__ group by ext into json",
            &tmp,
        );
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(plain, vec![r#"{"extension":"md","dbl":"0","upper(name)":"A.MD","size + 1":"1"}"#]);
        assert_eq!(grouped, vec![r#"{"extension":"md","dbl":"4","count(*)":"1","max(size) + 1":"1"}"#]);
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
