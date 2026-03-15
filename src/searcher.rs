//! Handles directory traversal and file processing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::fs::{DirEntry, FileType, Metadata};
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;
use chrono::{DateTime, Local};
#[cfg(feature = "git")]
use git2::Repository;
use lscolors::{LsColors, Style};
use mp3_metadata::MP3Metadata;
use regex::Regex;
#[cfg(all(unix, feature = "users"))]
use uzers::{Groups, Users, UsersCache};
#[cfg(unix)]
use xattr::FileExt;

use crate::config::Config;
use crate::expr::Expr;
use crate::field::Field;
use crate::fileinfo::{to_file_info, FileInfo};
use crate::function;
use crate::ignore::docker::{
    matches_dockerignore_filter, search_upstream_dockerignore, DockerignoreFilter,
};
use crate::ignore::hg::{matches_hgignore_filter, search_upstream_hgignore, HgignoreFilter};
use crate::mode;
use crate::operators::{LogicalOp, Op};
use crate::output::ResultsWriter;
use crate::query::TraversalMode::Bfs;
use crate::query::{Query, Root, TraversalMode};
use crate::util::dimensions::get_dimensions;
use crate::util::duration::get_duration;
use crate::util::*;
use crate::util::{Variant, VariantType};
use crate::util::error::{error_message, path_error_message, SearchError};

struct FileMetadataState {
    file_metadata: Option<Option<Metadata>>,
    line_count: Option<Option<usize>>,
    dimensions: Option<Option<Dimensions>>,
    duration: Option<Option<Duration>>,
    mp3_metadata: Option<Option<MP3Metadata>>,
    exif_metadata: Option<Option<HashMap<String, String>>>,
}

impl FileMetadataState {
    fn new() -> FileMetadataState {
        FileMetadataState {
            file_metadata: None,
            line_count: None,
            dimensions: None,
            duration: None,
            mp3_metadata: None,
            exif_metadata: None,
        }
    }

    fn clear(&mut self) {
        *self = Self::new();
    }

    fn update_file_metadata(&mut self, entry: &DirEntry, follow_symlinks: bool) {
        if self.file_metadata.is_none() {
            self.file_metadata = Some(get_metadata(entry, follow_symlinks));
        }
    }

    fn get_file_metadata(&self) -> Option<&Metadata> {
        self.file_metadata.as_ref().and_then(|o| o.as_ref())
    }

    fn get_file_metadata_as_option(&self) -> &Option<Metadata> {
        static NONE: Option<Metadata> = None;
        self.file_metadata.as_ref().unwrap_or(&NONE)
    }

    fn update_line_count(&mut self, entry: &DirEntry) {
        if self.line_count.is_none() {
            self.line_count = Some(get_line_count(entry));
        }
    }

    fn get_line_count(&self) -> Option<usize> {
        self.line_count.and_then(|o| o)
    }

    fn update_mp3_metadata(&mut self, entry: &DirEntry) {
        if self.mp3_metadata.is_none() {
            self.mp3_metadata = Some(get_mp3_metadata(entry));
        }
    }

    fn get_mp3_metadata(&self) -> Option<&MP3Metadata> {
        self.mp3_metadata.as_ref().and_then(|o| o.as_ref())
    }

    fn update_exif_metadata(&mut self, entry: &DirEntry) {
        if self.exif_metadata.is_none() {
            self.exif_metadata = Some(get_exif_metadata(entry));
        }
    }

    fn get_exif_metadata(&self) -> Option<&HashMap<String, String>> {
        self.exif_metadata.as_ref().and_then(|o| o.as_ref())
    }

    fn update_dimensions(&mut self, entry: &DirEntry) {
        if self.dimensions.is_none() {
            self.dimensions = Some(get_dimensions(entry.path()));
        }
    }

    fn get_dimensions(&self) -> Option<&Dimensions> {
        self.dimensions.as_ref().and_then(|o| o.as_ref())
    }

    fn update_duration(&mut self, entry: &DirEntry) {
        if self.duration.is_none() {
            self.update_mp3_metadata(entry);
            let mp3_flat = self.mp3_metadata.as_ref().unwrap_or(&None);
            self.duration = Some(get_duration(entry.path(), mp3_flat));
        }
    }

