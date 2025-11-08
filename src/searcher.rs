//! Handles directory traversal and file processing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
#[cfg(unix)]
use std::fs::symlink_metadata;
use std::fs::{DirEntry, FileType, Metadata};
use std::io;
use std::io::{ErrorKind, Write};
use std::ops::Add;
#[cfg(unix)]
use std::os::unix::fs::{DirEntryExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;

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

struct FileMetadataState {
    file_metadata_set: bool,
    file_metadata: Option<Metadata>,

    line_count_set: bool,
    line_count: Option<usize>,

    dimensions_set: bool,
    dimensions: Option<Dimensions>,

    duration_set: bool,
    duration: Option<Duration>,

    mp3_metadata_set: bool,
    mp3_metadata: Option<MP3Metadata>,

    exif_metadata_set: bool,
    exif_metadata: Option<HashMap<String, String>>,
}

impl FileMetadataState {
    fn new() -> FileMetadataState {
        FileMetadataState {
            file_metadata_set: false,
            file_metadata: None,

            line_count_set: false,
            line_count: None,

            dimensions_set: false,
            dimensions: None,

            duration_set: false,
            duration: None,

            mp3_metadata_set: false,
            mp3_metadata: None,

            exif_metadata_set: false,
            exif_metadata: None,
        }
    }

    fn clear(&mut self) {
        self.file_metadata_set = false;
        self.file_metadata = None;

        self.line_count_set = false;
        self.line_count = None;

        self.dimensions_set = false;
        self.dimensions = None;

        self.duration_set = false;
        self.duration = None;

        self.mp3_metadata_set = false;
        self.mp3_metadata = None;

        self.exif_metadata_set = false;
        self.exif_metadata = None;
    }

    fn update_file_metadata(&mut self, entry: &DirEntry, follow_symlinks: bool) {
        if !self.file_metadata_set {
            self.file_metadata_set = true;
            self.file_metadata = get_metadata(entry, follow_symlinks);
        }
    }

    fn update_line_count(&mut self, entry: &DirEntry) {
        if !self.line_count_set {
            self.line_count_set = true;
            self.line_count = get_line_count(entry);
        }
    }

    fn update_mp3_metadata(&mut self, entry: &DirEntry) {
        if !self.mp3_metadata_set {
            self.mp3_metadata_set = true;
            self.mp3_metadata = get_mp3_metadata(entry);
        }
    }

    fn update_exif_metadata(&mut self, entry: &DirEntry) {
        if !self.exif_metadata_set {
            self.exif_metadata_set = true;
            self.exif_metadata = get_exif_metadata(entry);
        }
    }

    fn update_dimensions(&mut self, entry: &DirEntry) {
        if !self.dimensions_set {
            self.dimensions_set = true;
            self.dimensions = get_dimensions(entry.path());
        }
    }

    fn update_duration(&mut self, entry: &DirEntry) {
        if !self.duration_set {
            self.update_mp3_metadata(entry);

            self.duration_set = true;
            self.duration = get_duration(entry.path(), &self.mp3_metadata);
        }
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
    raw_output_buffer: Vec<HashMap<String, String>>,
    partitioned_output_buffer: Rc<HashMap<Vec<String>, Vec<HashMap<String, String>>>>,
    output_buffer: TopN<Criteria<String>, String>,

    record_context: Rc<RefCell<HashMap<String, HashMap<String, String>>>>,
    current_alias: Option<String>,

    hgignore_filters: Vec<HgignoreFilter>,
    dockerignore_filters: Vec<DockerignoreFilter>,
    visited_dirs: HashSet<PathBuf>,
    #[cfg(unix)]
    visited_inodes: HashSet<u64>,
    lscolors: LsColors,
    dir_queue: Box<VecDeque<PathBuf>>,
    current_follow_symlinks: bool,

    fms: FileMetadataState,
    subquery_cache: HashMap<String, Vec<String>>,
    silent_mode: bool,

    pub error_count: i32,
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
            raw_output_buffer: vec![],
            partitioned_output_buffer: Rc::new(HashMap::new()),
            output_buffer: if limit == 0 {
                TopN::limitless()
            } else {
                TopN::new(limit)
            },

            record_context,
            current_alias: None,

            hgignore_filters: vec![],
            dockerignore_filters: vec![],
            visited_dirs: HashSet::new(),
            #[cfg(unix)]
            visited_inodes: HashSet::new(),
            lscolors: LsColors::from_env().unwrap_or_default(),
            dir_queue: Box::from(VecDeque::new()),
            current_follow_symlinks: false,

            fms: FileMetadataState::new(),
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
    pub fn list_search_results(&mut self) -> io::Result<()> {
        let current_dir = std::env::current_dir()?;

        if !self.silent_mode {
            if let Err(e) = self.results_writer.write_header(&mut std::io::stdout()) {
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

            // Apply filters
            if apply_hgignore {
                search_upstream_hgignore(&mut self.hgignore_filters, root_dir);
            }

            if apply_dockerignore {
                search_upstream_dockerignore(&mut self.dockerignore_filters, root_dir);
            }

            self.dir_queue.clear();

            #[cfg(unix)]
            let hardlinks = root.options.hardlinks;
            
            #[cfg(unix)]
            {
                if hardlinks {
                    let metadata = match self.current_follow_symlinks {
                        true => root_dir.metadata(),
                        false => symlink_metadata(root_dir),
                    };
                    if let Ok(metadata) = metadata {
                        self.visited_inodes.insert(metadata.ino());
                    }
                }                
            }

            let _result = self.visit_dir(
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
                #[cfg(unix)]
                hardlinks,
                root_dir,
            );
        }

        let compute_time = std::time::Instant::now();

        // ======== Compute results =========
        if self.has_aggregate_column() {
            if !self.query.grouping_fields.is_empty() {
                if self.partitioned_output_buffer.is_empty() {
                    self.partitioned_output_buffer = Rc::new(self.partition_output_buffer());
                }

                let group_keys: Vec<String> = self
                    .query
                    .grouping_fields
                    .iter()
                    .map(|f| f.to_string())
                    .collect();
                let buffer_partitions = self.partitioned_output_buffer.clone();
                let buffer_partitions = buffer_partitions.iter().collect::<Vec<_>>();                 

                let mut results = vec![];

                buffer_partitions.iter().for_each(|f| {
                    let mut items: Vec<(String, String)> = Vec::new();

                    let mut file_map = HashMap::new();
                    for (i, k) in group_keys.iter().enumerate() {
                        file_map.insert(k.clone(), f.0.get(i).unwrap().clone());
                    }

                    for column_expr in &self.query.fields {
                        let record = format!(
                            "{}",
                            self.get_column_expr_value(
                                None,
                                &None,
                                &Path::new(""),
                                &mut file_map,
                                Some(f.1),
                                column_expr
                            )
                        );
                        let field_name = column_expr.to_string().to_lowercase();
                        items.push((field_name, record));
                    }

                    results.push(items);
                });

                if !self.query.ordering_fields.is_empty() {
                    let ordering_fields = self
                        .query
                        .ordering_fields
                        .iter()
                        .map(|f| f.to_string().to_lowercase())
                        .collect::<Vec<String>>();
                    let directions = self.query.ordering_asc.clone();
                    let sorting_indices = ordering_fields
                        .iter()
                        .map(|f| {
                            self.query
                                .fields
                                .iter()
                                .map(|f| f.to_string().to_lowercase())
                                .position(|g| &g == f)
                                .unwrap_or(0)
                        })
                        .collect::<Vec<usize>>();

                    results.sort_by(|a, b| {
                        sorting_indices
                            .iter()
                            .enumerate()
                            .map(|(idx, i)| {
                                if let Some(a) = a.get(*i) {
                                    if let Ok(a) = a.1.parse::<i64>() {
                                        if let Some(b) = b.get(*i) {
                                            if let Ok(b) = b.1.parse::<i64>() {
                                                return if directions[idx] { 
                                                    a.cmp(&b) 
                                                } else { 
                                                    b.cmp(&a) 
                                                };
                                            }
                                        }
                                    }
                                }
                                if directions[idx] { 
                                    a.get(*i).unwrap().1.cmp(&b.get(*i).unwrap().1) 
                                } else { 
                                    b.get(*i).unwrap().1.cmp(&a.get(*i).unwrap().1) 
                                } 
                            })
                            .find(|r| *r != std::cmp::Ordering::Equal)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }

                if !self.silent_mode {
                    results.iter().for_each(|items| {
                        let mut buf = WritableBuffer::new();
                        let _ = self.results_writer.write_row(&mut buf, items.to_owned());
                        let _ = write!(std::io::stdout(), "{}", String::from(buf));
                    });
                }
            } else {
                let mut buf = WritableBuffer::new();
                let mut items: Vec<(String, String)> = Vec::new();

                for column_expr in &self.query.fields {
                    let record = format!(
                        "{}",
                        self.get_column_expr_value(
                            None,
                            &None,
                            &Path::new(""),
                            &mut HashMap::new(),
                            None,
                            column_expr
                        )
                    );
                    let field_name = column_expr.to_string().to_lowercase();
                    items.push((field_name, record));
                }

                if !self.silent_mode {
                    self.results_writer.write_row(&mut buf, items)?;

                    if let Err(e) = write!(std::io::stdout(), "{}", String::from(buf)) {
                        if e.kind() == ErrorKind::BrokenPipe {
                            return Ok(());
                        }
                    }
                }
            }
        } else if self.is_buffered() && !self.silent_mode {
            let mut first = true;
            for piece in self.output_buffer.values() {
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
        if let Some(cached) = self.subquery_cache.get(&query_str) {
            return cached.clone();
        }

        let mut sub_searcher = Searcher::new_with_context(
            &query,
            self.record_context.clone(),
            self.config,
            self.default_config,
            self.use_colors
        );
        sub_searcher.silent_mode = true;
        sub_searcher.list_search_results().unwrap_or_default();

        let result_values = sub_searcher.output_buffer.values().iter()
            .map(|s| s.trim_end().to_string())
            .collect::<Vec<String>>();

        self.subquery_cache.insert(query_str, result_values.clone());

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
        #[cfg(unix)]
        hardlinks: bool,
        root_dir: &Path,
    ) -> io::Result<()> {
        // Prevents infinite loops when following symlinks
        if self.current_follow_symlinks {
            if self.visited_dirs.contains(&dir.to_path_buf()) {
                return Ok(());
            } else {
                self.visited_dirs.insert(dir.to_path_buf());
            }
        }

        // Canonicalize the path to resolve symlinks and relative paths
        let canonical_path = crate::util::canonical_path(&dir.to_path_buf());
        if canonical_path.is_err() {
            self.error_count += 1;
            error_message(
                &dir.to_string_lossy(),
                String::from("could not canonicalize path: ")
                    .add(canonical_path.err().unwrap().as_str())
                    .as_str(),
            );
            return Ok(());
        }

        let canonical_path = canonical_path.unwrap();
        let canonical_depth = crate::util::calc_depth(&canonical_path);

        let base_depth = match root_depth {
            0 => canonical_depth,
            _ => root_depth,
        };

        let depth = canonical_depth - base_depth + 1;

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

                                if apply_gitignore || apply_hgignore || apply_dockerignore {
                                    if let Ok(canonicalized) = crate::util::canonical_path(&path) {
                                        canonical_path = PathBuf::from(canonicalized);
                                    }
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
                                    let checked = self.check_file(&entry, root_dir, &None)?;
                                    if !checked {
                                        return Ok(());
                                    }

                                    if search_archives
                                        && self.is_zip_archive(&path.to_string_lossy())
                                    {
                                        if let Ok(file) = fs::File::open(&path) {
                                            if let Ok(mut archive) = zip::ZipArchive::new(file) {
                                                for i in 0..archive.len() {
                                                    if self.query.limit > 0
                                                        && self.query.limit <= self.found
                                                    {
                                                        break;
                                                    }

                                                    if let Ok(afile) = archive.by_index(i) {
                                                        let file_info = to_file_info(&afile);
                                                        let checked = self
                                                            .check_file(&entry, root_dir, &Some(file_info))?;
                                                        if !checked {
                                                            return Ok(());
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

                                        if file_type.is_symlink() {
                                            if let Ok(resolved) = std::fs::read_link(&path) {
                                                ok = true;
                                                path = resolved;
                                            }
                                        } else if file_type.is_dir() {
                                            ok = true;
                                        }

                                        if ok && self.ok_to_visit_dir(&entry, file_type, #[cfg(unix)] hardlinks) {
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
                                                    #[cfg(unix)]
                                                    hardlinks,
                                                    root_dir,
                                                );

                                                if result.is_err() {
                                                    self.error_count += 1;
                                                    path_error_message(
                                                        &path,
                                                        result.err().unwrap(),
                                                    );
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
                    #[cfg(unix)]
                    hardlinks,
                    root_dir,
                );

                if result.is_err() {
                    self.error_count += 1;
                    path_error_message(&path, result.err().unwrap());
                }
            }
        }

        Ok(())
    }

    #[cfg(unix)]
    fn ok_to_visit_dir(&mut self, entry: &DirEntry, file_type: FileType, hardlinks: bool) -> bool {
        if hardlinks {
            let ino = entry.ino();
            if self.visited_inodes.contains(&ino) {
                return false;
            } else {
                self.visited_inodes.insert(ino);
            }
        }

        match self.current_follow_symlinks {
            true => true,
            false => !file_type.is_symlink(),
        }
    }

    #[cfg(not(unix))]
    fn ok_to_visit_dir(&mut self, _: &DirEntry, file_type: FileType) -> bool {
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
        buffer_data: Option<&Vec<HashMap<String, String>>>,
        column_expr: &Expr,
    ) -> Variant {
        let column_expr_str = column_expr.to_string();

        if column_expr_str.contains(".") {
            let parts: Vec<&str> = column_expr_str.split('.').collect();
            if parts.len() == 2 {
                let column_expr_context_name = parts[0];
                if let Some(ref current_alias) = self.current_alias {
                    if column_expr_context_name != current_alias {
                        let context = self.record_context.borrow();
                        if let Some(ctx) = context.get(column_expr_context_name) {
                            if let Some(val) = ctx.get(parts[1]) {
                                return Variant::from_string(val);
                            } else {
                                //TODO: this should be propagated up to the higher context
                                return Variant::empty(VariantType::String)
                            }
                        } else {
                            //this is a syntax error actually
                            error_exit("Invalid root alias", column_expr_context_name);
                        }
                    }
                }
            }
        }

        if file_map.contains_key(&column_expr_str) {
            return Variant::from_string(&file_map[&column_expr_str]);
        }

        if let Some(ref _function) = column_expr.function {
            let result =
                self.get_function_value(entry, file_info, root_path, file_map, buffer_data, column_expr);
            file_map.insert(column_expr_str, result.to_string());
            return result;
        }

        if let Some(ref field) = column_expr.field {
            if entry.is_some() {
                let result = self.get_field_value(entry.unwrap(), file_info, root_path, field);
                file_map.insert(column_expr_str, result.to_string());
                let mut context = self.record_context.borrow_mut();
                let context_key = self.current_alias.clone().unwrap_or_else(|| String::from(""));
                let context_entry = context.entry(context_key).or_insert(HashMap::new());
                context_entry.insert(field.to_string(), result.to_string());
                return result;
            } else if let Some(val) = file_map.get(&field.to_string()) {
                return Variant::from_string(val);
            } else {
                return Variant::empty(VariantType::String);
            }
        }

        if let Some(ref value) = column_expr.val {
            return Variant::from_signed_string(&value, column_expr.minus);
        }

        let result;

        if let Some(ref left) = column_expr.left {
            let left_result =
                self.get_column_expr_value(entry, file_info, root_path, file_map, buffer_data, left);

            if let Some(ref op) = column_expr.arithmetic_op {
                if let Some(ref right) = column_expr.right {
                    let right_result =
                        self.get_column_expr_value(entry, file_info, root_path, file_map, buffer_data, right);
                    result = op.calc(&left_result, &right_result);
                    file_map.insert(column_expr_str, result.to_string());
                } else {
                    result = left_result;
                }
            } else {
                result = left_result;
            }
        } else {
            result = Variant::empty(VariantType::Int);
        }

        result
    }

    fn get_function_value(
        &mut self,
        entry: Option<&DirEntry>,
        file_info: &Option<FileInfo>,
        root_path: &Path,
        file_map: &mut HashMap<String, String>,
        buffer_data: Option<&Vec<HashMap<String, String>>>,
        column_expr: &Expr,
    ) -> Variant {
        let dummy = Expr::value(String::from(""));
        let boxed_dummy = &Box::from(dummy);

        let left_expr = match &column_expr.left {
            Some(left_expr) => left_expr,
            _ => boxed_dummy,
        };

        let function = &column_expr.function.as_ref().unwrap();

        if function.is_aggregate_function() {
            let _ = self.get_column_expr_value(entry, file_info, root_path, file_map, buffer_data, left_expr);
            let buffer_key = left_expr.to_string();
            let aggr_result = function::get_aggregate_value(
                &column_expr.function,
                buffer_data.unwrap_or(&self.raw_output_buffer),
                buffer_key,
                &column_expr.val,
            );
            Variant::from_string(&aggr_result)
        } else {
            let function_arg =
                self.get_column_expr_value(entry, file_info, root_path, file_map, buffer_data, left_expr);
            let mut function_args = vec![];
            if let Some(args) = &column_expr.args {
                for arg in args {
                    let arg_value =
                        self.get_column_expr_value(entry, file_info, root_path, file_map, buffer_data, arg);
                    function_args.push(arg_value.to_string());
                }
            }
            let result = function::get_value(
                &column_expr.function,
                function_arg.to_string(),
                function_args,
                entry,
                file_info,
            );
            file_map.insert(column_expr.to_string(), result.to_string());

            result
        }
    }

    fn partition_output_buffer(&self) -> HashMap<Vec<String>, Vec<HashMap<String, String>>> {
        let group_fields: Vec<String> = self
            .query
            .grouping_fields
            .iter()
            .map(|ref expr| expr.to_string())
            .collect();
        let mut result: HashMap<Vec<String>, Vec<HashMap<String, String>>> = HashMap::new();

        self.raw_output_buffer.iter().for_each(|item| {
            let key: Vec<String> = group_fields
                .iter()
                .map(|f| item.get(f).unwrap_or(&String::new()).clone())
                .collect();
            if result.contains_key(&key) {
                result.get_mut(&key).unwrap().push(item.clone());
            } else {
                result.insert(key, vec![item.clone()]);
            }
        });

        result
    }

    fn get_field_value(
        &mut self,
        entry: &DirEntry,
        file_info: &Option<FileInfo>,
        root_path: &Path,
        field: &Field,
    ) -> Variant {
        if file_info.is_some() && !field.is_available_for_archived_files() {
            return Variant::empty(VariantType::String);
        }

        match field {
            Field::Name => match file_info {
                Some(file_info) => {
                    return Variant::from_string(&format!(
                        "[{}] {}",
                        entry.file_name().to_string_lossy(),
                        file_info.name
                    ));
                }
                _ => {
                    return Variant::from_string(&format!(
                        "{}",
                        entry.file_name().to_string_lossy()
                    ));
                }
            },
            Field::Extension => match file_info {
                Some(file_info) => {
                    return Variant::from_string(&format!(
                        "[{}] {}",
                        entry.file_name().to_string_lossy(),
                        crate::util::get_extension(&file_info.name)
                    ));
                }
                _ => {
                    return Variant::from_string(
                        &crate::util::get_extension(&entry.file_name().to_string_lossy())
                            .to_string(),
                    );
                }
            },
            Field::Path => return match file_info {
                Some(file_info) => {
                    Variant::from_string(&format!(
                        "[{}] {}",
                        entry.path().to_string_lossy(),
                        file_info.name
                    ))
                }
                _ => {
                    match entry.path().strip_prefix(root_path) {
                        Ok(stripped_path) => {
                            Variant::from_string(&format!(
                                "{}",
                                stripped_path.to_string_lossy()
                            ))
                        }
                        Err(_) => {
                            Variant::from_string(&format!("{}", entry.path().to_string_lossy()))
                        }
                    }
                }
            },
            Field::AbsPath => match file_info {
                Some(file_info) => {
                    return Variant::from_string(&format!(
                        "[{}] {}",
                        entry.path().to_string_lossy(),
                        file_info.name
                    ));
                }
                _ => {
                    if let Ok(path) = crate::util::canonical_path(&entry.path()) {
                        return Variant::from_string(&path);
                    }
                }
            },
            Field::Directory => {
                let file_path = match file_info {
                    Some(file_info) => file_info.name.clone(),
                    _ => entry.path().to_string_lossy().to_string(),
                };
                let pb = PathBuf::from(file_path);
                if let Some(parent) = pb.parent() {
                    return Variant::from_string(&parent.to_string_lossy().to_string());
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
                        return Variant::from_string(&parent.to_string_lossy().to_string());
                    }

                    if let Ok(path) = crate::util::canonical_path(&parent.to_path_buf()) {
                        return Variant::from_string(&path);
                    }
                }
            }
            Field::Size => match file_info {
                Some(file_info) => {
                    return Variant::from_int(file_info.size as i64);
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_int(attrs.len() as i64);
                    }
                }
            },
            Field::FormattedSize => match file_info {
                Some(file_info) => {
                    return Variant::from_string(&format_filesize(
                        file_info.size,
                        self.config
                            .default_file_size_format
                            .as_ref()
                            .unwrap_or(&String::new()),
                    ));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_string(&format_filesize(
                            attrs.len(),
                            self.config
                                .default_file_size_format
                                .as_ref()
                                .unwrap_or(&String::new()),
                        ));
                    }
                }
            },
            Field::IsDir => match file_info {
                Some(file_info) => {
                    return Variant::from_bool(
                        file_info.name.ends_with('/') || file_info.name.ends_with('\\'),
                    );
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_bool(attrs.is_dir());
                    }
                }
            },
            Field::IsFile => match file_info {
                Some(file_info) => {
                    return Variant::from_bool(!file_info.name.ends_with('/'));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_bool(attrs.is_file());
                    }
                }
            },
            Field::IsSymlink => match file_info {
                Some(_) => {
                    return Variant::from_bool(false);
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_bool(attrs.file_type().is_symlink());
                    }
                }
            },
            Field::IsPipe => {
                return self.check_file_mode(entry, &mode::is_pipe, file_info, &mode::mode_is_pipe);
            }
            Field::IsCharacterDevice => {
                return self.check_file_mode(
                    entry,
                    &mode::is_char_device,
                    file_info,
                    &mode::mode_is_char_device,
                );
            }
            Field::IsBlockDevice => {
                return self.check_file_mode(
                    entry,
                    &mode::is_block_device,
                    file_info,
                    &mode::mode_is_block_device,
                );
            }
            Field::IsSocket => {
                return self.check_file_mode(
                    entry,
                    &mode::is_socket,
                    file_info,
                    &mode::mode_is_socket,
                );
            }
            Field::Device => {
                #[cfg(unix)]
                {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_int(attrs.dev() as i64);
                    }
                }

                return Variant::empty(VariantType::String);
            }
            Field::Inode => {
                #[cfg(unix)]
                {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_int(attrs.ino() as i64);
                    }
                }

                return Variant::empty(VariantType::String);
            }
            Field::Blocks => {
                #[cfg(unix)]
                {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_int(attrs.blocks() as i64);
                    }
                }

                return Variant::empty(VariantType::String);
            }
            Field::Hardlinks => {
                #[cfg(unix)]
                {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_int(attrs.nlink() as i64);
                    }
                }

                return Variant::empty(VariantType::String);
            }
            Field::Mode => match file_info {
                Some(file_info) => {
                    if let Some(mode) = file_info.mode {
                        return Variant::from_string(&mode::format_mode(mode));
                    }
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return Variant::from_string(&mode::get_mode(attrs));
                    }
                }
            },
            Field::UserRead => {
                return self.check_file_mode(
                    entry,
                    &mode::user_read,
                    file_info,
                    &mode::mode_user_read,
                );
            }
            Field::UserWrite => {
                return self.check_file_mode(
                    entry,
                    &mode::user_write,
                    file_info,
                    &mode::mode_user_write,
                );
            }
            Field::UserExec => {
                return self.check_file_mode(
                    entry,
                    &mode::user_exec,
                    file_info,
                    &mode::mode_user_exec,
                );
            }
            Field::UserAll => {
                return self.check_file_mode(
                    entry,
                    &mode::user_all,
                    file_info,
                    &mode::mode_user_all,
                );
            }
            Field::GroupRead => {
                return self.check_file_mode(
                    entry,
                    &mode::group_read,
                    file_info,
                    &mode::mode_group_read,
                );
            }
            Field::GroupWrite => {
                return self.check_file_mode(
                    entry,
                    &mode::group_write,
                    file_info,
                    &mode::mode_group_write,
                );
            }
            Field::GroupExec => {
                return self.check_file_mode(
                    entry,
                    &mode::group_exec,
                    file_info,
                    &mode::mode_group_exec,
                );
            }
            Field::GroupAll => {
                return self.check_file_mode(
                    entry,
                    &mode::group_all,
                    file_info,
                    &mode::mode_group_all,
                );
            }
            Field::OtherRead => {
                return self.check_file_mode(
                    entry,
                    &mode::other_read,
                    file_info,
                    &mode::mode_other_read,
                );
            }
            Field::OtherWrite => {
                return self.check_file_mode(
                    entry,
                    &mode::other_write,
                    file_info,
                    &mode::mode_other_write,
                );
            }
            Field::OtherExec => {
                return self.check_file_mode(
                    entry,
                    &mode::other_exec,
                    file_info,
                    &mode::mode_other_exec,
                );
            }
            Field::OtherAll => {
                return self.check_file_mode(
                    entry,
                    &mode::other_all,
                    file_info,
                    &mode::mode_other_all,
                );
            }
            Field::Suid => {
                return self.check_file_mode(
                    entry,
                    &mode::suid_bit_set,
                    file_info,
                    &mode::mode_suid,
                );
            }
            Field::Sgid => {
                return self.check_file_mode(
                    entry,
                    &mode::sgid_bit_set,
                    file_info,
                    &mode::mode_sgid,
                );
            }
            Field::IsHidden => match file_info {
                Some(file_info) => {
                    return Variant::from_bool(is_hidden(&file_info.name, &None, true));
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    return Variant::from_bool(is_hidden(
                        &entry.file_name().to_string_lossy(),
                        &self.fms.file_metadata,
                        false,
                    ));
                }
            },
            Field::Uid => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(ref attrs) = self.fms.file_metadata {
                    if let Some(uid) = mode::get_uid(attrs) {
                        return Variant::from_int(uid as i64);
                    }
                }
            }
            Field::Gid => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(ref attrs) = self.fms.file_metadata {
                    if let Some(gid) = mode::get_gid(attrs) {
                        return Variant::from_int(gid as i64);
                    }
                }
            }
            #[cfg(all(unix, feature = "users"))]
            Field::User => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(ref attrs) = self.fms.file_metadata {
                    if let Some(uid) = mode::get_uid(attrs) {
                        if let Some(user) = self.user_cache.get_user_by_uid(uid) {
                            return Variant::from_string(
                                &user.name().to_string_lossy().to_string(),
                            );
                        }
                    }
                }
            }
            #[cfg(all(unix, feature = "users"))]
            Field::Group => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(ref attrs) = self.fms.file_metadata {
                    if let Some(gid) = mode::get_gid(attrs) {
                        if let Some(group) = self.user_cache.get_group_by_gid(gid) {
                            return Variant::from_string(
                                &group.name().to_string_lossy().to_string(),
                            );
                        }
                    }
                }
            }
            Field::Created => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(ref attrs) = self.fms.file_metadata {
                    if let Ok(sdt) = attrs.created() {
                        let dt: DateTime<Local> = DateTime::from(sdt);
                        return Variant::from_datetime(dt.naive_local());
                    }
                }
            }
            Field::Accessed => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(ref attrs) = self.fms.file_metadata {
                    if let Ok(sdt) = attrs.accessed() {
                        let dt: DateTime<Local> = DateTime::from(sdt);
                        return Variant::from_datetime(dt.naive_local());
                    }
                }
            }
            Field::Modified => match file_info {
                Some(file_info) => {
                    if let Some(file_info_modified) = &file_info.modified {
                        let dt = to_local_datetime(file_info_modified);
                        return Variant::from_datetime(dt);
                    }
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        if let Ok(sdt) = attrs.modified() {
                            let dt: DateTime<Local> = DateTime::from(sdt);
                            return Variant::from_datetime(dt.naive_local());
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
                            return Variant::from_bool(has_xattrs);
                        }
                    }
                }

                #[cfg(not(unix))]
                {
                    return Variant::from_bool(false);
                }
            }
            Field::Capabilities => {
                #[cfg(target_os = "linux")]
                {
                    if let Ok(file) = fs::File::open(entry.path()) {
                        if let Ok(Some(caps_xattr)) = file.get_xattr("security.capability") {
                            let caps_string =
                                crate::util::capabilities::parse_capabilities(caps_xattr);
                            return Variant::from_string(&caps_string);
                        }
                    }
                }

                return Variant::empty(VariantType::String);
            }
            Field::IsShebang => {
                return Variant::from_bool(is_shebang(&entry.path()));
            }
            Field::IsEmpty => match file_info {
                Some(file_info) => {
                    return Variant::from_bool(file_info.size == 0);
                }
                _ => {
                    self.fms
                        .update_file_metadata(entry, self.current_follow_symlinks);

                    if let Some(ref attrs) = self.fms.file_metadata {
                        return match attrs.is_dir() {
                            true => match is_dir_empty(entry) {
                                Some(result) => Variant::from_bool(result),
                                None => Variant::empty(VariantType::Bool),
                            },
                            false => Variant::from_bool(attrs.len() == 0),
                        };
                    }
                }
            },
            Field::Width => {
                self.fms.update_dimensions(entry);

                if let Some(Dimensions { width, .. }) = self.fms.dimensions {
                    return Variant::from_int(width as i64);
                }
            }
            Field::Height => {
                self.fms.update_dimensions(entry);

                if let Some(Dimensions { height, .. }) = self.fms.dimensions {
                    return Variant::from_int(height as i64);
                }
            }
            Field::Duration => {
                self.fms.update_duration(entry);

                if let Some(Duration { length, .. }) = self.fms.duration {
                    return Variant::from_int(length as i64);
                }
            }
            Field::Bitrate => {
                self.fms.update_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.fms.mp3_metadata {
                    return Variant::from_int(mp3_info.frames[0].bitrate as i64);
                }
            }
            Field::Freq => {
                self.fms.update_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.fms.mp3_metadata {
                    return Variant::from_int(mp3_info.frames[0].sampling_freq as i64);
                }
            }
            Field::Title => {
                self.fms.update_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.fms.mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_string(&mp3_tag.title);
                    }
                }
            }
            Field::Artist => {
                self.fms.update_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.fms.mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_string(&mp3_tag.artist);
                    }
                }
            }
            Field::Album => {
                self.fms.update_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.fms.mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_string(&mp3_tag.album);
                    }
                }
            }
            Field::Year => {
                self.fms.update_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.fms.mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_int(mp3_tag.year as i64);
                    }
                }
            }
            Field::Genre => {
                self.fms.update_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.fms.mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_string(&format!("{:?}", mp3_tag.genre));
                    }
                }
            }
            Field::ExifDateTime => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("DateTime") {
                        if let Ok(exif_datetime) = parse_datetime(exif_value) {
                            return Variant::from_datetime(exif_datetime.0);
                        }
                    }
                }
            }
            Field::ExifGpsAltitude => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("__Alt") {
                        return Variant::from_float(exif_value.parse().unwrap_or(0.0));
                    }
                }
            }
            Field::ExifGpsLatitude => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("__Lat") {
                        return Variant::from_float(exif_value.parse().unwrap_or(0.0));
                    }
                }
            }
            Field::ExifGpsLongitude => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("__Lng") {
                        return Variant::from_float(exif_value.parse().unwrap_or(0.0));
                    }
                }
            }
            Field::ExifMake => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("Make") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifModel => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("Model") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifSoftware => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("Software") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifVersion => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("ExifVersion") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifExposureTime => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("ExposureTime") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifAperture => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("ApertureValue") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifShutterSpeed => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("ShutterSpeedValue") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifFNumber => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("FNumber") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifIsoSpeed => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("ISOSpeed") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifFocalLength => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("FocalLength") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifLensMake => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("LensMake") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::ExifLensModel => {
                self.fms.update_exif_metadata(entry);

                if let Some(ref exif_info) = self.fms.exif_metadata {
                    if let Some(exif_value) = exif_info.get("LensModel") {
                        return Variant::from_string(exif_value);
                    }
                }
            }
            Field::LineCount => {
                self.fms.update_line_count(entry);

                if let Some(line_count) = self.fms.line_count {
                    return Variant::from_int(line_count as i64);
                }
            }
            Field::Mime => {
                if let Some(mime) = tree_magic_mini::from_filepath(&entry.path()) {
                    return Variant::from_string(&String::from(mime));
                }

                return Variant::empty(VariantType::String);
            }
            Field::IsBinary => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(ref meta) = self.fms.file_metadata {
                    if meta.is_dir() {
                        return Variant::from_bool(false);
                    }
                }

                if let Some(mime) = tree_magic_mini::from_filepath(&entry.path()) {
                    let is_binary = !is_text_mime(mime);
                    return Variant::from_bool(is_binary);
                }

                return Variant::from_bool(false);
            }
            Field::IsText => {
                self.fms
                    .update_file_metadata(entry, self.current_follow_symlinks);

                if let Some(ref meta) = self.fms.file_metadata {
                    if meta.is_dir() {
                        return Variant::from_bool(false);
                    }
                }

                if let Some(mime) = tree_magic_mini::from_filepath(&entry.path()) {
                    let is_text = is_text_mime(mime);
                    return Variant::from_bool(is_text);
                }

                return Variant::from_bool(false);
            }
            Field::IsArchive => {
                let is_archive = match file_info {
                    Some(file_info) => self.is_archive(&file_info.name),
                    None => self.is_archive(&entry.file_name().to_string_lossy()),
                };

                return Variant::from_bool(is_archive);
            }
            Field::IsAudio => {
                let is_audio = match file_info {
                    Some(file_info) => self.is_audio(&file_info.name),
                    None => self.is_audio(&entry.file_name().to_string_lossy()),
                };

                return Variant::from_bool(is_audio);
            }
            Field::IsBook => {
                let is_book = match file_info {
                    Some(file_info) => self.is_book(&file_info.name),
                    None => self.is_book(&entry.file_name().to_string_lossy()),
                };

                return Variant::from_bool(is_book);
            }
            Field::IsDoc => {
                let is_doc = match file_info {
                    Some(file_info) => self.is_doc(&file_info.name),
                    None => self.is_doc(&entry.file_name().to_string_lossy()),
                };

                return Variant::from_bool(is_doc);
            }
            Field::IsFont => {
                let is_font = match file_info {
                    Some(file_info) => self.is_font(&file_info.name),
                    None => self.is_font(&entry.file_name().to_string_lossy()),
                };

                return Variant::from_bool(is_font);
            }
            Field::IsImage => {
                let is_image = match file_info {
                    Some(file_info) => self.is_image(&file_info.name),
                    None => self.is_image(&entry.file_name().to_string_lossy()),
                };

                return Variant::from_bool(is_image);
            }
            Field::IsSource => {
                let is_source = match file_info {
                    Some(file_info) => self.is_source(&file_info.name),
                    None => self.is_source(&entry.file_name().to_string_lossy()),
                };

                return Variant::from_bool(is_source);
            }
            Field::IsVideo => {
                let is_video = match file_info {
                    Some(file_info) => self.is_video(&file_info.name),
                    None => self.is_video(&entry.file_name().to_string_lossy()),
                };

                return Variant::from_bool(is_video);
            }
            Field::Sha1 => {
                return Variant::from_string(&crate::util::get_sha1_file_hash(entry));
            }
            Field::Sha256 => {
                return Variant::from_string(&crate::util::get_sha256_file_hash(entry));
            }
            Field::Sha512 => {
                return Variant::from_string(&crate::util::get_sha512_file_hash(entry));
            }
            Field::Sha3 => {
                return Variant::from_string(&crate::util::get_sha3_512_file_hash(entry));
            }
        };

        return Variant::empty(VariantType::String);
    }

    fn check_file(&mut self, entry: &DirEntry, root_path: &Path, file_info: &Option<FileInfo>) -> io::Result<bool> {
        self.fms.clear();

        if let Some(ref expr) = self.query.expr {
            let result = self.conforms(entry, file_info, root_path, expr);
            if !result {
                return Ok(true);
            }
        }

        self.found += 1;

        let mut file_map = HashMap::new();

        let mut buf = WritableBuffer::new();
        let mut criteria = vec!["".to_string(); self.query.ordering_fields.len()];

        for field in self.query.get_all_fields() {
            file_map.insert(
                field.to_string(),
                self.get_field_value(entry, file_info, root_path, &field).to_string(),
            );
        }

        if !self.is_buffered() && self.found > 1 {
            self.results_writer.write_row_separator(&mut buf)?;
        }

        let mut items: Vec<(String, String)> = Vec::new();

        for field in self.query.fields.iter() {
            let record =
                self.get_column_expr_value(Some(entry), file_info, root_path, &mut file_map, None, field);

            let value = match self.use_colors && field.contains_colorized() {
                true => self.colorize(&record.to_string()),
                false => record.to_string(),
            };
            items.push((field.to_string(), value));
        }

        for field in self.query.grouping_fields.iter() {
            if file_map.get(&field.to_string()).is_none() {
                self.get_column_expr_value(Some(entry), file_info, root_path, &mut file_map, None, field);
            }
        }

        for (idx, field) in self.query.ordering_fields.iter().enumerate() {
            criteria[idx] = match file_map.get(&field.to_string()) {
                Some(record) => record.clone(),
                None => self
                    .get_column_expr_value(Some(entry), file_info, root_path, &mut file_map, None, field)
                    .to_string(),
            }
        }

        self.results_writer.write_row(&mut buf, items)?;

        if self.is_buffered() {
            self.output_buffer.insert(
                Criteria::new(
                    Rc::new(self.query.ordering_fields.clone()),
                    criteria,
                    Rc::new(self.query.ordering_asc.clone()),
                ),
                String::from(buf),
            );

            if self.has_aggregate_column() {
                self.raw_output_buffer.push(file_map);
            }
        } else if let Err(e) = write!(std::io::stdout(), "{}", String::from(buf)) {
            if e.kind() == ErrorKind::BrokenPipe {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn colorize(&mut self, value: &str) -> String {
        let style;

        if let Some(ref metadata) = self.fms.file_metadata {
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

                if let Some(ref attrs) = self.fms.file_metadata {
                    return Variant::from_bool(mode_func_boxed(attrs));
                }
            }
        }

        Variant::from_bool(false)
    }

    fn conforms(&mut self, entry: &DirEntry, file_info: &Option<FileInfo>, root_path: &Path, expr: &Expr) -> bool {
        let mut result = false;

        if let Some(ref logical_op) = expr.logical_op {
            let mut left_result = false;
            let mut right_result = false;

            if let Some(ref left) = expr.left {
                let left_res = self.conforms(entry, file_info, root_path, left);
                left_result = left_res;
            }

            match logical_op {
                LogicalOp::And => {
                    if !left_result {
                        result = false;
                    } else {
                        if let Some(ref right) = expr.right {
                            let right_res = self.conforms(entry, file_info, root_path, right);
                            right_result = right_res;
                        }

                        result = left_result && right_result;
                    }
                }
                LogicalOp::Or => {
                    if left_result {
                        result = true;
                    } else {
                        if let Some(ref right) = expr.right {
                            let right_res = self.conforms(entry, file_info, root_path, right);
                            right_result = right_res;
                        }

                        result = left_result || right_result
                    }
                }
            }
        } else if let Some(ref op) = expr.op {
            let field_value = self.get_column_expr_value(
                Some(entry),
                file_info,
                root_path,
                &mut HashMap::new(),
                None,
                expr.left.as_ref().unwrap(),
            );
            let value = self.get_column_expr_value(
                Some(entry),
                file_info,
                root_path,
                &mut HashMap::new(),
                None,
                expr.right.as_ref().unwrap(),
            );

            result = match field_value.get_type() {
                VariantType::String => {
                    let val = value.to_string();
                    match op {
                        Op::Eq => match is_glob(&val) {
                            true => {
                                let regex = self.regex_cache.get(&val);
                                match regex {
                                    Some(regex) => {
                                        return regex.is_match(&field_value.to_string());
                                    }
                                    None => {
                                        let pattern = convert_glob_to_pattern(&val);
                                        let regex = Regex::new(&pattern);
                                        match regex {
                                            Ok(ref regex) => {
                                                self.regex_cache.insert(val, regex.clone());
                                                return regex.is_match(&field_value.to_string());
                                            }
                                            _ => {
                                                return val.eq(&field_value.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                            false => val.eq(&field_value.to_string()),
                        },
                        Op::Ne => match is_glob(&val) {
                            true => {
                                let regex = self.regex_cache.get(&val);
                                match regex {
                                    Some(regex) => {
                                        return !regex.is_match(&field_value.to_string());
                                    }
                                    None => {
                                        let pattern = convert_glob_to_pattern(&val);
                                        let regex = Regex::new(&pattern);
                                        match regex {
                                            Ok(ref regex) => {
                                                self.regex_cache.insert(val, regex.clone());
                                                return !regex.is_match(&field_value.to_string());
                                            }
                                            _ => {
                                                return val.ne(&field_value.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                            false => val.ne(&field_value.to_string()),
                        },
                        Op::Rx => {
                            let regex = self.regex_cache.get(&val);
                            match regex {
                                Some(regex) => {
                                    return regex.is_match(&field_value.to_string());
                                }
                                None => {
                                    let regex = Regex::new(&val);
                                    match regex {
                                        Ok(ref regex) => {
                                            self.regex_cache.insert(val, regex.clone());
                                            return regex.is_match(&field_value.to_string());
                                        }
                                        _ => error_exit("Incorrect regex expression", val.as_str()),
                                    }
                                }
                            }
                        }
                        Op::NotRx => {
                            let regex = self.regex_cache.get(&val);
                            match regex {
                                Some(regex) => {
                                    return !regex.is_match(&field_value.to_string());
                                }
                                None => {
                                    let regex = Regex::new(&val);
                                    match regex {
                                        Ok(ref regex) => {
                                            self.regex_cache.insert(val, regex.clone());
                                            return !regex.is_match(&field_value.to_string());
                                        }
                                        _ => error_exit("Incorrect regex expression", val.as_str()),
                                    }
                                }
                            }
                        }
                        Op::Like => {
                            let regex = self.regex_cache.get(&val);
                            match regex {
                                Some(regex) => {
                                    return regex.is_match(&field_value.to_string());
                                }
                                None => {
                                    let pattern = convert_like_to_pattern(&val);
                                    let regex = Regex::new(&pattern);
                                    match regex {
                                        Ok(ref regex) => {
                                            self.regex_cache.insert(val, regex.clone());
                                            return regex.is_match(&field_value.to_string());
                                        }
                                        _ => error_exit("Incorrect LIKE expression", val.as_str()),
                                    }
                                }
                            }
                        }
                        Op::NotLike => {
                            let regex = self.regex_cache.get(&val);
                            match regex {
                                Some(regex) => {
                                    return !regex.is_match(&field_value.to_string());
                                }
                                None => {
                                    let pattern = convert_like_to_pattern(&val);
                                    let regex = Regex::new(&pattern);
                                    match regex {
                                        Ok(ref regex) => {
                                            self.regex_cache.insert(val, regex.clone());
                                            return !regex.is_match(&field_value.to_string());
                                        }
                                        _ => error_exit("Incorrect LIKE expression", val.as_str()),
                                    }
                                }
                            }
                        }
                        Op::Eeq => val.eq(&field_value.to_string()),
                        Op::Ene => val.ne(&field_value.to_string()),
                        Op::In => {
                            let field_value = field_value.to_string();
                            let mut result = false;
                            let right = expr.clone().right.unwrap();
                            let args = match right.args {
                                Some(args) => args,
                                None => {
                                    if let Some(subquery) = right.subquery {
                                        self.get_list_from_subquery(*subquery).iter().map(|s| {
                                            Expr::value(s.clone().to_string())
                                        }).collect()
                                    } else {
                                        vec![]
                                    }
                                }
                            };

                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_string().eq(&field_value) {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        }
                        Op::NotIn => {
                            let field_value = field_value.to_string();
                            let mut result = true;
                            for item in expr.clone().right.unwrap().args.unwrap().iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_string().eq(&field_value) {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
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
                            let field_value = field_value.to_int();
                            let mut result = false;
                            let right = expr.clone().right.unwrap();
                            let args = match right.args {
                                Some(args) => args,
                                None => {
                                    if let Some(subquery) = right.subquery {
                                        self.get_list_from_subquery(*subquery).iter().map(|s| {
                                            Expr::value(s.clone().to_string())
                                        }).collect()
                                    } else {
                                        vec![]
                                    }
                                }
                            };

                            for item in args.iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_int() == field_value {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        },
                        Op::NotIn => {
                            let field_value = field_value.to_int();
                            let mut result = true;
                            for item in expr.clone().right.unwrap().args.unwrap().iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_int() == field_value {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
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
                            let mut result = false;
                            for item in expr.clone().right.unwrap().args.unwrap().iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_float() == field_value {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        },
                        Op::NotIn => {
                            let field_value = field_value.to_float();
                            let mut result = true;
                            for item in expr.clone().right.unwrap().args.unwrap().iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_float() == field_value {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
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
                            let mut result = false;
                            for item in expr.clone().right.unwrap().args.unwrap().iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_bool() == field_value {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        },
                        Op::NotIn => {
                            let field_value = field_value.to_bool();
                            let mut result = true;
                            for item in expr.clone().right.unwrap().args.unwrap().iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_bool() == field_value {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
                        _ => false,
                    }
                }
                VariantType::DateTime => {
                    let (start, finish) = value.to_datetime();
                    let start = start.and_utc().timestamp();
                    let finish = finish.and_utc().timestamp();
                    let dt = field_value.to_datetime().0.and_utc().timestamp();
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
                            let field_value = field_value.to_datetime().0.and_utc().timestamp();
                            let mut result = false;
                            for item in expr.clone().right.unwrap().args.unwrap().iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_datetime().0.and_utc().timestamp() == field_value {
                                    result = true;
                                    break;
                                }
                            }
                            result
                        },
                        Op::NotIn => {
                            let field_value = field_value.to_datetime().0.and_utc().timestamp();
                            let mut result = true;
                            for item in expr.clone().right.unwrap().args.unwrap().iter().map(|arg| self.get_column_expr_value(
                                Some(entry),
                                file_info,
                                root_path,
                                &mut HashMap::new(),
                                None,
                                arg,
                            )) {
                                if item.to_datetime().0.and_utc().timestamp() == field_value {
                                    result = false;
                                    break;
                                }
                            }
                            result
                        }
                        _ => false,
                    }
                }
            };
        }

        result
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
    use crate::function::Function;
    use crate::query::{OutputFormat, Query};

    // Tests for FileMetadataState
    #[test]
    fn test_file_metadata_state_new() {
        let state = FileMetadataState::new();

        assert!(!state.file_metadata_set);
        assert!(state.file_metadata.is_none());

        assert!(!state.line_count_set);
        assert!(state.line_count.is_none());

        assert!(!state.dimensions_set);
        assert!(state.dimensions.is_none());

        assert!(!state.duration_set);
        assert!(state.duration.is_none());

        assert!(!state.mp3_metadata_set);
        assert!(state.mp3_metadata.is_none());

        assert!(!state.exif_metadata_set);
        assert!(state.exif_metadata.is_none());
    }

    #[test]
    fn test_file_metadata_state_clear() {
        let mut state = FileMetadataState::new();

        // Set some values
        state.file_metadata_set = true;
        state.line_count_set = true;
        state.dimensions_set = true;
        state.duration_set = true;
        state.mp3_metadata_set = true;
        state.exif_metadata_set = true;

        // Clear the state
        state.clear();

        // Verify all values are reset
        test_file_metadata_state_new();
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
            output_format: OutputFormat::Tabs,
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
            output_format: OutputFormat::Tabs,
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
            output_format: OutputFormat::Tabs,
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
}
