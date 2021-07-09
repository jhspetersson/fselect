use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::fs::{DirEntry, FileType};
use std::fs::Metadata;
#[cfg(unix)]
use std::fs::symlink_metadata;
use std::io;
use std::io::ErrorKind;
use std::io::Write;
use std::ops::Add;
#[cfg(unix)]
use std::os::unix::fs::DirEntryExt;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

use chrono::{DateTime, Local};
use lscolors::{LsColors, Style};
use mp3_metadata;
use mp3_metadata::MP3Metadata;
use regex::Regex;
#[cfg(all(unix, feature = "users"))]
use users::{Groups, Users, UsersCache};
#[cfg(unix)]
use xattr::FileExt;
use zip;

use crate::config::Config;
use crate::expr::Expr;
use crate::field::Field;
use crate::fileinfo::FileInfo;
use crate::fileinfo::to_file_info;
use crate::function;
use crate::function::Variant;
use crate::function::VariantType;
use crate::ignore::docker::DockerignoreFilter;
use crate::ignore::docker::matches_dockerignore_filter;
use crate::ignore::docker::parse_dockerignore;
use crate::ignore::git::GitignoreFilter;
use crate::ignore::git::matches_gitignore_filter;
use crate::ignore::git::parse_gitignore;
use crate::ignore::hg::HgignoreFilter;
use crate::ignore::hg::matches_hgignore_filter;
use crate::ignore::hg::parse_hgignore;
use crate::mode;
use crate::operators::LogicalOp;
use crate::operators::Op;
use crate::query::{Query, Root, TraversalMode};
use crate::query::TraversalMode::Bfs;
use crate::util::*;
use crate::util::dimensions::get_dimensions;
use crate::output::ResultsWriter;

pub struct Searcher {
    query: Query,
    config : Config,
    use_colors: bool,
    results_writer: ResultsWriter,
    #[cfg(all(unix, feature = "users"))]
    user_cache: UsersCache,
    regex_cache: HashMap<String, Regex>,
    found: u32,
    raw_output_buffer: Vec<HashMap<String, String>>,
    output_buffer: TopN<Criteria<String>, String>,
    gitignore_map: HashMap<PathBuf, Vec<GitignoreFilter>>,
    hgignore_filters: Vec<HgignoreFilter>,
    dockerignore_filters: Vec<DockerignoreFilter>,
    visited_dirs: HashSet<PathBuf>,
    #[cfg(unix)]
    visited_inodes: HashSet<u64>,
    lscolors: LsColors,
    dir_queue: Box<VecDeque<PathBuf>>,
    current_follow_symlinks: bool,

    file_metadata: Option<Metadata>,
    file_metadata_set: bool,

    file_line_count: Option<usize>,
    file_line_count_set: bool,

    file_dimensions: Option<Dimensions>,
    file_dimensions_set: bool,

    file_mp3_metadata: Option<MP3Metadata>,
    file_mp3_metadata_set: bool,

    file_exif_metadata: Option<HashMap<String, String>>,
    file_exif_metadata_set: bool,
}

impl Searcher {
    pub fn new(query: Query, config: Config, use_colors: bool) -> Self {
        let limit = query.limit;

        let results_writer = ResultsWriter::new(&query.output_format);
        Searcher {
            query,
            config,
            use_colors,
            results_writer,
            #[cfg(all(unix, feature = "users"))]
            user_cache: UsersCache::new(),
            regex_cache: HashMap::new(),
            found: 0,
            raw_output_buffer: vec![],
            output_buffer: if limit == 0 { TopN::limitless() } else { TopN::new(limit) },
            gitignore_map: HashMap::new(),
            hgignore_filters: vec![],
            dockerignore_filters: vec![],
            visited_dirs: HashSet::new(),
            #[cfg(unix)]
            visited_inodes: HashSet::new(),
            lscolors: LsColors::from_env().unwrap_or_default(),
            dir_queue: Box::from(VecDeque::new()),
            current_follow_symlinks: false,

            file_metadata: None,
            file_metadata_set: false,

            file_line_count: None,
            file_line_count_set: false,

            file_dimensions: None,
            file_dimensions_set: false,

            file_mp3_metadata: None,
            file_mp3_metadata_set: false,

            file_exif_metadata: None,
            file_exif_metadata_set: false,
        }
    }

    fn clear_file_data(&mut self) {
        self.file_metadata_set = false;
        self.file_metadata = None;

        self.file_line_count = None;
        self.file_line_count_set = false;

        self.file_dimensions_set = false;
        self.file_dimensions = None;

        self.file_mp3_metadata_set = false;
        self.file_mp3_metadata = None;

        self.file_exif_metadata_set = false;
        self.file_exif_metadata = None;
    }

    pub fn is_buffered(&self) -> bool {
        self.has_ordering() || self.has_aggregate_column()
    }

    fn has_ordering(&self) -> bool {
        self.query.is_ordered()
    }

    fn has_aggregate_column(&self) -> bool {
        self.query.has_aggregate_column()
    }