    fn get_duration(&self) -> Option<&Duration> {
        self.duration.as_ref().and_then(|o| o.as_ref())
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
    dir_queue: Box<VecDeque<PathBuf>>,
    current_follow_symlinks: bool,

    fms: FileMetadataState,
    conforms_map: HashMap<String, String>,
    subquery_cache: HashMap<String, Vec<String>>,
    silent_mode: bool,

    pub error_count: i32,
}

static FIELD_WITH_ALIAS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^([a-zA-Z0-9_]+)\\.([a-zA-Z0-9_]+)$").unwrap()
});

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
            dir_queue: Box::from(VecDeque::new()),
            current_follow_symlinks: false,

            fms: FileMetadataState::new(),
            conforms_map: HashMap::new(),
            subquery_cache: HashMap::new(),
            silent_mode: false,

            error_count: 0,
        }
    }

    pub fn is_buffered(&self) -> bool {
        self.has_ordering() || self.has_aggregate_column() || self.silent_mode
    }

    fn has_ordering(&self) -> bool {
        self.query.is_ordered()
    }

    fn has_aggregate_column(&self) -> bool {
        self.query.has_aggregate_column()
    }

    /// Searches directories based on configured query and outputs results to stdout.
    pub fn list_search_results(&mut self) -> Result<(), SearchError> {
        let current_dir = std::env::current_dir()?;

        if !self.silent_mode {
            let raw_query = self.query.raw_query.clone();
            let col_count = self.query.fields.len();
            if let Err(e) = self.results_writer.write_header(raw_query, col_count, &mut std::io::stdout()) {
                if e.kind() == ErrorKind::BrokenPipe {
                    return Ok(());
                }
            }
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
                        let rx = Regex::new(&rx_string).unwrap();
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

                        ext_roots.clear();
                        ext_roots.append(&mut tmp);
                    } else if ext_roots.is_empty() {
                        ext_roots.push(part.to_string());
                    } else {
                        //update all roots
                        let mut new_roots = ext_roots
                            .iter()
                            .map(|root| root.to_string() + "/" + part)
                            .collect::<Vec<String>>();
                        ext_roots.clear();
                        ext_roots.append(&mut new_roots);
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

            let root_dir = Path::new(&root.path);
            let min_depth = root.options.min_depth;
            let max_depth = root.options.max_depth;
            let search_archives = root.options.archives;
            let apply_gitignore = root
                .options
                .gitignore
                .unwrap_or(self.config.gitignore.unwrap_or(false));
            let apply_hgignore = root
                .options
                .hgignore
                .unwrap_or(self.config.hgignore.unwrap_or(false));
            let apply_dockerignore = root
                .options
                .dockerignore
                .unwrap_or(self.config.dockerignore.unwrap_or(false));
            let traversal_mode = root.options.traversal;

            self.dir_queue.clear();
            self.visited_dirs.clear();
            self.hgignore_filters.clear();
            self.dockerignore_filters.clear();

            // Apply filters
            if apply_hgignore {
                search_upstream_hgignore(&mut self.hgignore_filters, root_dir);
            }

            if apply_dockerignore {
                search_upstream_dockerignore(&mut self.dockerignore_filters, root_dir);
            }

            let result = self.visit_dir(
                root_dir,
                min_depth,
                max_depth,
                0,
                search_archives,
                apply_gitignore,
                #[cfg(feature = "git")]
                Repository::discover(&root_dir).ok().as_ref(),
                apply_hgignore,
                apply_dockerignore,
                traversal_mode,
                true,
                root_dir,
            );

            if let Err(err) = result {
                if err.is_fatal() {
                    return Err(err);
                }
            }
        }

        let compute_time = std::time::Instant::now();

        // ======== Compute results =========
        if self.has_aggregate_column() {
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
                    let _ = self.results_writer.write_row(&mut buf, items.clone());
                    let rendered = String::from(buf);
                    self.output_buffer.insert(
                        Criteria::new(Rc::new(vec![]), vec![], Rc::new(vec![])),
                        rendered.clone(),
                    );
                    if !self.silent_mode {
                        if !first {
                            let _ = self.results_writer.write_row_separator(&mut std::io::stdout());
                        }
                        first = false;
                        let _ = write!(std::io::stdout(), "{}", rendered);
                    }
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
                    if let Err(e) = write!(std::io::stdout(), "{}", rendered) {
                        if e.kind() == ErrorKind::BrokenPipe {
                            return Ok(());
                        }
                    }
                }
            }
        } else if self.is_buffered() && !self.silent_mode {
            let mut first = true;
            for piece in self.output_buffer.iter_values().skip(self.query.offset as usize) {
                if first {
                    first = false;
                } else if let Err(e) = self
                    .results_writer
                    .write_row_separator(&mut std::io::stdout())
                {
                    if e.kind() == ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                }
                if let Err(e) = write!(std::io::stdout(), "{}", piece) {
                    if e.kind() == ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                }
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
        min_depth: u32,
        max_depth: u32,
        root_depth: u32,
        search_archives: bool,
        apply_gitignore: bool,
        #[cfg(feature = "git")]
        git_repository: Option<&Repository>,
        apply_hgignore: bool,
        apply_dockerignore: bool,
        traversal_mode: TraversalMode,
        process_queue: bool,
        root_dir: &Path,
    ) -> Result<(), SearchError> {
        let canonical_path = match crate::util::canonical_path(&dir.to_path_buf()) {
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
        let canonical_depth = crate::util::calc_depth(&canonical_path);

        let base_depth = match root_depth {
            0 => canonical_depth,
            _ => root_depth,
        };

        let depth = canonical_depth.saturating_sub(base_depth) + 1;

        // Read the directory and process each entry
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
                            let pass_ignores = if apply_gitignore || apply_hgignore || apply_dockerignore {
                                let mut canonical_path = path.clone();

                                if let Ok(canonicalized) = crate::util::canonical_path(&path) {
                                    canonical_path = PathBuf::from(canonicalized);
                                }

                                // Check the path against the filters
                                #[cfg(feature = "git")]
                                let pass_gitignore = !apply_gitignore
                                    || !(git_repository.is_some() &&
                                    git_repository.unwrap().is_path_ignored(&canonical_path)
                                        .unwrap_or(false));
                                #[cfg(not(feature = "git"))]
                                let pass_gitignore = true;

                                let pass_hgignore = !apply_hgignore
                                    || !matches_hgignore_filter(
                                    &self.hgignore_filters,
                                    canonical_path.to_string_lossy().as_ref(),
                                );
                                let pass_dockerignore = !apply_dockerignore
                                    || !matches_dockerignore_filter(
                                    &self.dockerignore_filters,
                                    canonical_path.to_string_lossy().as_ref(),
                                );

                                pass_gitignore && pass_hgignore && pass_dockerignore
                            } else {
                                true
                            };                            

                            // If the path passes the filters, process it
                            if pass_ignores {
                                if min_depth == 0 || depth >= min_depth {
                                    let checked = self.check_file(&entry, root_dir, &None);
                                    match checked {
                                        Err(mut err) => {
                                            if err.is_fatal() {
                                                return Err(err);
                                            }
                                            self.error_count += 1;
                                            if err.source.is_empty() {
                                                err.source = path.to_string_lossy().to_string();
                                            }
                                            err.print();
                                            continue;
                                        }
                                        Ok(()) => {}
                                    }

                                    if search_archives
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
                                                        match self.check_file(&entry, root_dir, &Some(file_info)) {
                                                            Err(mut err) => {
                                                                if err.is_fatal() {
                                                                    return Err(err);
                                                                }
                                                                self.error_count += 1;
                                                                if err.source.is_empty() {
                                                                    err.source = path.to_string_lossy().to_string();
                                                                }
                                                                err.print();
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
                                if max_depth == 0 || depth < max_depth {
                                    let result = entry.file_type();
                                    if let Ok(file_type) = result {
                                        let mut ok = false;

                                        if file_type.is_symlink() && self.current_follow_symlinks {
                                            if let Ok(resolved) = std::fs::read_link(&path) {
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
                                            if traversal_mode == TraversalMode::Dfs {
                                                #[cfg(feature = "git")]
                                                let repo;
                                                #[cfg(feature = "git")]
                                                let git_repository = match git_repository {
                                                    Some(repo) => Some(repo),
                                                    None if apply_gitignore => {
                                                        repo = Repository::open(&path).ok();
                                                        repo.as_ref()
                                                    },
                                                    _ => None,
                                                };
                                                let result = self.visit_dir(
                                                    &path,
                                                    min_depth,
                                                    max_depth,
                                                    base_depth,
                                                    search_archives,
                                                    apply_gitignore,
                                                    #[cfg(feature = "git")]
                                                    git_repository,
                                                    apply_hgignore,
                                                    apply_dockerignore,
                                                    traversal_mode,
                                                    false,
                                                    root_dir,
                                                );

                                                if let Err(mut err) = result {
                                                    if err.is_fatal() {
                                                        return Err(err);
                                                    }
                                                    self.error_count += 1;
                                                    if err.source.is_empty() {
                                                        err.source = path.to_string_lossy().to_string();
                                                    }
                                                    err.print();
                                                }
                                            } else {
                                                self.dir_queue.push_back(path);
                                            }
                                        }
                                    } else {
                                        self.error_count += 1;
                                        path_error_message(&path, result.err().unwrap());
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

        if traversal_mode == Bfs && process_queue {
            while !self.dir_queue.is_empty() {
                let path = self.dir_queue.pop_front().unwrap();
                #[cfg(feature = "git")]
                let repo;
                #[cfg(feature = "git")]
                let git_repository = match git_repository {
                    Some(repo) => Some(repo),
                    None if apply_gitignore => {
                        repo = Repository::open(&path).ok();
                        repo.as_ref()
                    },
                    _ => None,
                };
                let result = self.visit_dir(
                    &path,
                    min_depth,
                    max_depth,
                    base_depth,
                    search_archives,
                    apply_gitignore,
                    #[cfg(feature = "git")]
                    git_repository,
                    apply_hgignore,
                    apply_dockerignore,
                    traversal_mode,
                    false,
                    root_dir,
                );

                if let Err(mut err) = result {
                    if err.is_fatal() {
                        return Err(err);
                    }
                    self.error_count += 1;
                    if err.source.is_empty() {
                        err.source = path.to_string_lossy().to_string();
                    }
                    err.print();
                }
            }
        }

        Ok(())
    }

    fn ok_to_visit_dir(&mut self, file_type: FileType) -> bool {
        match self.current_follow_symlinks {
            true => true,
            false => !file_type.is_symlink(),
        }
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
                let context_entry = context.entry(context_key.to_string()).or_insert(HashMap::new());
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

        let function = &column_expr.function.as_ref().unwrap();

        if function.is_aggregate_function() {
            let _ = self.get_column_expr_value(entry, file_info, root_path, file_map, accumulator, left_expr)?;
            let buffer_key = left_expr.to_string();
            let empty_acc = function::GroupAccumulator::default();
            let aggr_result = function::get_aggregate_value(
                &column_expr.function.as_ref().unwrap(),
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
                &column_expr.function.as_ref().unwrap(),
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
        if file_info.is_some() && !field.is_available_for_archived_files() {
            return Ok(Variant::empty(VariantType::String));
        }

        match field {
            Field::Name => return match file_info {
                Some(file_info) => {
                    Ok(Variant::from_string(&format!(
                        "[{}] {}",
                        entry.file_name().to_string_lossy(),
                        file_info.name
                    )))
                }
                _ => {
                    Ok(Variant::from_string(&entry.file_name().to_string_lossy().to_string()))
                }
            },
            Field::Filename => return match file_info {
                Some(file_info) => {
                    Ok(Variant::from_string(&format!(
                        "[{}] {}",
                        entry.file_name().to_string_lossy(),
                        get_stem(&file_info.name)
                    )))
                }
                _ => {
                    Ok(Variant::from_string(
                        &get_stem(&entry.file_name().to_string_lossy())
                            .to_string(),
                    ))
                }
            },
            Field::Extension => return match file_info {
                Some(file_info) => {
                    Ok(Variant::from_string(&format!(
                        "[{}] {}",
                        entry.file_name().to_string_lossy(),
                        crate::util::get_extension(&file_info.name)
                    )))
                }
                _ => {
                    Ok(Variant::from_string(
                        &crate::util::get_extension(&entry.file_name().to_string_lossy())
                            .to_string(),
                    ))
                }
            },
            Field::Path => return match file_info {
                Some(file_info) => {
                    Ok(Variant::from_string(&format!(
                        "[{}] {}",
                        entry.path().to_string_lossy(),
                        file_info.name
                    )))
                }
                _ => {
                    match entry.path().strip_prefix(root_path) {
                        Ok(stripped_path) => {
                            Ok(Variant::from_string(&stripped_path.to_string_lossy().to_string()))
                        }
                        Err(_) => {
                            Ok(Variant::from_string(&entry.path().to_string_lossy().to_string()))
                        }
                    }
                }
            },
            Field::AbsPath => return match file_info {
                Some(file_info) => {
                    Ok(Variant::from_string(&format!(
                        "[{}] {}",
                        entry.path().to_string_lossy(),
                        file_info.name
                    )))
                }
                _ => {
                    match crate::util::canonical_path(&entry.path()) {
                        Ok(path) => {
                            Ok(Variant::from_string(&path))
                        },
                        Err(e) => {
                            Err(format!("could not get absolute path: {}", e).into())
                        }
                    }
                }
            },
            Field::Directory => {
                let file_path = match file_info {
                    Some(file_info) => file_info.name.clone(),
                    _ => match entry.path().strip_prefix(root_path) {
                        Ok(relative_path) => relative_path.to_string_lossy().to_string(),
                        Err(_) => entry.path().to_string_lossy().to_string()
                    },
                };
                let pb = PathBuf::from(file_path);
                if let Some(parent) = pb.parent() {
                    return Ok(Variant::from_string(&parent.to_string_lossy().to_string()));
                }
            }
            Field::AbsDir => {
                let file_path = match file_info {
                    Some(file_info) => file_info.name.clone(),
                    _ => entry.path().to_string_lossy().to_string(),
                };
                let pb = PathBuf::from(file_path);
                if let Some(parent) = pb.parent() {
                    if file_info.is_some() {
                        return Ok(Variant::from_string(&parent.to_string_lossy().to_string()));
                    }

                    if let Ok(path) = crate::util::canonical_path(&parent.to_path_buf()) {
                        return Ok(Variant::from_string(&path));
                    }
                }
            }
            Field::Size => match file_info {
                Some(file_info) => {
                    return Ok(Variant::from_int(file_info.size as i64));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_int(attrs.len() as i64));
                    }
                }
            },
            Field::FormattedSize => match file_info {
                Some(file_info) => {
                    return Ok(Variant::from_string(&format_filesize(
                        file_info.size,
                        self.config
                            .default_file_size_format
                            .as_ref()
                            .unwrap_or(&String::new()),
                    )?));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_string(&format_filesize(
                            attrs.len(),
                            self.config
                                .default_file_size_format
                                .as_ref()
                                .unwrap_or(&String::new()),
                        )?));
                    }
                }
            },
            Field::IsDir => match file_info {
                Some(file_info) => {
                    return Ok(Variant::from_bool(
                        file_info.name.ends_with('/') || file_info.name.ends_with('\\'),
                    ));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_bool(attrs.is_dir()));
                    }
                }
            },
            Field::IsFile => match file_info {
                Some(file_info) => {
                    return Ok(Variant::from_bool(!file_info.name.ends_with('/') && !file_info.name.ends_with('\\')));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_bool(attrs.is_file()));
                    }
                }
            },
            Field::IsSymlink => match file_info {
                Some(_) => {
                    return Ok(Variant::from_bool(false));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_bool(attrs.file_type().is_symlink()));
                    }
                }
            },
            Field::IsPipe => {
                return Ok(self.check_file_mode(entry, &mode::is_pipe, file_info, &mode::mode_is_pipe));
            }
            Field::IsCharacterDevice => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::is_char_device,
                    file_info,
                    &mode::mode_is_char_device,
                ));
            }
            Field::IsBlockDevice => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::is_block_device,
                    file_info,
                    &mode::mode_is_block_device,
                ));
            }
            Field::IsSocket => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::is_socket,
                    file_info,
                    &mode::mode_is_socket,
                ));
            }
            Field::Device => {
                #[cfg(unix)]
                {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_int(attrs.dev() as i64));
                    }
                }

                return Ok(Variant::empty(VariantType::String));
            }
            Field::Inode => {
                #[cfg(unix)]
                {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_int(attrs.ino() as i64));
                    }
                }

                return Ok(Variant::empty(VariantType::String));
            }
            Field::Blocks => {
                #[cfg(unix)]
                {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_int(attrs.blocks() as i64));
                    }
                }

                return Ok(Variant::empty(VariantType::String));
            }
            Field::Hardlinks => {
                #[cfg(unix)]
                {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_int(attrs.nlink() as i64));
                    }
                }

                return Ok(Variant::empty(VariantType::String));
            }
            Field::Mode => match file_info {
                Some(file_info) => {
                    if let Some(mode) = file_info.mode {
                        return Ok(Variant::from_string(&mode::format_mode(mode)));
                    }
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return Ok(Variant::from_string(&mode::get_mode(attrs)));
                    }
                }
            },
            Field::UserRead => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::user_read,
                    file_info,
                    &mode::mode_user_read,
                ));
            }
            Field::UserWrite => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::user_write,
                    file_info,
                    &mode::mode_user_write,
                ));
            }
            Field::UserExec => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::user_exec,
                    file_info,
                    &mode::mode_user_exec,
                ));
            }
            Field::UserAll => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::user_all,
                    file_info,
                    &mode::mode_user_all,
                ));
            }
            Field::GroupRead => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::group_read,
                    file_info,
                    &mode::mode_group_read,
                ));
            }
            Field::GroupWrite => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::group_write,
                    file_info,
                    &mode::mode_group_write,
                ));
            }
            Field::GroupExec => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::group_exec,
                    file_info,
                    &mode::mode_group_exec,
                ));
            }
            Field::GroupAll => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::group_all,
                    file_info,
                    &mode::mode_group_all,
                ));
            }
            Field::OtherRead => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::other_read,
                    file_info,
                    &mode::mode_other_read,
                ));
            }
            Field::OtherWrite => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::other_write,
                    file_info,
                    &mode::mode_other_write,
                ));
            }
            Field::OtherExec => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::other_exec,
                    file_info,
                    &mode::mode_other_exec,
                ));
            }
            Field::OtherAll => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::other_all,
                    file_info,
                    &mode::mode_other_all,
                ));
            }
            Field::Suid => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::suid_bit_set,
                    file_info,
                    &mode::mode_suid,
                ));
            }
            Field::Sgid => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::sgid_bit_set,
                    file_info,
                    &mode::mode_sgid,
                ));
            }
            Field::IsSticky => {
                return Ok(self.check_file_mode(
                    entry,
                    &mode::sticky_bit_set,
                    file_info,
                    &mode::mode_sticky,
                ));
            }
            Field::IsHidden => match file_info {
                Some(file_info) => {
                    return Ok(Variant::from_bool(is_hidden(&file_info.name, &None, true)));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    return Ok(Variant::from_bool(is_hidden(
                        &entry.file_name().to_string_lossy(),
                        self.fms.get_file_metadata_as_option(),
                        false,
                    )));
                }
            },
            Field::Uid => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(attrs) = self.fms.get_file_metadata() {
                    if let Some(uid) = mode::get_uid(attrs) {
                        return Ok(Variant::from_int(uid as i64));
                    }
                }
            }
            Field::Gid => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(attrs) = self.fms.get_file_metadata() {
                    if let Some(gid) = mode::get_gid(attrs) {
                        return Ok(Variant::from_int(gid as i64));
                    }
                }
            }
            #[cfg(all(unix, feature = "users"))]
            Field::User => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(attrs) = self.fms.get_file_metadata() {
                    if let Some(uid) = mode::get_uid(attrs) {
                        if let Some(user) = self.user_cache.get_user_by_uid(uid) {
                            return Ok(Variant::from_string(
                                &user.name().to_string_lossy().to_string(),
                            ));
                        }
                    }
                }
            }
            #[cfg(all(unix, feature = "users"))]
            Field::Group => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(attrs) = self.fms.get_file_metadata() {
                    if let Some(gid) = mode::get_gid(attrs) {
                        if let Some(group) = self.user_cache.get_group_by_gid(gid) {
                            return Ok(Variant::from_string(
                                &group.name().to_string_lossy().to_string(),
                            ));
                        }
                    }
                }
            }
            Field::Created => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(attrs) = self.fms.get_file_metadata() {
                    if let Ok(sdt) = attrs.created() {
                        let dt: DateTime<Local> = DateTime::from(sdt);
                        return Ok(Variant::from_datetime(dt.naive_local()));
                    }
                }
            }
            Field::Accessed => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(attrs) = self.fms.get_file_metadata() {
                    if let Ok(sdt) = attrs.accessed() {
                        let dt: DateTime<Local> = DateTime::from(sdt);
                        return Ok(Variant::from_datetime(dt.naive_local()));
                    }
                }
            }
            Field::Modified => match file_info {
                Some(file_info) => {
                    if let Some(file_info_modified) = &file_info.modified {
                        let dt = to_local_datetime(file_info_modified);
                        return Ok(Variant::from_datetime(dt));
                    }
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        if let Ok(sdt) = attrs.modified() {
                            let dt: DateTime<Local> = DateTime::from(sdt);
                            return Ok(Variant::from_datetime(dt.naive_local()));
                        }
                    }
                }
            },
            Field::HasXattrs => {
                #[cfg(unix)]
                {
                    if let Ok(file) = fs::File::open(entry.path()) {
                        if let Ok(xattrs) = file.list_xattr() {
                            let has_xattrs = xattrs.count() > 0;
                            return Ok(Variant::from_bool(has_xattrs));
                        }
                    }
                }

                #[cfg(not(unix))]
                {
                    return Ok(Variant::from_bool(false));
                }
            }
            Field::Extattrs => {
                #[cfg(target_os = "linux")]
                {
                    if let Ok(file) = fs::File::open(entry.path()) {
                        if let Some(flags) = crate::util::extattrs::get_ext_attrs(&file) {
                            return Ok(Variant::from_string(
                                &crate::util::extattrs::format_ext_attrs(flags),
                            ));
                        }
                    }
                }

                return Ok(Variant::empty(VariantType::String));
            }
            Field::HasExtattrs => {
                #[cfg(target_os = "linux")]
                {
                    if let Ok(file) = fs::File::open(entry.path()) {
                        if let Some(flags) = crate::util::extattrs::get_ext_attrs(&file) {
                            return Ok(Variant::from_bool(flags != 0));
                        }
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    return Ok(Variant::from_bool(false));
                }
            }
            Field::HasAcl => {
                #[cfg(target_os = "linux")]
                {
                    if let Ok(file) = fs::File::open(entry.path()) {
                        if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_access") {
                            if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                                return Ok(Variant::from_bool(!entries.is_empty()));
                            }
                        }
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    return Ok(Variant::from_bool(false));
                }
            }
            Field::HasDefaultAcl => {
                #[cfg(target_os = "linux")]
                {
                    if entry.path().is_dir() {
                        if let Ok(file) = fs::File::open(entry.path()) {
                            if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_default") {
                                if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                                    return Ok(Variant::from_bool(!entries.is_empty()));
                                }
                            }
                        }
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    return Ok(Variant::from_bool(false));
                }
            }
            Field::Capabilities => {
                #[cfg(target_os = "linux")]
                {
                    if let Ok(file) = fs::File::open(entry.path()) {
                        if let Ok(Some(caps_xattr)) = file.get_xattr("security.capability") {
                            let caps_string =
                                crate::util::capabilities::parse_capabilities(caps_xattr);
                            return Ok(Variant::from_string(&caps_string));
                        }
                    }
                }

                return Ok(Variant::empty(VariantType::String));
            }
            Field::IsShebang => {
                return Ok(Variant::from_bool(is_shebang(&entry.path())));
            }
            Field::IsEmpty => match file_info {
                Some(file_info) => {
                    return Ok(Variant::from_bool(file_info.size == 0));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(attrs) = self.fms.get_file_metadata() {
                        return match attrs.is_dir() {
                            true => match is_dir_empty(entry) {
                                Some(result) => Ok(Variant::from_bool(result)),
                                None => Ok(Variant::empty(VariantType::Bool)),
                            },
                            false => Ok(Variant::from_bool(attrs.len() == 0)),
                        };
                    }
                }
            },
            Field::Width => {
                self.fms.update_dimensions(entry);

                if let Some(&Dimensions { width, .. }) = self.fms.get_dimensions() {
                    return Ok(Variant::from_int(width as i64));
                }
            }
            Field::Height => {
                self.fms.update_dimensions(entry);

                if let Some(&Dimensions { height, .. }) = self.fms.get_dimensions() {
                    return Ok(Variant::from_int(height as i64));
                }
            }
            Field::Duration => {
                self.fms.update_duration(entry);

                if let Some(&Duration { length, .. }) = self.fms.get_duration() {
                    return Ok(Variant::from_int(length as i64));
                }
            }
            Field::Bitrate => {
                self.fms.update_mp3_metadata(entry);

                if let Some(mp3_info) = self.fms.get_mp3_metadata() {
                    return Ok(Variant::from_int(mp3_info.frames[0].bitrate as i64));
                }
            }
            Field::Freq => {
                self.fms.update_mp3_metadata(entry);

                if let Some(mp3_info) = self.fms.get_mp3_metadata() {
                    return Ok(Variant::from_int(mp3_info.frames[0].sampling_freq as i64));
                }
            }
            Field::Title => {
                self.fms.update_mp3_metadata(entry);

                if let Some(mp3_info) = self.fms.get_mp3_metadata() {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Ok(Variant::from_string(&mp3_tag.title));
                    }
                }
            }
            Field::Artist => {
                self.fms.update_mp3_metadata(entry);

                if let Some(mp3_info) = self.fms.get_mp3_metadata() {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Ok(Variant::from_string(&mp3_tag.artist));
                    }
                }
            }
            Field::Album => {
                self.fms.update_mp3_metadata(entry);

                if let Some(mp3_info) = self.fms.get_mp3_metadata() {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Ok(Variant::from_string(&mp3_tag.album));
                    }
                }
            }
            Field::Year => {
                self.fms.update_mp3_metadata(entry);

                if let Some(mp3_info) = self.fms.get_mp3_metadata() {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Ok(Variant::from_int(mp3_tag.year as i64));
                    }
                }
            }
            Field::Genre => {
                self.fms.update_mp3_metadata(entry);

                if let Some(mp3_info) = self.fms.get_mp3_metadata() {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Ok(Variant::from_string(&format!("{:?}", mp3_tag.genre)));
                    }
                }
            }
            Field::ExifDateTime => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("DateTime") {
                        if let Ok(exif_datetime) = parse_datetime(exif_value) {
                            return Ok(Variant::from_datetime(exif_datetime.0));
                        }
                    }
                }
            }
            Field::ExifGpsAltitude => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("__Alt") {
                        return Ok(Variant::from_float(exif_value.parse().unwrap_or(0.0)));
                    }
                }
            }
            Field::ExifGpsLatitude => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("__Lat") {
                        return Ok(Variant::from_float(exif_value.parse().unwrap_or(0.0)));
                    }
                }
            }
            Field::ExifGpsLongitude => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("__Lng") {
                        return Ok(Variant::from_float(exif_value.parse().unwrap_or(0.0)));
                    }
                }
            }
            Field::ExifMake => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("Make") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifModel => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("Model") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifSoftware => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("Software") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifVersion => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("ExifVersion") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifExposureTime => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("ExposureTime") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifAperture => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("ApertureValue") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifShutterSpeed => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("ShutterSpeedValue") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifFNumber => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("FNumber") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifIsoSpeed => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("ISOSpeed") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifFocalLength => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("FocalLength") {
                        return Ok( Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifLensMake => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("LensMake") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::ExifLensModel => {
                self.fms.update_exif_metadata(entry);

                if let Some(exif_info) = self.fms.get_exif_metadata() {
                    if let Some(exif_value) = exif_info.get("LensModel") {
                        return Ok(Variant::from_string(exif_value));
                    }
                }
            }
            Field::LineCount => {
                self.fms.update_line_count(entry);

                if let Some(line_count) = self.fms.get_line_count() {
                    return Ok(Variant::from_int(line_count as i64));
                }
            }
            Field::Mime => {
                if let Some(mime) = tree_magic_mini::from_filepath(&entry.path()) {
                    return Ok(Variant::from_string(&String::from(mime)));
                }

                return Ok(Variant::empty(VariantType::String));
            }
            Field::IsBinary => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(meta) = self.fms.get_file_metadata() {
                    if meta.is_dir() {
                        return Ok(Variant::from_bool(false));
                    }
                }

                if let Some(mime) = tree_magic_mini::from_filepath(&entry.path()) {
                    let is_binary = !is_text_mime(mime);
                    return Ok(Variant::from_bool(is_binary));
                }

                return Ok(Variant::from_bool(false));
            }
            Field::IsText => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(meta) = self.fms.get_file_metadata() {
                    if meta.is_dir() {
                        return Ok(Variant::from_bool(false));
                    }
                }

                if let Some(mime) = tree_magic_mini::from_filepath(&entry.path()) {
                    let is_text = is_text_mime(mime);
                    return Ok(Variant::from_bool(is_text));
                }

                return Ok(Variant::from_bool(false));
            }
            Field::IsArchive => {
                let is_archive = match file_info {
                    Some(file_info) => self.is_archive(&file_info.name),
                    None => self.is_archive(&entry.file_name().to_string_lossy()),
                };

                return Ok(Variant::from_bool(is_archive));
            }
            Field::IsAudio => {
                let is_audio = match file_info {
                    Some(file_info) => self.is_audio(&file_info.name),
                    None => self.is_audio(&entry.file_name().to_string_lossy()),
                };

                return Ok(Variant::from_bool(is_audio));
            }
            Field::IsBook => {
                let is_book = match file_info {
                    Some(file_info) => self.is_book(&file_info.name),
                    None => self.is_book(&entry.file_name().to_string_lossy()),
                };

                return Ok(Variant::from_bool(is_book));
            }
            Field::IsDoc => {
                let is_doc = match file_info {
                    Some(file_info) => self.is_doc(&file_info.name),
                    None => self.is_doc(&entry.file_name().to_string_lossy()),
                };

                return Ok(Variant::from_bool(is_doc));
            }
            Field::IsFont => {
                let is_font = match file_info {
                    Some(file_info) => self.is_font(&file_info.name),
                    None => self.is_font(&entry.file_name().to_string_lossy()),
                };

                return Ok(Variant::from_bool(is_font));
            }
            Field::IsImage => {
                let is_image = match file_info {
                    Some(file_info) => self.is_image(&file_info.name),
                    None => self.is_image(&entry.file_name().to_string_lossy()),
                };

                return Ok(Variant::from_bool(is_image));
            }
            Field::IsSource => {
                let is_source = match file_info {
                    Some(file_info) => self.is_source(&file_info.name),
                    None => self.is_source(&entry.file_name().to_string_lossy()),
                };

                return Ok(Variant::from_bool(is_source));
            }
            Field::IsVideo => {
                let is_video = match file_info {
                    Some(file_info) => self.is_video(&file_info.name),
                    None => self.is_video(&entry.file_name().to_string_lossy()),
                };

                return Ok(Variant::from_bool(is_video));
            }
            Field::Sha1 => {
                return Ok(Variant::from_string(&crate::util::get_sha1_file_hash(entry)));
            }
            Field::Sha256 => {
                return Ok(Variant::from_string(&crate::util::get_sha256_file_hash(entry)));
            }
            Field::Sha512 => {
                return Ok(Variant::from_string(&crate::util::get_sha512_file_hash(entry)));
            }
            Field::Sha3 => {
                return Ok(Variant::from_string(&crate::util::get_sha3_512_file_hash(entry)));
            }
        };

        Ok(Variant::empty(VariantType::String))
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

            if let Some(ref required_fields) = self.subquery_required_fields.clone() {
                let mut field_values = HashMap::new();
                for (field, alias) in required_fields {
                    let field_value = self.get_field_value(entry, file_info, root_path, field).unwrap_or(Variant::empty(VariantType::String));
                    field_values.insert(alias.clone(), field_value);
                }

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

        if self.has_aggregate_column() {
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
        } else if let Err(e) = write!(std::io::stdout(), "{}", String::from(buf)) {
            if e.kind() == ErrorKind::BrokenPipe {
                return Err(SearchError::fatal("broken pipe").with_source("output"));
            }
        }

        Ok(())
    }

    fn colorize(&mut self, value: &str) -> String {
        let style;

        if let Some(metadata) = self.fms.get_file_metadata() {
            style = self
                .lscolors
                .style_for_path_with_metadata(Path::new(&value), Some(metadata));
        } else {
            style = self.lscolors.style_for_path(Path::new(&value));
        }

        let ansi_style = style.map(Style::to_nu_ansi_term_style).unwrap_or_default();

        format!("{}", ansi_style.paint(value))
    }

    fn check_file_mode(
        &mut self,
        entry: &DirEntry,
        mode_func_boxed: &dyn Fn(&Metadata) -> bool,
        file_info: &Option<FileInfo>,
        mode_func_i32: &dyn Fn(u32) -> bool,
    ) -> Variant {
        match file_info {
            Some(file_info) => {
                if let Some(mode) = file_info.mode {
                    return Variant::from_bool(mode_func_i32(mode));
                }
            }
            _ => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(attrs) = self.fms.get_file_metadata() {
                    return Variant::from_bool(mode_func_boxed(attrs));
                }
            }
        }

        Variant::from_bool(false)
    }

    fn get_in_args(&mut self, expr: &Expr) -> Vec<Expr> {
        let right = expr.right.as_ref().unwrap().clone();
        match right.args {
            Some(args) => args,
            None => {
                if let Some(subquery) = right.subquery {
                    self.get_list_from_subquery(*subquery).iter().map(|s| {
                        Expr::value(s.to_string())
                    }).collect()
                } else {
                    vec![]
                }
            }
        }
    }

    fn check_exists(&mut self, expr: &Expr) -> bool {
        let right = expr.right.as_ref().unwrap().clone();
        match right.args {
            Some(args) => !args.is_empty(),
            None => {
                if let Some(mut subquery) = right.subquery {
                    if subquery.grouping_fields.is_empty() {
                        subquery.limit = 1;
                    }
                    !self.get_list_from_subquery(*subquery).is_empty()
                } else {
                    false
                }
            }
        }
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

            match logical_op {
                LogicalOp::And => {
                    if !left_result {
                        result = false;
                    } else {
                        result = match expr.right {
                            Some(ref right) => self.conforms(entry, file_info, root_path, right)?,
                            None => false,
                        };
                    }
                }
                LogicalOp::Or => {
                    if left_result {
                        result = true;
                    } else {
                        result = match expr.right {
                            Some(ref right) => self.conforms(entry, file_info, root_path, right)?,
                            None => false,
                        };
                    }
                }
            }
        } else if let Some(ref op) = expr.op {
            let mut temp_map = std::mem::take(&mut self.conforms_map);
            let field_value = self.get_column_expr_value(
                Some(entry),
                file_info,
                root_path,
                &mut temp_map,
                None,
                expr.left.as_ref().unwrap(),
            )?;
            temp_map.clear();
            let value = match op {
                Op::In | Op::NotIn | Op::Exists | Op::NotExists => Variant::empty(VariantType::String),
                _ => {
                    let v = self.get_column_expr_value(
                        Some(entry),
                        file_info,
                        root_path,
                        &mut temp_map,
                        None,
                        expr.right.as_ref().unwrap(),
                    )?;
                    temp_map.clear();
                    v
                }
            };
            self.conforms_map = temp_map;

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
                        Op::Rx => {
                            fn identity(s: &str) -> Result<String, String> { Ok(s.to_string()) }
                            return self.match_pattern(val, &field_str, identity, "Incorrect regex expression: ");
                        }
                        Op::NotRx => {
                            fn identity(s: &str) -> Result<String, String> { Ok(s.to_string()) }
                            return self.match_pattern(val, &field_str, identity, "Incorrect regex expression: ").map(|m| !m);
                        }
                        Op::Like => {
                            return self.match_pattern(val, &field_str, convert_like_to_pattern, "Incorrect LIKE expression: ");
                        }
                        Op::NotLike => {
                            return self.match_pattern(val, &field_str, convert_like_to_pattern, "Incorrect LIKE expression: ").map(|m| !m);
                        }
                        Op::Eeq => val.eq(&field_str),
                        Op::Ene => val.ne(&field_str),
                        Op::In => {
                            let args = self.get_in_args(expr);
                            let mut result = false;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_string().eq(&field_str) {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        }
                        Op::NotIn => {
                            let args = self.get_in_args(expr);
                            let mut result = true;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_string().eq(&field_str) {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
                        Op::Exists => self.check_exists(expr),
                        Op::NotExists => !self.check_exists(expr),
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
                        Op::In => {
                            let field_value = field_value.to_float();
                            let args = self.get_in_args(expr);
                            let mut result = false;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_float() == field_value {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        },
                        Op::NotIn => {
                            let field_value = field_value.to_float();
                            let args = self.get_in_args(expr);
                            let mut result = true;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_float() == field_value {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
                        Op::Exists => self.check_exists(expr),
                        Op::NotExists => !self.check_exists(expr),
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
                        Op::In => {
                            let field_value = field_value.to_float();
                            let args = self.get_in_args(expr);
                            let mut result = false;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_float() == field_value {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        },
                        Op::NotIn => {
                            let field_value = field_value.to_float();
                            let args = self.get_in_args(expr);
                            let mut result = true;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_float() == field_value {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
                        Op::Exists => self.check_exists(expr),
                        Op::NotExists => !self.check_exists(expr),
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
                        Op::In => {
                            let field_value = field_value.to_bool();
                            let args = self.get_in_args(expr);
                            let mut result = false;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_bool() == field_value {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        },
                        Op::NotIn => {
                            let field_value = field_value.to_bool();
                            let args = self.get_in_args(expr);
                            let mut result = true;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_bool() == field_value {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
                        Op::Exists => self.check_exists(expr),
                        Op::NotExists => !self.check_exists(expr),
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
                        Op::In => {
                            let field_value = field_value.to_datetime()?.0.and_utc().timestamp();
                            let args = self.get_in_args(expr);
                            let mut result = false;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_datetime()?.0.and_utc().timestamp() == field_value {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        },
                        Op::NotIn => {
                            let field_value = field_value.to_datetime()?.0.and_utc().timestamp();
                            let args = self.get_in_args(expr);
                            let mut result = true;
                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry), file_info, root_path, &mut HashMap::new(), None, arg,
                            )) {
                                if item?.to_datetime()?.0.and_utc().timestamp() == field_value {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
                        Op::Exists => self.check_exists(expr),
                        Op::NotExists => !self.check_exists(expr),
                        _ => false,
                    }
                }
            };
        }

        Ok(result)
    }

    fn is_zip_archive(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_zip_archive
                .as_ref()
                .unwrap_or(self.default_config.is_zip_archive.as_ref().unwrap()),
        )
    }

    fn is_archive(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_archive
                .as_ref()
                .unwrap_or(self.default_config.is_archive.as_ref().unwrap()),
        )
    }

    fn is_audio(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_audio
                .as_ref()
                .unwrap_or(self.default_config.is_audio.as_ref().unwrap()),
        )
    }

    fn is_book(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_book
                .as_ref()
                .unwrap_or(self.default_config.is_book.as_ref().unwrap()),
        )
    }

    fn is_doc(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_doc
                .as_ref()
                .unwrap_or(self.default_config.is_doc.as_ref().unwrap()),
        )
    }

    fn is_font(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_font
                .as_ref()
                .unwrap_or(self.default_config.is_font.as_ref().unwrap()),
        )
    }

    fn is_image(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_image
                .as_ref()
                .unwrap_or(self.default_config.is_image.as_ref().unwrap()),
        )
    }

    fn is_source(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_source
                .as_ref()
                .unwrap_or(self.default_config.is_source.as_ref().unwrap()),
        )
    }

    fn is_video(&self, file_name: &str) -> bool {
        has_extension(
            file_name,
            self.config
                .is_video
                .as_ref()
                .unwrap_or(self.default_config.is_video.as_ref().unwrap()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Expr;
    use crate::field::Field;
    use crate::fileinfo::FileInfo;
    use crate::function::Function;
    use crate::query::{OutputFormat, Query, Root, RootOptions};

    // Tests for FileMetadataState
    #[test]
    fn test_file_metadata_state_new() {
        let state = FileMetadataState::new();

        assert!(state.file_metadata.is_none());
        assert!(state.line_count.is_none());
        assert!(state.dimensions.is_none());
        assert!(state.duration.is_none());
        assert!(state.mp3_metadata.is_none());
        assert!(state.exif_metadata.is_none());
    }

    #[test]
    fn test_file_metadata_state_clear() {
        let mut state = FileMetadataState::new();

        state.file_metadata = Some(None);
        state.line_count = Some(None);
        state.dimensions = Some(None);
        state.duration = Some(None);
        state.mp3_metadata = Some(None);
        state.exif_metadata = Some(None);

        state.clear();

        assert!(state.file_metadata.is_none());
        assert!(state.line_count.is_none());
        assert!(state.dimensions.is_none());
        assert!(state.duration.is_none());
        assert!(state.mp3_metadata.is_none());
        assert!(state.exif_metadata.is_none());
    }

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
        assert!(searcher_with_ordering.has_ordering());

        let searcher_without_ordering = create_test_searcher();
        assert!(!searcher_without_ordering.has_ordering());
    }

    #[test]
    fn test_has_aggregate_column() {
        let searcher_with_aggregate = create_test_searcher_with_aggregate();
        assert!(searcher_with_aggregate.has_aggregate_column());

        let searcher_without_aggregate = create_test_searcher();
        assert!(!searcher_without_aggregate.has_aggregate_column());
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
    fn test_is_archive() {
        let searcher = create_test_searcher();

        // Test with archive extensions
        assert!(searcher.is_archive("test.zip"));
        assert!(searcher.is_archive("test.tar"));
        assert!(searcher.is_archive("test.gz"));
        assert!(searcher.is_archive("test.rar"));

        // Test with non-archive extensions
        assert!(!searcher.is_archive("test.txt"));
        assert!(!searcher.is_archive("test.jpg"));
        assert!(!searcher.is_archive("test"));
    }

    #[test]
    fn test_is_audio() {
        let searcher = create_test_searcher();

        // Test with audio extensions
        assert!(searcher.is_audio("test.mp3"));
        assert!(searcher.is_audio("test.wav"));
        assert!(searcher.is_audio("test.flac"));
        assert!(searcher.is_audio("test.ogg"));

        // Test with non-audio extensions
        assert!(!searcher.is_audio("test.txt"));
        assert!(!searcher.is_audio("test.jpg"));
        assert!(!searcher.is_audio("test"));
    }

    #[test]
    fn test_is_book() {
        let searcher = create_test_searcher();

        // Test with book extensions
        assert!(searcher.is_book("test.pdf"));
        assert!(searcher.is_book("test.epub"));
        assert!(searcher.is_book("test.mobi"));
        assert!(searcher.is_book("test.djvu"));

        // Test with non-book extensions
        assert!(!searcher.is_book("test.txt"));
        assert!(!searcher.is_book("test.jpg"));
        assert!(!searcher.is_book("test"));
    }

    #[test]
    fn test_is_doc() {
        let searcher = create_test_searcher();

        // Test with document extensions
        assert!(searcher.is_doc("test.doc"));
        assert!(searcher.is_doc("test.docx"));
        assert!(searcher.is_doc("test.pdf"));
        assert!(searcher.is_doc("test.xls"));

        // Test with non-document extensions
        assert!(!searcher.is_doc("test.txt"));
        assert!(!searcher.is_doc("test.jpg"));
        assert!(!searcher.is_doc("test"));
    }

    #[test]
    fn test_is_font() {
        let searcher = create_test_searcher();

        // Test with font extensions
        assert!(searcher.is_font("test.ttf"));
        assert!(searcher.is_font("test.otf"));
        assert!(searcher.is_font("test.woff"));
        assert!(searcher.is_font("test.woff2"));

        // Test with non-font extensions
        assert!(!searcher.is_font("test.txt"));
        assert!(!searcher.is_font("test.jpg"));
        assert!(!searcher.is_font("test"));
    }

    #[test]
    fn test_is_image() {
        let searcher = create_test_searcher();

        // Test with image extensions
        assert!(searcher.is_image("test.jpg"));
        assert!(searcher.is_image("test.png"));
        assert!(searcher.is_image("test.gif"));
        assert!(searcher.is_image("test.svg"));

        // Test with non-image extensions
        assert!(!searcher.is_image("test.txt"));
        assert!(!searcher.is_image("test.mp3"));
        assert!(!searcher.is_image("test"));
    }

    #[test]
    fn test_is_source() {
        let searcher = create_test_searcher();

        // Test with source code extensions
        assert!(searcher.is_source("test.rs"));
        assert!(searcher.is_source("test.c"));
        assert!(searcher.is_source("test.cpp"));
        assert!(searcher.is_source("test.java"));

        // Test with non-source extensions
        assert!(!searcher.is_source("test.txt"));
        assert!(!searcher.is_source("test.jpg"));
        assert!(!searcher.is_source("test"));
    }

    #[test]
    fn test_is_video() {
        let searcher = create_test_searcher();

        // Test with video extensions
        assert!(searcher.is_video("test.mp4"));
        assert!(searcher.is_video("test.avi"));
        assert!(searcher.is_video("test.mkv"));
        assert!(searcher.is_video("test.mov"));

        // Test with non-video extensions
        assert!(!searcher.is_video("test.txt"));
        assert!(!searcher.is_video("test.jpg"));
        assert!(!searcher.is_video("test"));
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

        let _ = searcher.visit_dir(
            &root,
            0, 0, 0,
            false, false,
            #[cfg(feature = "git")]
            None,
            false, false,
            TraversalMode::Dfs,
            true,
            &root,
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

        let _ = searcher.visit_dir(
            &root,
            0, 0, 0,
            false, false,
            #[cfg(feature = "git")]
            None,
            false, false,
            TraversalMode::Bfs,
            true,
            &root,
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

        let _ = searcher.visit_dir(
            &tmp,
            0, 0, 0,
            false, false,
            #[cfg(feature = "git")]
            None,
            false, false,
            TraversalMode::Dfs,
            true,
            &tmp,
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

        let _ = searcher.visit_dir(
            &tmp,
            0, 0, 0,
            false, false,
            #[cfg(feature = "git")]
            None,
            false, false,
            TraversalMode::Dfs,
            true,
            &tmp,
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
    fn test_is_file_false_for_backslash_terminated_archive_entry() {
        let tmp = std::env::temp_dir().join("fselect_test_isfile_backslash");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("dummy.txt"), "").unwrap();

        let entry = fs::read_dir(&tmp).unwrap().next().unwrap().unwrap();

        let file_info = Some(FileInfo {
            name: String::from("somedir\\"),
            size: 0,
            mode: None,
            modified: None,
        });

        let mut searcher = create_test_searcher();
        let result = searcher.get_field_value(&entry, &file_info, &tmp, &Field::IsFile).unwrap();
        let _ = fs::remove_dir_all(&tmp);

        // A directory entry (name ends with \) should NOT be reported as a file
        assert_eq!(result.to_string(), "false");
    }

    #[test]
    fn test_is_dir_and_is_file_consistent_for_backslash_archive_entry() {
        let tmp = std::env::temp_dir().join("fselect_test_consistency_backslash");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("dummy.txt"), "").unwrap();

        let entry = fs::read_dir(&tmp).unwrap().next().unwrap().unwrap();

        let file_info = Some(FileInfo {
            name: String::from("somedir\\"),
            size: 0,
            mode: None,
            modified: None,
        });

        let mut searcher = create_test_searcher();
        let is_dir = searcher.get_field_value(&entry, &file_info, &tmp, &Field::IsDir).unwrap();
        let is_file = searcher.get_field_value(&entry, &file_info, &tmp, &Field::IsFile).unwrap();
        let _ = fs::remove_dir_all(&tmp);

        // is_dir and is_file should be mutually exclusive for directories
        assert_eq!(is_dir.to_string(), "true");
        assert_eq!(is_file.to_string(), "false");
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

    #[test]
    fn test_is_symlink_false_when_following_symlinks() {
        let tmp = std::env::temp_dir().join("fselect_test_symlink_follow_islink");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("real_file.txt"), "hello world").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("real_file.txt", tmp.join("link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(tmp.join("real_file.txt"), tmp.join("link")).unwrap();

        let entry = fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == "link")
            .unwrap();

        let mut searcher = create_test_searcher();
        searcher.current_follow_symlinks = true;

        let result = searcher
            .get_field_value(&entry, &None, &tmp, &Field::IsSymlink)
            .unwrap();
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(
            result.to_string(),
            "false",
            "is_symlink should be false when following symlinks"
        );
    }

    #[test]
    fn test_size_follows_symlink_when_requested() {
        let tmp = std::env::temp_dir().join("fselect_test_symlink_follow_size");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let content = "hello world, this is a reasonably long test string for size comparison";
        fs::write(tmp.join("real_file.txt"), content).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("real_file.txt", tmp.join("link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(tmp.join("real_file.txt"), tmp.join("link")).unwrap();

        let entry = fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == "link")
            .unwrap();

        let mut searcher = create_test_searcher();
        searcher.current_follow_symlinks = true;

        let result = searcher
            .get_field_value(&entry, &None, &tmp, &Field::Size)
            .unwrap();
        let _ = fs::remove_dir_all(&tmp);

        let size = result.to_int();
        let expected_size = content.len() as i64;

        assert_eq!(
            size, expected_size,
            "size should be target file's size ({}) when following symlinks, got {}",
            expected_size, size
        );
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

        let _ = searcher.visit_dir(
            &root_a,
            0, 0, 0,
            false, false,
            #[cfg(feature = "git")]
            None,
            false, false,
            TraversalMode::Dfs,
            true,
            &root_a,
        );

        let found_after_a = searcher.found;

        searcher.visited_dirs.clear();

        let _ = searcher.visit_dir(
            &root_b,
            0, 0, 0,
            false, false,
            #[cfg(feature = "git")]
            None,
            false, false,
            TraversalMode::Dfs,
            true,
            &root_b,
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