    pub fn list_search_results(&mut self) -> io::Result<()> {
        let current_dir = std::env::current_dir().unwrap();

        if let Err(e) = self.results_writer.write_header(&mut std::io::stdout()) {
            if e.kind() == ErrorKind::BrokenPipe {
                return Ok(())
            }
        }

        let mut roots = vec![];

        for root in self.query.roots.clone() {
            if root.regexp {
                let mut ext_roots: Vec<String> = vec![];
                let parts = root.path.split('/').collect::<Vec<&str>>();
                for part in parts {
                    if self.looks_like_regexp(part) {
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

                        for root in ext_roots.clone() {
                            let mut start_from_rx_dir = false;

                            let mut path = Path::new(&root);

                            if path == Path::new("") {
                                path = current_dir.as_path();
                                start_from_rx_dir = true;
                            }

                            match path.read_dir() {
                                Ok(read_result) => {
                                    for entry in read_result {
                                        if let Ok(entry) = entry {
                                            if let Ok(file_type) = entry.file_type() {
                                                if file_type.is_dir() {
                                                    if rx.is_match(entry.file_name().to_string_lossy().as_ref()) {
                                                        if start_from_rx_dir {
                                                            tmp.push(entry.file_name().to_string_lossy().to_string());
                                                        } else {
                                                            tmp.push(entry.path().to_string_lossy().to_string());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                },
                                Err(e) => path_error_message(&path, e)
                            }
                        }

                        ext_roots.clear();
                        ext_roots.append(&mut tmp);
                    } else {
                        if ext_roots.is_empty() {
                            ext_roots.push(part.to_string());
                        } else {
                            //update all roots
                            let mut new_roots = ext_roots.iter().map(|root| root.to_string() + "/" + part).collect::<Vec<String>>();
                            ext_roots.clear();
                            ext_roots.append(&mut new_roots);
                        }
                    }
                }

                ext_roots.iter().for_each(|ext_root| roots.push(Root::clone_with_path(ext_root.to_string(), root.clone())));
            } else {
                roots.push(root);
            }
        }

        for root in roots.clone() {
            self.current_follow_symlinks = root.symlinks;

            let root_dir = Path::new(&root.path);
            let min_depth = root.min_depth;
            let max_depth = root.max_depth;
            let search_archives = root.archives;
            let apply_gitignore = root.gitignore.unwrap_or(self.config.gitignore.unwrap_or(false));
            let apply_hgignore = root.hgignore.unwrap_or(self.config.hgignore.unwrap_or(false));
            let apply_dockerignore = root.dockerignore.unwrap_or(self.config.dockerignore.unwrap_or(false));
            let traversal_mode = root.traversal;

            if apply_gitignore {
                self.search_upstream_gitignore(&root_dir);
            }

            if apply_hgignore {
                self.search_upstream_hgignore(&root_dir);
            }

            if apply_dockerignore {
                self.search_upstream_dockerignore(&root_dir);
            }

            self.dir_queue.clear();

            #[cfg(unix)]
                {
                    let metadata = match self.current_follow_symlinks {
                        true => root_dir.metadata(),
                        false => symlink_metadata(root_dir)
                    };
                    if let Ok(metadata) = metadata {
                        self.visited_inodes.insert(metadata.ino());
                    }
                }

            let _result = self.visit_dir(
                root_dir,
                min_depth,
                max_depth,
                0,
                search_archives,
                apply_gitignore,
                apply_hgignore,
                apply_dockerignore,
                traversal_mode,
                true
            );
        }

        if self.has_aggregate_column() {

            let mut buf = WritableBuffer::new();
            let mut items: Vec<(String, String)> = Vec::new();

            for column_expr in &self.query.fields.clone() {
                let record = format!("{}", self.get_column_expr_value(None, &None, &mut HashMap::new(), column_expr));
                let field_name = column_expr.to_string().to_lowercase();
                items.push((field_name, record));
            }

            self.results_writer.write_row(&mut buf, items)?;

            if let Err(e) = write!(std::io::stdout(), "{}", String::from(buf)) {
                if e.kind() == ErrorKind::BrokenPipe {
                    return Ok(());
                }
            }
        } else if self.is_buffered() {
            let mut first = true;
            for piece in self.output_buffer.values() {
                if first {
                    first = false;
                } else {
                    if let Err(e) = self.results_writer.write_row_separator(&mut std::io::stdout()){
                        if e.kind() == ErrorKind::BrokenPipe {
                            return Ok(());
                        }
                    }
                }
                if let Err(e) = write!(std::io::stdout(), "{}", piece) {
                    if e.kind() == ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                }
            }
        }

        self.results_writer.write_footer(&mut std::io::stdout())?;

        Ok(())
    }

    fn search_upstream_gitignore(&mut self, dir: &Path) {
        if let Ok(canonical_path) = crate::util::canonical_path(&dir.to_path_buf()) {
            let mut path = PathBuf::from(canonical_path);

            loop {
                let parent_found = path.pop();

                if !parent_found {
                    return;
                }

                self.update_gitignore_map(&mut path);
            }
        }
    }

    fn update_gitignore_map(&mut self, path: &Path) {
        let gitignore_file = path.join(".gitignore");
        if gitignore_file.is_file() {
            let regexes = parse_gitignore(&gitignore_file, &path);
            self.gitignore_map.insert(path.to_path_buf(), regexes);
        }
    }

    fn get_gitignore_filters(&self, dir: &Path) -> Vec<GitignoreFilter> {
        let mut result = vec![];

        for (dir_path, regexes) in &self.gitignore_map {
            if dir.to_path_buf() == *dir_path {
                for ref mut rx in regexes {
                    result.push(rx.clone());
                }

                return result;
            }
        }

        let mut path = dir.to_path_buf();

        loop {
            let parent_found = path.pop();

            if !parent_found {
                return result;
            }

            for (dir_path, regexes) in &self.gitignore_map {
                if path == *dir_path {
                    let mut tmp = vec![];
                    for ref mut rx in regexes {
                        tmp.push(rx.clone());
                    }
                    tmp.append(&mut result);
                    result.clear();
                    result.append(&mut tmp);
                }
            }
        }
    }

    fn search_upstream_hgignore(&mut self, dir: &Path) {
        if let Ok(canonical_path) = crate::util::canonical_path(&dir.to_path_buf()) {
            let mut path = PathBuf::from(canonical_path);

            loop {
                let hgignore_file = path.clone().join(".hgignore");
                let hg_directory = path.clone().join(".hg");

                if hgignore_file.is_file() && hg_directory.is_dir() {
                    self.update_hgignore_filters(&mut path);
                    return;
                }

                let parent_found = path.pop();

                if !parent_found {
                    return;
                }
            }
        }
    }

    fn update_hgignore_filters(&mut self, path: &Path) {
        let hgignore_file = path.join(".hgignore");
        if hgignore_file.is_file() {
            let regexes = parse_hgignore(&hgignore_file, &path);
            match regexes {
                Ok(ref regexes) => {
                    self.hgignore_filters.append(&mut regexes.clone());
                },
                Err(err) => {
                    eprintln!("{}: {}", path.to_string_lossy(), err);
                }
            }
        }
    }

    fn search_upstream_dockerignore(&mut self, dir: &Path) {
        if let Ok(canonical_path) = crate::util::canonical_path(&dir.to_path_buf()) {
            let mut path = PathBuf::from(canonical_path);

            loop {
                let dockerignore_file = path.clone().join(".dockerignore");

                if dockerignore_file.is_file() {
                    self.update_dockerignore_filters(&mut path);
                    return;
                }

                let parent_found = path.pop();

                if !parent_found {
                    return;
                }
            }
        }
    }

    fn update_dockerignore_filters(&mut self, path: &Path) {
        let dockerignore_file = path.join(".dockerignore");
        if dockerignore_file.is_file() {
            let regexes = parse_dockerignore(&dockerignore_file, &path);
            match regexes {
                Ok(ref regexes) => {
                    self.dockerignore_filters.append(&mut regexes.clone());
                },
                Err(err) => {
                    eprintln!("{}: {}", path.to_string_lossy(), err);
                }
            }
        }
    }

    fn visit_dir(&mut self,
                 dir: &Path,
                 min_depth: u32,
                 max_depth: u32,
                 root_depth: u32,
                 search_archives: bool,
                 apply_gitignore: bool,
                 apply_hgignore: bool,
                 apply_dockerignore: bool,
                 traversal_mode: TraversalMode,
                 process_queue: bool) -> io::Result<()> {
        if self.current_follow_symlinks {
            if self.visited_dirs.contains(&dir.to_path_buf()) {
                return Ok(());
            } else {
                self.visited_dirs.insert(dir.to_path_buf());
            }
        }

        let canonical_path = crate::util::canonical_path(&dir.to_path_buf());

        if canonical_path.is_err() {
            error_message(&dir.to_string_lossy(), String::from("could not canonicalize path: ").add(canonical_path.err().unwrap().as_str()).as_str());
            return Ok(());
        }

        let canonical_path = canonical_path.unwrap();
        let canonical_depth = crate::util::calc_depth(&canonical_path);

        let base_depth = match root_depth {
            0 => canonical_depth,
            _ => root_depth
        };

        let depth = canonical_depth - base_depth + 1;

        let mut gitignore_filters = None;

        if apply_gitignore {
            let canonical_path = PathBuf::from(canonical_path);
            self.update_gitignore_map(&canonical_path);
            gitignore_filters = Some(self.get_gitignore_filters(&canonical_path));
        }

        match fs::read_dir(dir) {
            Ok(entry_list) => {
                for entry in entry_list {
                    if !self.is_buffered() && self.query.limit > 0 && self.query.limit <= self.found {
                        break;
                    }

                    match entry {
                        Ok(entry) => {
                            let mut path = entry.path();
                            let mut canonical_path = path.clone();

                            if apply_gitignore || apply_hgignore || apply_dockerignore {
                                if let Ok(canonicalized) = crate::util::canonical_path(&path) {
                                    canonical_path = PathBuf::from(canonicalized);
                                }
                            }

                            let pass_gitignore = !apply_gitignore || !matches_gitignore_filter(&gitignore_filters, canonical_path.to_string_lossy().as_ref(), path.is_dir());
                            let pass_hgignore = !apply_hgignore || !matches_hgignore_filter(&self.hgignore_filters, canonical_path.to_string_lossy().as_ref());
                            let pass_dockerignore = !apply_dockerignore || !matches_dockerignore_filter(&self.dockerignore_filters, canonical_path.to_string_lossy().as_ref());

                            if pass_gitignore && pass_hgignore && pass_dockerignore {
                                if min_depth == 0 || depth >= min_depth {
                                    let checked = self.check_file(&entry, &None)?;
                                    if !checked {
                                        return Ok(());
                                    }

                                    if search_archives && self.is_zip_archive(&path.to_string_lossy()) {
                                        if let Ok(file) = fs::File::open(&path) {
                                            if let Ok(mut archive) = zip::ZipArchive::new(file) {
                                                for i in 0..archive.len() {
                                                    if self.query.limit > 0 && self.query.limit <= self.found {
                                                        break;
                                                    }

                                                    if let Ok(afile) = archive.by_index(i) {
                                                        let file_info = to_file_info(&afile);
                                                        let checked = self.check_file(&entry, &Some(file_info))?;
                                                        if !checked {
                                                            return Ok(());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

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

                                        if ok {
                                            if self.ok_to_visit_dir(&entry, file_type) {
                                                if traversal_mode == TraversalMode::Dfs {
                                                    let result = self.visit_dir(
                                                        &path,
                                                        min_depth,
                                                        max_depth,
                                                        base_depth,
                                                        search_archives,
                                                        apply_gitignore,
                                                        apply_hgignore,
                                                        apply_dockerignore,
                                                        traversal_mode,
                                                        false);

                                                    if result.is_err() {
                                                        path_error_message(&path, result.err().unwrap());
                                                    }
                                                } else {
                                                    self.dir_queue.push_back(path);
                                                }
                                            }
                                        }
                                    } else {
                                        path_error_message(&path, result.err().unwrap());
                                    }
                                }
                            }
                        },
                        Err(err) => {
                            path_error_message(dir, err);
                        }
                    }
                }
            },
            Err(err) => {
                path_error_message(dir, err);
            }
        }

        if traversal_mode == Bfs && process_queue {
            while !self.dir_queue.is_empty() {
                let path = self.dir_queue.pop_front().unwrap();
                let result = self.visit_dir(
                    &path,
                    min_depth,
                    max_depth,
                    base_depth,
                    search_archives,
                    apply_gitignore,
                    apply_hgignore,
                    apply_dockerignore,
                    traversal_mode,
                    false);

                if result.is_err() {
                    path_error_message(&path, result.err().unwrap());
                }
            }
        }

        Ok(())
    }

    #[cfg(unix)]
    fn ok_to_visit_dir(&mut self, entry: &DirEntry, file_type: FileType) -> bool {
        let ino = entry.ino();
        if self.visited_inodes.contains(&ino) {
            return false;
        } else {
            self.visited_inodes.insert(ino);
        }

        match self.current_follow_symlinks {
            true => true,
            false => !file_type.is_symlink()
        }
    }

    #[cfg(not(unix))]
    fn ok_to_visit_dir(&mut self, _: &DirEntry, file_type: FileType) -> bool {
        match self.current_follow_symlinks {
            true => true,
            false => !file_type.is_symlink()
        }
    }

    fn get_column_expr_value(&mut self,
                             entry: Option<&DirEntry>,
                             file_info: &Option<FileInfo>,
                             file_map: &mut HashMap<String, String>,
                             column_expr: &Expr) -> Variant {
        if let Some(ref _function) = column_expr.function {
            let result = self.get_function_value(entry, file_info, file_map, column_expr);
            file_map.insert(column_expr.to_string(), result.to_string());
            return result;
        }

        if let Some(ref field) = column_expr.field {
            if entry.is_some() {
                let result = self.get_field_value(entry.unwrap(), file_info, field);
                file_map.insert(column_expr.to_string(), result.to_string());
                return result;
            } else {
                if let Some(val) = file_map.get(&field.to_string()) {
                    return Variant::from_string(val);
                } else {
                    return Variant::empty(VariantType::String);
                }
            }
        }

        if let Some(ref value) = column_expr.val {
            return Variant::from_signed_string(&value, column_expr.minus);
        }

        let result;

        if let Some(ref left) = column_expr.left {
            let left_result = self.get_column_expr_value(entry, file_info, file_map, left);

            if let Some(ref op) = column_expr.arithmetic_op {
                if let Some(ref right) = column_expr.right {
                    let right_result = self.get_column_expr_value(entry, file_info, file_map, right);
                    result = op.calc(&left_result, &right_result);
                    file_map.insert(column_expr.to_string(), result.to_string());
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

    fn get_function_value(&mut self,
                          entry: Option<&DirEntry>,
                          file_info: &Option<FileInfo>,
                          file_map: &mut HashMap<String, String>,
                          column_expr: &Expr) -> Variant {
        let dummy = Expr::value(String::from(""));
        let boxed_dummy = &Box::from(dummy);

        let left_expr = match &column_expr.left {
            Some(left_expr) => left_expr,
            _ => boxed_dummy
        };

        let function = &column_expr.function.as_ref().unwrap();

        if function.is_aggregate_function() {
            let _ = self.get_column_expr_value(entry, file_info, file_map, left_expr);
            let buffer_key = left_expr.to_string();
            let aggr_result = function::get_aggregate_value(&column_expr.function,
                                                            &self.raw_output_buffer,
                                                            //left_expr.field.as_ref().unwrap().to_string().to_lowercase(),
                                                            buffer_key,
                                                            &column_expr.val);
            return Variant::from_string(&aggr_result);
        } else {
            let function_arg = self.get_column_expr_value(entry, file_info, file_map, left_expr);
            let mut function_args = vec![];
            if let Some(args) = &column_expr.args {
                for arg in args {
                    let arg_value = self.get_column_expr_value(entry, file_info, file_map, arg);
                    function_args.push(arg_value.to_string());
                }
            }
            let result = function::get_value(&column_expr.function, function_arg.to_string(), function_args, entry, file_info);
            file_map.insert(column_expr.to_string(), result.to_string());

            return result;
        }
    }

    fn update_file_metadata(&mut self, entry: &DirEntry) {
        if !self.file_metadata_set {
            self.file_metadata_set = true;
            self.file_metadata = get_metadata(entry, self.current_follow_symlinks);
        }
    }

    fn update_file_line_count(&mut self, entry: &DirEntry) {
        if !self.file_line_count_set {
            self.file_line_count_set = true;
            self.file_line_count = crate::util::get_line_count(entry);
        }
    }

    fn update_file_mp3_metadata(&mut self, entry: &DirEntry) {
        if !self.file_mp3_metadata_set {
            self.file_mp3_metadata_set = true;
            self.file_mp3_metadata = get_mp3_metadata(entry);
        }
    }

    fn update_file_exif_metadata(&mut self, entry: &DirEntry) {
        if !self.file_exif_metadata_set {
            self.file_exif_metadata_set = true;
            self.file_exif_metadata = get_exif_metadata(entry);
        }
    }

    fn get_field_value(&mut self,
                       entry: &DirEntry,
                       file_info: &Option<FileInfo>,
                       field: &Field) -> Variant {
        if file_info.is_some() && !field.is_available_for_archived_files() {
            return Variant::empty(VariantType::String);
        }

        match field {
            Field::Name => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_string(&format!("[{}] {}", entry.file_name().to_string_lossy(), file_info.name));
                    },
                    _ => {
                        return Variant::from_string(&format!("{}", entry.file_name().to_string_lossy()));
                    }
                }
            },
            Field::Extension => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_string(&format!("[{}] {}", entry.file_name().to_string_lossy(), crate::util::get_extension(&file_info.name)));
                    },
                    _ => {
                        return Variant::from_string(&format!("{}", crate::util::get_extension(&entry.file_name().to_string_lossy())));
                    }
                }
            },
            Field::Path => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_string(&format!("[{}] {}", entry.path().to_string_lossy(), file_info.name));
                    },
                    _ => {
                        return Variant::from_string(&format!("{}", entry.path().to_string_lossy()));
                    }
                }
            },
            Field::AbsPath => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_string(&format!("[{}] {}", entry.path().to_string_lossy(), file_info.name));
                    },
                    _ => {
                        if let Ok(path) = crate::util::canonical_path(&entry.path()) {
                            return Variant::from_string(&path);
                        }
                    }
                }
            },
            Field::Directory => {
                let file_path = match file_info {
                    Some(ref file_info) => file_info.name.clone(),
                    _ => entry.path().to_string_lossy().to_string()
                };
                let pb = PathBuf::from(file_path);
                if let Some(parent) = pb.parent() {
                    return Variant::from_string(&parent.to_string_lossy().to_string());
                }
            },
            Field::AbsDir => {
                let file_path = match file_info {
                    Some(ref file_info) => file_info.name.clone(),
                    _ => entry.path().to_string_lossy().to_string()
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
            },
            Field::Size => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_int(file_info.size as i64);
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_int(attrs.len() as i64);
                        }
                    }
                }
            },
            Field::FormattedSize => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_string( &format_filesize(file_info.size, self.config.default_file_size_format.as_ref().unwrap_or(&String::new())));
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_string(&format_filesize(attrs.len(), self.config.default_file_size_format.as_ref().unwrap_or(&String::new())));
                        }
                    }
                }
            },
            Field::IsDir => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_bool(file_info.name.ends_with('/') || file_info.name.ends_with('\\'));
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_bool(attrs.is_dir());
                        }
                    }
                }
            },
            Field::IsFile => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_bool(!file_info.name.ends_with('/'));
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_bool(attrs.is_file());
                        }
                    }
                }
            },
            Field::IsSymlink => {
                match file_info {
                    Some(_) => {
                        return Variant::from_bool(false);
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_bool(attrs.file_type().is_symlink());
                        }
                    }
                }
            },
            Field::IsPipe => {
                return self.check_file_mode(entry, &mode::is_pipe, &file_info, &mode::mode_is_pipe);
            },
            Field::IsCharacterDevice => {
                return self.check_file_mode(entry, &mode::is_char_device, &file_info, &mode::mode_is_char_device);
            },
            Field::IsBlockDevice => {
                return self.check_file_mode(entry, &mode::is_block_device, &file_info, &mode::mode_is_block_device);
            },
            Field::IsSocket => {
                return self.check_file_mode(entry, &mode::is_socket, &file_info, &mode::mode_is_socket);
            },
            Field::Device => {
                #[cfg(unix)]
                    {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_int(attrs.dev() as i64);
                        }
                    }

                return Variant::empty(VariantType::String);
            },
            Field::Inode => {
                #[cfg(unix)]
                    {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_int(attrs.ino() as i64);
                        }
                    }

                return Variant::empty(VariantType::String);
            },
            Field::Blocks => {
                #[cfg(unix)]
                    {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_int(attrs.blocks() as i64);
                        }
                    }

                return Variant::empty(VariantType::String);
            },
            Field::Hardlinks => {
                #[cfg(unix)]
                    {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_int(attrs.nlink() as i64);
                        }
                    }

                return Variant::empty(VariantType::String);
            },
            Field::Mode => {
                match file_info {
                    Some(ref file_info) => {
                        if let Some(mode) = file_info.mode {
                            return Variant::from_string(&mode::format_mode(mode));
                        }
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return Variant::from_string(&mode::get_mode(attrs));
                        }
                    }
                }
            },
            Field::UserRead => {
                return self.check_file_mode(entry, &mode::user_read, &file_info, &mode::mode_user_read);
            },
            Field::UserWrite => {
                return self.check_file_mode(entry, &mode::user_write, &file_info, &mode::mode_user_write);
            },
            Field::UserExec => {
                return self.check_file_mode(entry, &mode::user_exec, &file_info, &mode::mode_user_exec);
            },
            Field::UserAll => {
                return self.check_file_mode(entry, &mode::user_all, &file_info, &mode::mode_user_all);
            },
            Field::GroupRead => {
                return self.check_file_mode(entry, &mode::group_read, &file_info, &mode::mode_group_read);
            },
            Field::GroupWrite => {
                return self.check_file_mode(entry, &mode::group_write, &file_info, &mode::mode_group_write);
            },
            Field::GroupExec => {
                return self.check_file_mode(entry, &mode::group_exec, &file_info, &mode::mode_group_exec);
            },
            Field::GroupAll => {
                return self.check_file_mode(entry, &mode::group_all, &file_info, &mode::mode_group_all);
            },
            Field::OtherRead => {
                return self.check_file_mode(entry, &mode::other_read, &file_info, &mode::mode_other_read);
            },
            Field::OtherWrite => {
                return self.check_file_mode(entry, &mode::other_write, &file_info, &mode::mode_other_write);
            },
            Field::OtherExec => {
                return self.check_file_mode(entry, &mode::other_exec, &file_info, &mode::mode_other_exec);
            },
            Field::OtherAll => {
                return self.check_file_mode(entry, &mode::other_all, &file_info, &mode::mode_other_all);
            },
            Field::Suid => {
                return self.check_file_mode(entry, &mode::suid_bit_set, &file_info, &mode::mode_suid);
            },
            Field::Sgid => {
                return self.check_file_mode(entry, &mode::sgid_bit_set, &file_info, &mode::mode_sgid);
            },
            Field::IsHidden => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_bool(is_hidden(&file_info.name, &None, true));
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        return Variant::from_bool(is_hidden(&entry.file_name().to_string_lossy(), &self.file_metadata, false));
                    }
                }
            },
            Field::Uid => {
                self.update_file_metadata(entry);

                if let Some(ref attrs) = self.file_metadata {
                    if let Some(uid) = mode::get_uid(attrs) {
                        return Variant::from_int(uid as i64);
                    }
                }
            },
            Field::Gid => {
                self.update_file_metadata(entry);

                if let Some(ref attrs) = self.file_metadata {
                    if let Some(gid) = mode::get_gid(attrs) {
                        return Variant::from_int(gid as i64);
                    }
                }
            },
            Field::User => {
                #[cfg(all(unix, feature = "users"))]
                {
                    self.update_file_metadata(entry);

                    if let Some(ref attrs) = self.file_metadata {
                        if let Some(uid) = mode::get_uid(attrs) {
                            if let Some(user) = self.user_cache.get_user_by_uid(uid) {
                                return Variant::from_string(&user.name().to_string_lossy().to_string());
                            }
                        }
                    }
                }
            },
            Field::Group => {
                #[cfg(all(unix, feature = "users"))]
                {
                    self.update_file_metadata(entry);

                    if let Some(ref attrs) = self.file_metadata {
                        if let Some(gid) = mode::get_gid(attrs) {
                            if let Some(group) = self.user_cache.get_group_by_gid(gid) {
                                return Variant::from_string(&group.name().to_string_lossy().to_string());
                            }
                        }
                    }
                }
            },
            Field::Created => {
                self.update_file_metadata(entry);

                if let Some(ref attrs) = self.file_metadata {
                    if let Ok(sdt) = attrs.created() {
                        let dt: DateTime<Local> = DateTime::from(sdt);
                        return Variant::from_datetime(dt);
                    }
                }
            },
            Field::Accessed => {
                self.update_file_metadata(entry);

                if let Some(ref attrs) = self.file_metadata {
                    if let Ok(sdt) = attrs.accessed() {
                        let dt: DateTime<Local> = DateTime::from(sdt);
                        return Variant::from_datetime(dt);
                    }
                }
            },
            Field::Modified => {
                match file_info {
                    Some(ref file_info) => {
                        let dt: DateTime<Local> = to_local_datetime(&file_info.modified);
                        return Variant::from_datetime(dt);
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            if let Ok(sdt) = attrs.modified() {
                                let dt: DateTime<Local> = DateTime::from(sdt);
                                return Variant::from_datetime(dt);
                            }
                        }
                    }
                }
            },
            Field::HasXattrs => {
                #[cfg(unix)]
                    {
                        if let Ok(file) = fs::File::open(&entry.path()) {
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
            },
            Field::IsShebang => {
                return Variant::from_bool(is_shebang(&entry.path()));
            },
            Field::IsEmpty => {
                match file_info {
                    Some(ref file_info) => {
                        return Variant::from_bool(file_info.size == 0);
                    },
                    _ => {
                        self.update_file_metadata(entry);

                        if let Some(ref attrs) = self.file_metadata {
                            return match attrs.is_dir() {
                                true =>  match is_dir_empty(entry) {
                                    Some(result) => Variant::from_bool(result),
                                    None => Variant::empty(VariantType::Bool)
                                },
                                false => Variant::from_bool(attrs.len() == 0)
                            };
                        }
                    }
                }
            },
            Field::Width => {
                if !self.file_dimensions_set {
                    self.file_dimensions_set = true;
                    self.file_dimensions = get_dimensions(entry.path());
                }

                if let Some(Dimensions { width, .. }) = self.file_dimensions {
                    return Variant::from_int(width as i64);
                }
            },
            Field::Height => {
                if !self.file_dimensions_set {
                    self.file_dimensions_set = true;
                    self.file_dimensions = get_dimensions(entry.path());
                }

                if let Some(Dimensions { height, .. }) = self.file_dimensions {
                    return Variant::from_int(height as i64);
                }
            },
            Field::Duration => {
                self.update_file_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.file_mp3_metadata {
                    return Variant::from_int(mp3_info.duration.as_secs() as i64);
                }
            },
            Field::Bitrate => {
                self.update_file_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.file_mp3_metadata {
                    return Variant::from_int(mp3_info.frames[0].bitrate as i64);
                }
            },
            Field::Freq => {
                self.update_file_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.file_mp3_metadata {
                    return Variant::from_int(mp3_info.frames[0].sampling_freq as i64);
                }
            },
            Field::Title => {
                self.update_file_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.file_mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_string(&mp3_tag.title);
                    }
                }
            },
            Field::Artist => {
                self.update_file_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.file_mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_string(&mp3_tag.artist);
                    }
                }
            },
            Field::Album => {
                self.update_file_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.file_mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_string(&mp3_tag.album);
                    }
                }
            },
            Field::Year => {
                self.update_file_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.file_mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_int(mp3_tag.year as i64);
                    }
                }
            },
            Field::Genre => {
                self.update_file_mp3_metadata(entry);

                if let Some(ref mp3_info) = self.file_mp3_metadata {
                    if let Some(ref mp3_tag) = mp3_info.tag {
                        return Variant::from_string(&format!("{:?}", mp3_tag.genre));
                    }
                }
            },
            Field::ExifDateTime => {
                self.update_file_exif_metadata(entry);

                if let Some(ref exif_info) = self.file_exif_metadata {
                    if let Some(exif_value) = exif_info.get("DateTime") {
                        if let Ok(exif_datetime) = parse_datetime(&exif_value) {
                            return Variant::from_datetime(exif_datetime.0);
                        }
                    }
                }
            },
            Field::ExifGpsAltitude => {
                self.update_file_exif_metadata(entry);

                if let Some(ref exif_info) = self.file_exif_metadata {
                    if let Some(exif_value) = exif_info.get("__Alt") {
                        return Variant::from_float(exif_value.parse().unwrap_or(0.0));
                    }
                }
            },
            Field::ExifGpsLatitude => {
                self.update_file_exif_metadata(entry);

                if let Some(ref exif_info) = self.file_exif_metadata {
                    if let Some(exif_value) = exif_info.get("__Lat") {
                        return Variant::from_float(exif_value.parse().unwrap_or(0.0));
                    }
                }
            },
            Field::ExifGpsLongitude => {
                self.update_file_exif_metadata(entry);

                if let Some(ref exif_info) = self.file_exif_metadata {
                    if let Some(exif_value) = exif_info.get("__Lng") {
                        return Variant::from_float(exif_value.parse().unwrap_or(0.0));
                    }
                }
            },
            Field::ExifMake => {
                self.update_file_exif_metadata(entry);

                if let Some(ref exif_info) = self.file_exif_metadata {
                    if let Some(exif_value) = exif_info.get("Make") {
                        return Variant::from_string(&exif_value);
                    }
                }
            },
            Field::ExifModel => {
                self.update_file_exif_metadata(entry);

                if let Some(ref exif_info) = self.file_exif_metadata {
                    if let Some(exif_value) = exif_info.get("Model") {
                        return Variant::from_string(&exif_value);
                    }
                }
            },
            Field::ExifSoftware => {
                self.update_file_exif_metadata(entry);

                if let Some(ref exif_info) = self.file_exif_metadata {
                    if let Some(exif_value) = exif_info.get("Software") {
                        return Variant::from_string(&exif_value);
                    }
                }
            },
            Field::ExifVersion => {
                self.update_file_exif_metadata(entry);

                if let Some(ref exif_info) = self.file_exif_metadata {
                    if let Some(exif_value) = exif_info.get("ExifVersion") {
                        return Variant::from_string(&exif_value);
                    }
                }
            },
            Field::LineCount => {
                self.update_file_line_count(entry);

                if let Some(line_count) = self.file_line_count {
                    return Variant::from_int(line_count as i64);
                }
            },
            Field::Mime => {
                let mime = tree_magic::from_filepath(&entry.path());

                return Variant::from_string(&mime);
            },
            Field::IsBinary => {
                self.update_file_metadata(entry);

                if let Some(ref meta) = self.file_metadata {
                    if meta.is_dir() {
                        return Variant::from_bool(false);
                    }
                }

                let mime = tree_magic::from_filepath(&entry.path());
                let is_binary = !is_text_mime(&mime);

                return Variant::from_bool(is_binary);
            },
            Field::IsText => {
                self.update_file_metadata(entry);

                if let Some(ref meta) = self.file_metadata {
                    if meta.is_dir() {
                        return Variant::from_bool(false);
                    }
                }

                let mime = tree_magic::from_filepath(&entry.path());
                let is_text = is_text_mime(&mime);

                return Variant::from_bool(is_text);
            },
            Field::IsArchive => {
                let is_archive = match file_info {
                    Some(file_info) => self.is_archive(&file_info.name),
                    None => self.is_archive(&entry.file_name().to_string_lossy())
                };

                return Variant::from_bool(is_archive);
            },
            Field::IsAudio => {
                let is_audio = match file_info {
                    Some(file_info) => self.is_audio(&file_info.name),
                    None => self.is_audio(&entry.file_name().to_string_lossy())
                };

                return Variant::from_bool(is_audio);
            },
            Field::IsBook => {
                let is_book = match file_info {
                    Some(file_info) => self.is_book(&file_info.name),
                    None => self.is_book(&entry.file_name().to_string_lossy())
                };

                return Variant::from_bool(is_book);
            },
            Field::IsDoc => {
                let is_doc = match file_info {
                    Some(file_info) => self.is_doc(&file_info.name),
                    None => self.is_doc(&entry.file_name().to_string_lossy())
                };

                return Variant::from_bool(is_doc);
            },
            Field::IsImage => {
                let is_image = match file_info {
                    Some(file_info) => self.is_image(&file_info.name),
                    None => self.is_image(&entry.file_name().to_string_lossy())
                };

                return Variant::from_bool(is_image);
            },
            Field::IsSource => {
                let is_source = match file_info {
                    Some(file_info) => self.is_source(&file_info.name),
                    None => self.is_source(&entry.file_name().to_string_lossy())
                };

                return Variant::from_bool(is_source);
            },
            Field::IsVideo => {
                let is_video = match file_info {
                    Some(file_info) => self.is_video(&file_info.name),
                    None => self.is_video(&entry.file_name().to_string_lossy())
                };

                return Variant::from_bool(is_video);
            },
            Field::Sha1 => {
                return Variant::from_string(&crate::util::get_sha1_file_hash(&entry));
            },
            Field::Sha256 => {
                return Variant::from_string(&crate::util::get_sha256_file_hash(&entry));
            },
            Field::Sha512 => {
                return Variant::from_string(&crate::util::get_sha512_file_hash(&entry));
            },
            Field::Sha3 => {
                return Variant::from_string(&crate::util::get_sha3_512_file_hash(&entry));
            }
        };

        return Variant::empty(VariantType::String);
    }

    fn check_file(&mut self,
                  entry: &DirEntry,
                  file_info: &Option<FileInfo>) -> std::io::Result<bool> {
        self.clear_file_data();

        if let Some(ref expr) = self.query.expr.clone() {
            let result = self.conforms(entry, file_info, expr);
            if !result {
                return Ok(true);
            }
        }

        self.found += 1;

        let mut file_map = HashMap::new();

        let mut buf = WritableBuffer::new();
        let mut criteria = vec!["".to_string(); self.query.ordering_fields.len()];

        for field in self.query.get_all_fields() {
            file_map.insert(field.to_string(), self.get_field_value(entry, file_info, &field).to_string());
        }


        if !self.is_buffered() && self.found > 1 {
            self.results_writer.write_row_separator(&mut buf)?;
        }

        let mut items: Vec<(String, String)> = Vec::new();

        for field in self.query.fields.clone().iter() {
            let record = self.get_column_expr_value(Some(entry), file_info, &mut file_map, &field);

            let value = match self.use_colors && field.contains_colorized() {
                true => self.colorize(&record.to_string()),
                false => record.to_string()
            };
            items.push((field.to_string(), value));
        }

        for (idx, field) in self.query.ordering_fields.clone().iter().enumerate() {
            criteria[idx] = match file_map.get(&field.to_string()) {
                Some(record) => record.clone(),
                None => self.get_column_expr_value(Some(entry), file_info, &mut file_map, &field).to_string()
            }
        }

        self.results_writer.write_row(&mut buf, items)?;

        if self.is_buffered() {
            self.output_buffer.insert(Criteria::new(self.query.ordering_fields.clone(), criteria, self.query.ordering_asc.clone()), String::from(buf));

            if self.has_aggregate_column() {
                self.raw_output_buffer.push(file_map);
            }
        } else {
            if let Err(e) = write!(std::io::stdout(), "{}", String::from(buf)) {
                if e.kind() == ErrorKind::BrokenPipe {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    fn colorize(&mut self, value: &str) -> String {
        let style;

        if let Some(ref metadata) = self.file_metadata {
            style = self.lscolors.style_for_path_with_metadata(Path::new(&value), Some(metadata));
        } else {
            style = self.lscolors.style_for_path(Path::new(&value));
        }

        let ansi_style = style.map(Style::to_ansi_term_style).unwrap_or_default();

        format!("{}", ansi_style.paint(value))
    }

    fn check_file_mode(&mut self,
                       entry: &DirEntry,
                       mode_func_boxed: &dyn Fn(&Metadata) -> bool,
                       file_info: &Option<FileInfo>,
                       mode_func_i32: &dyn Fn(u32) -> bool) -> Variant {
        match file_info {
            Some(ref file_info) => {
                if let Some(mode) = file_info.mode {
                    return Variant::from_bool(mode_func_i32(mode));
                }
            },
            _ => {
                self.update_file_metadata(entry);

                if let Some(ref attrs) = self.file_metadata {
                    return Variant::from_bool(mode_func_boxed(attrs));
                }
            }
        }

        Variant::from_bool(false)
    }

    fn conforms(&mut self,
                entry: &DirEntry,
                file_info: &Option<FileInfo>,
                expr: &Expr) -> bool {
        let mut result = false;

        if let Some(ref logical_op) = expr.logical_op {
            let mut left_result = false;
            let mut right_result = false;

            if let Some(ref left) = expr.left {
                let left_res = self.conforms(entry, file_info, &left);
                left_result = left_res;
            }

            match logical_op {
                LogicalOp::And => {
                    if !left_result {
                        result = false;
                    } else {
                        if let Some(ref right) = expr.right {
                            let right_res = self.conforms(entry, file_info, &right);
                            right_result = right_res;
                        }

                        result = left_result && right_result;
                    }
                },
                LogicalOp::Or => {
                    if left_result {
                        result = true;
                    } else {
                        if let Some(ref right) = expr.right {
                            let right_res = self.conforms(entry, file_info, &right);
                            right_result = right_res;
                        }

                        result = left_result || right_result
                    }
                }
            }
        } else if let Some(ref op) = expr.op {
            let field_value = self.get_column_expr_value(Some(entry), file_info, &mut HashMap::new(), expr.left.as_ref().unwrap());
            let value = self.get_column_expr_value(Some(entry), file_info, &mut HashMap::new(), expr.right.as_ref().unwrap());

            result = match field_value.get_type() {
                VariantType::String => {
                    let val = value.to_string();
                    match op {
                        Op::Eq => {
                            match is_glob(&val) {
                                true => {
                                    let regex = self.regex_cache.get(&val);
                                    match regex {
                                        Some(ref regex) => {
                                            return regex.is_match(&field_value.to_string());
                                        },
                                        None => {
                                            let pattern = convert_glob_to_pattern(&val);
                                            let regex = Regex::new(&pattern);
                                            match regex {
                                                Ok(ref regex) => {
                                                    self.regex_cache.insert(val, regex.clone());
                                                    return regex.is_match(&field_value.to_string());
                                                },
                                                _ => {
                                                    return val.eq(&field_value.to_string());
                                                }
                                            }
                                        }
                                    }
                                },
                                false => val.eq(&field_value.to_string())
                            }
                        },
                        Op::Ne => {
                            match is_glob(&val) {
                                true => {
                                    let regex = self.regex_cache.get(&val);
                                    match regex {
                                        Some(ref regex) => {
                                            return !regex.is_match(&field_value.to_string());
                                        },
                                        None => {
                                            let pattern = convert_glob_to_pattern(&val);
                                            let regex = Regex::new(&pattern);
                                            match regex {
                                                Ok(ref regex) => {
                                                    self.regex_cache.insert(val, regex.clone());
                                                    return !regex.is_match(&field_value.to_string());
                                                },
                                                _ => {
                                                    return val.ne(&field_value.to_string());
                                                }
                                            }
                                        }
                                    }
                                },
                                false => val.ne(&field_value.to_string())
                            }
                        },
                        Op::Rx => {
                            let regex = self.regex_cache.get(&val);
                            match regex {
                                Some(ref regex) => {
                                    return regex.is_match(&field_value.to_string());
                                },
                                None => {
                                    let regex = Regex::new(&val);
                                    match regex {
                                        Ok(ref regex) => {
                                            self.regex_cache.insert(val, regex.clone());
                                            return regex.is_match(&field_value.to_string());
                                        },
                                        _ => {
                                            panic!("Incorrect regex expression")
                                        }
                                    }
                                }
                            }
                        },
                        Op::NotRx => {
                            let regex = self.regex_cache.get(&val);
                            match regex {
                                Some(ref regex) => {
                                    return !regex.is_match(&field_value.to_string());
                                },
                                None => {
                                    let regex = Regex::new(&val);
                                    match regex {
                                        Ok(ref regex) => {
                                            self.regex_cache.insert(val, regex.clone());
                                            return !regex.is_match(&field_value.to_string());
                                        },
                                        _ => {
                                            panic!("Incorrect regex expression")
                                        }
                                    }
                                }
                            }
                        },
                        Op::Like => {
                            let regex = self.regex_cache.get(&val);
                            match regex {
                                Some(ref regex) => {
                                    return regex.is_match(&field_value.to_string());
                                },
                                None => {
                                    let pattern = convert_like_to_pattern(&val);
                                    let regex = Regex::new(&pattern);
                                    match regex {
                                        Ok(ref regex) => {
                                            self.regex_cache.insert(val, regex.clone());
                                            return regex.is_match(&field_value.to_string());
                                        },
                                        _ => {
                                            panic!("Incorrect LIKE expression")
                                        }
                                    }
                                }
                            }
                        },
                        Op::NotLike => {
                            let regex = self.regex_cache.get(&val);
                            match regex {
                                Some(ref regex) => {
                                    return !regex.is_match(&field_value.to_string());
                                },
                                None => {
                                    let pattern = convert_like_to_pattern(&val);
                                    let regex = Regex::new(&pattern);
                                    match regex {
                                        Ok(ref regex) => {
                                            self.regex_cache.insert(val, regex.clone());
                                            return !regex.is_match(&field_value.to_string());
                                        },
                                        _ => {
                                            panic!("Incorrect LIKE expression")
                                        }
                                    }
                                }
                            }
                        },
                        Op::Eeq => val.eq(&field_value.to_string()),
                        Op::Ene => val.ne(&field_value.to_string()),
                        _ => false
                    }
                },
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
                        _ => false
                    }
                },
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
                        _ => false
                    }
                },
                VariantType::Bool => {
                    let val = value.to_bool();
                    match op {
                        Op::Eq | Op::Eeq => field_value.to_bool() == val,
                        Op::Ne | Op::Ene => field_value.to_bool() != val,
                        Op::Gt => field_value.to_bool() > val,
                        Op::Gte => field_value.to_bool() >= val,
                        Op::Lt => field_value.to_bool() < val,
                        Op::Lte => field_value.to_bool() <= val,
                        _ => false
                    }
                },
                VariantType::DateTime => {
                    let (start, finish) = value.to_datetime();
                    let start = start.timestamp();
                    let finish = finish.timestamp();
                    let dt = field_value.to_datetime().0.timestamp();
                    match op {
                        Op::Eeq => dt == start,
                        Op::Ene => dt != start,
                        Op::Eq => dt >= start && dt <= finish,
                        Op::Ne => dt < start || dt > finish,
                        Op::Gt => dt > finish,
                        Op::Gte => dt >= start,
                        Op::Lt => dt < start,
                        Op::Lte => dt <= finish,
                        _ => false
                    }
                }
            };
        }

        result
    }

    fn is_zip_archive(&self, file_name: &str) -> bool {
        has_extension(file_name, &self.config.is_zip_archive)
    }

    fn is_archive(&self, file_name: &str) -> bool {
        has_extension(file_name, &self.config.is_archive)
    }

    fn is_audio(&self, file_name: &str) -> bool {
        has_extension(file_name, &self.config.is_audio)
    }

    fn is_book(&self, file_name: &str) -> bool {
        has_extension(file_name, &self.config.is_book)
    }

    fn is_doc(&self, file_name: &str) -> bool {
        has_extension(file_name, &self.config.is_doc)
    }

    fn is_image(&self, file_name: &str) -> bool {
        has_extension(file_name, &self.config.is_image)
    }

    fn is_source(&self, file_name: &str) -> bool {
        has_extension(file_name, &self.config.is_source)
    }

    fn is_video(&self, file_name: &str) -> bool {
        has_extension(file_name, &self.config.is_video)
    }

    fn looks_like_regexp(&self, s: &str) -> bool {
        s.contains('*') || s.contains('[') || s.contains('?')
    }
}
