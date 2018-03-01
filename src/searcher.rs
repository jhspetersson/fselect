use std::error::Error;
use std::fs;
use std::fs::DirEntry;
use std::fs::Metadata;
use std::fs::symlink_metadata;
use std::path::Path;
use std::io;

use chrono::DateTime;
use chrono::Local;
use csv;
use humansize::{FileSize, file_size_opts};
use imagesize;
use term;
use term::StdoutTerminal;
#[cfg(unix)]
use users::{Groups, Users, UsersCache};
use zip;

use mode;
use parser::Query;
use parser::Expr;
use parser::LogicalOp;
use parser::Op;
use parser::OutputFormat;

pub struct Searcher {
    query: Query,
    user_cache: UsersCache,
    found: u32,
}

impl Searcher {
    pub fn new(query: Query) -> Self {
        Searcher {
            query,
            user_cache: UsersCache::new(),
            found: 0
        }
    }

    pub fn list_search_results(&mut self, t: &mut Box<StdoutTerminal>) -> io::Result<()> {
        let need_metadata = self.query.fields.iter()
            .filter(|s| s.as_str().ne("name")).count() > 0;

        let need_dim = self.query.fields.iter()
            .filter(|s| {
                let str = s.as_str();
                str.eq("width") || str.eq("height")
            }).count() > 0;

        for root in &self.query.clone().roots {
            let root_dir = Path::new(&root.path);
            let max_depth = root.depth;
            let search_archives = root.archives;
            let follow_symlinks = root.symlinks;
            let _result = self.visit_dirs(
                root_dir,
                need_metadata,
                need_dim,
                max_depth,
                1,
                search_archives,
                follow_symlinks,
                t
            );
        }

        Ok(())
    }

    fn visit_dirs(&mut self,
                  dir: &Path,
                  need_metadata: bool,
                  need_dim: bool,
                  max_depth: u32,
                  depth: u32,
                  search_archives: bool,
                  follow_symlinks: bool,
                  t: &mut Box<StdoutTerminal>) -> io::Result<()> {
        if max_depth == 0 || (max_depth > 0 && depth <= max_depth) {
            let metadata = match follow_symlinks {
                true => dir.metadata(),
                false => symlink_metadata(dir)
            };
            match metadata {
                Ok(metadata) => {
                    if metadata.is_dir() {
                        match fs::read_dir(dir) {
                            Ok(entry_list) => {
                                for entry in entry_list {
                                    if self.query.limit > 0 && self.query.limit <= self.found {
                                        break;
                                    }

                                    match entry {
                                        Ok(entry) => {
                                            let path = entry.path();

                                            self.check_file(&entry, &None, need_metadata, need_dim, follow_symlinks);

                                            if search_archives && is_zip_archive(&path.to_string_lossy()) {
                                                if let Ok(file) = fs::File::open(&path) {
                                                    if let Ok(mut archive) = zip::ZipArchive::new(file) {
                                                        for i in 0..archive.len() {
                                                            if self.query.limit > 0 && self.query.limit <= self.found {
                                                                break;
                                                            }

                                                            if let Ok(afile) = archive.by_index(i) {
                                                                let file_info = to_file_info(&afile);
                                                                self.check_file(&entry, &Some(file_info), need_metadata, need_dim, false);
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            if path.is_dir() {
                                                let result = self.visit_dirs(&path, need_metadata, need_dim, max_depth, depth + 1, search_archives, follow_symlinks, t);
                                                if result.is_err() {
                                                    error_message(&path, result.err().unwrap(), t);
                                                }
                                            }
                                        },
                                        Err(err) => {
                                            error_message(dir, err, t);
                                        }
                                    }
                                }
                            },
                            Err(err) => {
                                error_message(dir, err, t);
                            }
                        }
                    }
                },
                Err(err) => {
                    error_message(dir, err, t);
                }
            }
        }

        Ok(())
    }

    fn check_file(&mut self,
                  entry: &DirEntry,
                  file_info: &Option<FileInfo>,
                  need_metadata: bool,
                  need_dim: bool,
                  follow_symlinks: bool) {
        let mut meta = None;
        let mut dim = None;
        if let Some(ref expr) = self.query.expr.clone() {
            let (result, entry_meta, entry_dim) = self.conforms(entry, file_info, expr, None, None, follow_symlinks);
            if !result {
                return
            }

            meta = entry_meta;
            dim = entry_dim;
        }

        self.found += 1;

        let attrs = match need_metadata {
            true => update_meta(entry, meta, follow_symlinks),
            false => None
        };

        let dimensions = match need_dim {
            true => {
                if dim.is_some() {
                    dim
                } else {
                    match imagesize::size(entry.path()) {
                        Ok(imgsize) => Some((imgsize.width, imgsize.height)),
                        _ => None
                    }
                }
            },
            false => None
        };

        let mut csv_writer = None;
        let mut records = vec![];
        if let OutputFormat::Csv = self.query.output_format {
            csv_writer = Some(csv::Writer::from_writer(io::stdout()));
        }

        for field in self.query.fields.iter() {
            let mut record = String::new();
            match field.as_str() {
                "name" => {
                    match file_info {
                        &Some(ref file_info) => {
                            record = format!("[{}] {}", entry.path().to_string_lossy(), file_info.name);
                        },
                        _ => {
                            record = format!("{}", entry.file_name().to_string_lossy());
                        }
                    }
                },
                "path" => {
                    match file_info {
                        &Some(ref file_info) => {
                            record = format!("[{}] {}", entry.path().to_string_lossy(), file_info.name);
                        },
                        _ => {
                            record = format!("{}", entry.path().to_string_lossy());
                        }
                    }
                },
                "size" => {
                    match file_info {
                        &Some(ref file_info) => {
                            record = format!("{}", file_info.size);
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                record = format!("{}", attrs.len());
                            }
                        }
                    }
                },
                "hsize" | "fsize" => {
                    match file_info {
                        &Some(ref file_info) => {
                            record = format!("{}", file_info.size.file_size(file_size_opts::BINARY).unwrap());
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                record = format!("{}", attrs.len().file_size(file_size_opts::BINARY).unwrap());
                            }
                        }
                    }
                },
                "is_dir" => {
                    match file_info {
                        &Some(ref file_info) => {
                            record = format!("{}", file_info.name.ends_with('/'));
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                record = format!("{}", attrs.is_dir());
                            }
                        }
                    }
                },
                "is_file" => {
                    match file_info {
                        &Some(ref file_info) => {
                            record = format!("{}", !file_info.name.ends_with('/'));
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                record = format!("{}", attrs.is_file());
                            }
                        }
                    }
                },
                "is_symlink" => {
                    match file_info {
                        &Some(_) => {
                            record = format!("{}", false);
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                record = format!("{}", attrs.file_type().is_symlink());
                            }
                        }
                    }
                },
                "mode" => {
                    match file_info {
                        &Some(ref file_info) => {
                            if let Some(mode) = file_info.mode {
                                record = format!("{}", mode::format_mode(mode));
                            }
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                record = format!("{}", mode::get_mode(attrs));
                            }
                        }
                    }
                },
                "user_read" => {
                    record = Self::print_file_mode(&attrs, &mode::user_read, &file_info, &mode::mode_user_read);
                },
                "user_write" => {
                    record = Self::print_file_mode(&attrs, &mode::user_write, &file_info, &mode::mode_user_write);
                },
                "user_exec" => {
                    record = Self::print_file_mode(&attrs, &mode::user_exec, &file_info, &mode::mode_user_exec);
                },
                "group_read" => {
                    record = Self::print_file_mode(&attrs, &mode::group_read, &file_info, &mode::mode_group_read);
                },
                "group_write" => {
                    record = Self::print_file_mode(&attrs, &mode::group_write, &file_info, &mode::mode_group_write);
                },
                "group_exec" => {
                    record = Self::print_file_mode(&attrs, &mode::group_exec, &file_info, &mode::mode_group_exec);
                },
                "other_read" => {
                    record = Self::print_file_mode(&attrs, &mode::other_read, &file_info, &mode::mode_other_read);
                },
                "other_write" => {
                    record = Self::print_file_mode(&attrs, &mode::other_write, &file_info, &mode::mode_other_write);
                },
                "other_exec" => {
                    record = Self::print_file_mode(&attrs, &mode::other_exec, &file_info, &mode::mode_other_exec);
                },
                "is_hidden" => {
                    match file_info {
                        &Some(ref file_info) => {
                            record = format!("{}", is_hidden(&file_info.name, &None, true));
                        },
                        _ => {
                            record = format!("{}", is_hidden(&entry.file_name().to_string_lossy(), &attrs, false));
                        }
                    }
                },
                "uid" => {
                    if let Some(ref attrs) = attrs {
                        if let Some(uid) = mode::get_uid(attrs) {
                            record = format!("{}", uid);
                        }
                    }
                },
                "gid" => {
                    if let Some(ref attrs) = attrs {
                        if let Some(gid) = mode::get_gid(attrs) {
                            record = format!("{}", gid);
                        }
                    }
                },
                "user" => {
                    if let Some(ref attrs) = attrs {
                        if let Some(uid) = mode::get_uid(attrs) {
                            if let Some(user) = self.user_cache.get_user_by_uid(uid) {
                                record = format!("{}", user.name());
                            }
                        }
                    }
                },
                "group" => {
                    if let Some(ref attrs) = attrs {
                        if let Some(gid) = mode::get_gid(attrs) {
                            if let Some(group) = self.user_cache.get_group_by_gid(gid) {
                                record = format!("{}", group.name());
                            }
                        }
                    }
                },
                "created" => {
                    if let Some(ref attrs) = attrs {
                        if let Ok(sdt) = attrs.created() {
                            let dt: DateTime<Local> = DateTime::from(sdt);
                            let format = dt.format("%Y-%m-%d %H:%M:%S");
                            record = format!("{}", format);
                        }
                    }
                },
                "accessed" => {
                    if let Some(ref attrs) = attrs {
                        if let Ok(sdt) = attrs.accessed() {
                            let dt: DateTime<Local> = DateTime::from(sdt);
                            let format = dt.format("%Y-%m-%d %H:%M:%S");
                            record = format!("{}", format);
                        }
                    }
                },
                "modified" => {
                    if let Some(ref attrs) = attrs {
                        if let Ok(sdt) = attrs.modified() {
                            let dt: DateTime<Local> = DateTime::from(sdt);
                            let format = dt.format("%Y-%m-%d %H:%M:%S");
                            record = format!("{}", format);
                        }
                    }
                },
                "width" => {
                    if let Some(ref dimensions) = dimensions {
                        record = format!("{}", dimensions.0);
                    }
                },
                "height" => {
                    if let Some(ref dimensions) = dimensions {
                        record = format!("{}", dimensions.1);
                    }
                },
                "is_archive" => {
                    let is_archive = is_archive(&entry.file_name().to_string_lossy());
                    record = format!("{}", is_archive);
                },
                "is_audio" => {
                    let is_audio = is_audio(&entry.file_name().to_string_lossy());
                    record = format!("{}", is_audio);
                },
                "is_doc" => {
                    let is_doc = is_doc(&entry.file_name().to_string_lossy());
                    record = format!("{}", is_doc);
                },
                "is_image" => {
                    let is_image = is_image(&entry.file_name().to_string_lossy());
                    record = format!("{}", is_image);
                },
                "is_source" => {
                    let is_source = is_source(&entry.file_name().to_string_lossy());
                    record = format!("{}", is_source);
                },
                "is_video" => {
                    let is_video = is_video(&entry.file_name().to_string_lossy());
                    record = format!("{}", is_video);
                },
                _ => {}
            };

            match self.query.output_format {
                OutputFormat::Lines => print!("{}\n", record),
                OutputFormat::List => print!("{}\0", record),
                OutputFormat::Tabs => print!("{}\t", record),
                OutputFormat::Csv => records.push(record),
                _ => print!("{}\t", record),
            }
        }

        match self.query.output_format {
            OutputFormat::Lines => {},
            OutputFormat::List => {
                print!("\0");
            },
            OutputFormat::Tabs => {
                print!("\n");
            },
            OutputFormat::Csv => {
                if let Some(ref mut csv_writer) = csv_writer {
                    let _ = csv_writer.write_record(records);
                }
            },
            _ => {
                print!("\n");
            },
        }
    }

    fn print_file_mode(attrs: &Option<Box<Metadata>>,
                       mode_func_boxed: &Fn(&Box<Metadata>) -> bool,
                       file_info: &Option<FileInfo>,
                       mode_func_i32: &Fn(u32) -> bool) -> String {
        match file_info {
            &Some(ref file_info) => {
                if let Some(mode) = file_info.mode {
                    return format!("{}", mode_func_i32(mode));
                }
            },
            _ => {
                if let &Some(ref attrs) = attrs {
                    return format!("{}", mode_func_boxed(attrs));
                }
            }
        }

        String::new()
    }

    fn conforms(&mut self,
                entry: &DirEntry,
                file_info: &Option<FileInfo>,
                expr: &Box<Expr>,
                entry_meta: Option<Box<fs::Metadata>>,
                entry_dim: Option<(usize, usize)>,
                follow_symlinks: bool) -> (bool, Option<Box<fs::Metadata>>, Option<(usize, usize)>) {
        let mut result = false;
        let mut meta = entry_meta;
        let mut dim = entry_dim;

        if let Some(ref logical_op) = expr.logical_op {
            let mut left_result = false;
            let mut right_result = false;

            if let Some(ref left) = expr.left {
                let (left_res, left_meta, left_dim) = self.conforms(entry, file_info, &left, meta, dim, follow_symlinks);
                left_result = left_res;
                meta = left_meta;
                dim = left_dim;
            }

            match logical_op {
                &LogicalOp::And => {
                    if !left_result {
                        result = false;
                    } else {
                        if let Some(ref right) = expr.right {
                            let (right_res, right_meta, right_dim) = self.conforms(entry, file_info, &right, meta, dim, follow_symlinks);
                            right_result = right_res;
                            meta = right_meta;
                            dim = right_dim;
                        }

                        result = left_result && right_result;
                    }
                },
                &LogicalOp::Or => {
                    if left_result {
                        result = true;
                    } else {
                        if let Some(ref right) = expr.right {
                            let (right_res, right_meta, right_dim) = self.conforms(entry, file_info, &right, meta, dim, follow_symlinks);
                            right_result = right_res;
                            meta = right_meta;
                            dim = right_dim;
                        }

                        result = left_result || right_result
                    }
                }
            }
        }

        if let Some(ref field) = expr.field {
            if field.to_ascii_lowercase() == "name" {
                match expr.val {
                    Some(ref val) => {
                        let file_name = match file_info {
                            &Some(ref file_info) => file_info.name.clone(),
                            _ => entry.file_name().into_string().unwrap()
                        };

                        result = match expr.op {
                            Some(Op::Eq) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(&file_name),
                                    None => val.eq(&file_name)
                                }
                            },
                            Some(Op::Ne) => {
                                match expr.regex {
                                    Some(ref regex) => !regex.is_match(&file_name),
                                    None => val.ne(&file_name)
                                }
                            },
                            Some(Op::Rx) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(&file_name),
                                    None => false
                                }
                            },
                            Some(Op::Eeq) => {
                                val.eq(&file_name)
                            },
                            Some(Op::Ene) => {
                                val.ne(&file_name)
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "path" {
                match expr.val {
                    Some(ref val) => {
                        let file_path = match file_info {
                            &Some(ref file_info) => file_info.name.clone(),
                            _ => String::from(entry.path().to_str().unwrap())
                        };

                        result = match expr.op {
                            Some(Op::Eq) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(&file_path),
                                    None => val.eq(&file_path)
                                }
                            },
                            Some(Op::Ne) => {
                                match expr.regex {
                                    Some(ref regex) => !regex.is_match(&file_path),
                                    None => val.ne(&file_path)
                                }
                            },
                            Some(Op::Rx) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(&file_path),
                                    None => false
                                }
                            },
                            Some(Op::Eeq) => {
                                val.eq(&file_path)
                            },
                            Some(Op::Ene) => {
                                val.ne(&file_path)
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "size" ||
                field.to_ascii_lowercase() == "hsize" ||
                field.to_ascii_lowercase() == "fsize" {
                match expr.val {
                    Some(ref val) => {
                        let file_size = match file_info {
                            &Some(ref file_info) => {
                                Some(file_info.size)
                            },
                            _ => {
                                meta = update_meta(entry, meta, follow_symlinks);

                                match meta {
                                    Some(ref metadata) => {
                                        Some(metadata.len())
                                    },
                                    _ => None
                                }
                            }
                        };

                        match file_size {
                            Some(file_size) => {
                                let size = parse_filesize(val);
                                match size {
                                    Some(size) => {
                                        result = match expr.op {
                                            Some(Op::Eq) | Some(Op::Eeq) => file_size == size,
                                            Some(Op::Ne) | Some(Op::Ene) => file_size != size,
                                            Some(Op::Gt) => file_size > size,
                                            Some(Op::Gte) => file_size >= size,
                                            Some(Op::Lt) => file_size < size,
                                            Some(Op::Lte) => file_size <= size,
                                            _ => false
                                        };
                                    },
                                    _ => { }
                                }
                            },
                            _ => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "uid" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref val) => {
                        meta = update_meta(entry, meta, follow_symlinks);

                        match meta {
                            Some(ref metadata) => {
                                let uid = val.parse::<u32>();
                                match uid {
                                    Ok(uid) => {
                                        let file_uid = mode::get_uid(metadata);
                                        match file_uid {
                                            Some(file_uid) => {
                                                result = match expr.op {
                                                    Some(Op::Eq) | Some(Op::Eeq) => file_uid == uid,
                                                    Some(Op::Ne) | Some(Op::Ene) => file_uid != uid,
                                                    Some(Op::Gt) => file_uid > uid,
                                                    Some(Op::Gte) => file_uid >= uid,
                                                    Some(Op::Lt) => file_uid < uid,
                                                    Some(Op::Lte) => file_uid <= uid,
                                                    _ => false
                                                };
                                            },
                                            None => { }
                                        }
                                    },
                                    _ => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "user" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref val) => {
                        meta = update_meta(entry, meta, follow_symlinks);

                        match meta {
                            Some(ref metadata) => {
                                let file_uid = mode::get_uid(metadata);
                                match file_uid {
                                    Some(file_uid) => {
                                        match self.user_cache.get_user_by_uid(file_uid) {
                                            Some(user) => {
                                                let user_name = user.name();
                                                result = match expr.op {
                                                    Some(Op::Eq) => {
                                                        match expr.regex {
                                                            Some(ref regex) => regex.is_match(user_name),
                                                            None => val.eq(user_name)
                                                        }
                                                    },
                                                    Some(Op::Ne) => {
                                                        match expr.regex {
                                                            Some(ref regex) => !regex.is_match(user_name),
                                                            None => val.ne(user_name)
                                                        }
                                                    },
                                                    Some(Op::Rx) => {
                                                        match expr.regex {
                                                            Some(ref regex) => regex.is_match(user_name),
                                                            None => false
                                                        }
                                                    },
                                                    Some(Op::Eeq) => {
                                                        val.eq(user_name)
                                                    },
                                                    Some(Op::Ene) => {
                                                        val.ne(user_name)
                                                    },
                                                    _ => false
                                                };
                                            },
                                            None => { }
                                        }
                                    },
                                    None => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "gid" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref val) => {
                        meta = update_meta(entry, meta, follow_symlinks);

                        match meta {
                            Some(ref metadata) => {
                                let gid = val.parse::<u32>();
                                match gid {
                                    Ok(gid) => {
                                        let file_gid = mode::get_gid(metadata);
                                        match file_gid {
                                            Some(file_gid) => {
                                                result = match expr.op {
                                                    Some(Op::Eq) | Some(Op::Eeq) => file_gid == gid,
                                                    Some(Op::Ne) | Some(Op::Ene) => file_gid != gid,
                                                    Some(Op::Gt) => file_gid > gid,
                                                    Some(Op::Gte) => file_gid >= gid,
                                                    Some(Op::Lt) => file_gid < gid,
                                                    Some(Op::Lte) => file_gid <= gid,
                                                    _ => false
                                                };
                                            },
                                            None => { }
                                        }
                                    },
                                    _ => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "group" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref val) => {
                        meta = update_meta(entry, meta, follow_symlinks);

                        match meta {
                            Some(ref metadata) => {
                                let file_gid = mode::get_gid(metadata);
                                match file_gid {
                                    Some(file_gid) => {
                                        match self.user_cache.get_group_by_gid(file_gid) {
                                            Some(group) => {
                                                let group_name = group.name();
                                                result = match expr.op {
                                                    Some(Op::Eq) => {
                                                        match expr.regex {
                                                            Some(ref regex) => regex.is_match(group_name),
                                                            None => val.eq(group_name)
                                                        }
                                                    },
                                                    Some(Op::Ne) => {
                                                        match expr.regex {
                                                            Some(ref regex) => !regex.is_match(group_name),
                                                            None => val.ne(group_name)
                                                        }
                                                    },
                                                    Some(Op::Rx) => {
                                                        match expr.regex {
                                                            Some(ref regex) => regex.is_match(group_name),
                                                            None => false
                                                        }
                                                    },
                                                    Some(Op::Eeq) => {
                                                        val.eq(group_name)
                                                    },
                                                    Some(Op::Ene) => {
                                                        val.ne(group_name)
                                                    },
                                                    _ => false
                                                };
                                            },
                                            None => { }
                                        }
                                    },
                                    None => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "is_dir" {
                if let Some(ref val) = expr.val {
                    let is_dir = match file_info {
                        &Some(ref file_info) => Some(file_info.name.ends_with('/')),
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => {
                                    Some(metadata.is_dir())
                                },
                                _ => None
                            }
                        }
                    };

                    if let Some(is_dir) = is_dir {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_dir
                                } else {
                                    !is_dir
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_dir
                                } else {
                                    is_dir
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "is_file" {
                if let Some(ref val) = expr.val {
                    let is_file = match file_info {
                        &Some(ref file_info) => Some(!file_info.name.ends_with('/')),
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => {
                                    Some(metadata.is_file())
                                },
                                _ => None
                            }
                        }
                    };

                    if let Some(is_file) = is_file {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_file
                                } else {
                                    !is_file
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_file
                                } else {
                                    is_file
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "is_symlink" {
                if let Some(ref val) = expr.val {
                    let is_symlink = match file_info {
                        &Some(_) => Some(false),
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => {
                                    Some(metadata.file_type().is_symlink())
                                },
                                _ => None
                            }
                        }
                    };

                    if let Some(is_symlink) = is_symlink {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_symlink
                                } else {
                                    !is_symlink
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_symlink
                                } else {
                                    is_symlink
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "mode" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => {
                            match file_info.mode {
                                Some(mode) => Some(mode::format_mode(mode)),
                                _ => None
                            }
                        },
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => {
                                    Some(mode::get_mode(metadata))
                                },
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        result = match expr.op {
                            Some(Op::Eq) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(&mode),
                                    None => val.eq(&mode)
                                }
                            },
                            Some(Op::Ne) => {
                                match expr.regex {
                                    Some(ref regex) => !regex.is_match(&mode),
                                    None => val.ne(&mode)
                                }
                            },
                            Some(Op::Rx) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(&mode),
                                    None => false
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "user_read" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_user_read(mode)
                                } else {
                                    !mode::mode_user_read(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_user_read(mode)
                                } else {
                                    mode::mode_user_read(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "user_write" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_user_write(mode)
                                } else {
                                    !mode::mode_user_write(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_user_write(mode)
                                } else {
                                    mode::mode_user_write(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "user_exec" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_user_exec(mode)
                                } else {
                                    !mode::mode_user_exec(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_user_exec(mode)
                                } else {
                                    mode::mode_user_exec(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "group_read" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_group_read(mode)
                                } else {
                                    !mode::mode_group_read(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_group_read(mode)
                                } else {
                                    mode::mode_group_read(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "group_write" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_group_write(mode)
                                } else {
                                    !mode::mode_group_write(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_group_write(mode)
                                } else {
                                    mode::mode_group_write(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "group_exec" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_group_exec(mode)
                                } else {
                                    !mode::mode_group_exec(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_group_exec(mode)
                                } else {
                                    mode::mode_group_exec(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "other_read" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_other_read(mode)
                                } else {
                                    !mode::mode_other_read(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_other_read(mode)
                                } else {
                                    mode::mode_other_read(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "other_write" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_other_write(mode)
                                } else {
                                    !mode::mode_other_write(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_other_write(mode)
                                } else {
                                    mode::mode_other_write(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "other_exec" {
                if let Some(ref val) = expr.val {
                    let mode = match file_info {
                        &Some(ref file_info) => file_info.mode,
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);

                            match meta {
                                Some(ref metadata) => mode::get_mode_from_boxed_unix_int(metadata),
                                _ => None
                            }
                        }
                    };

                    if let Some(mode) = mode {
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    mode::mode_other_exec(mode)
                                } else {
                                    !mode::mode_other_exec(mode)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !mode::mode_other_exec(mode)
                                } else {
                                    mode::mode_other_exec(mode)
                                }
                            },
                            _ => false
                        };
                    }
                }
            } else if field.to_ascii_lowercase() == "is_hidden" {
                match expr.val {
                    Some(ref val) => {
                        let is_hidden = match file_info {
                            &Some(ref file_info) => is_hidden(&file_info.name, &None, true),
                            _ => is_hidden(entry.file_name().to_str().unwrap(), &meta, false)
                        };

                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_hidden
                                } else {
                                    !is_hidden
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_hidden
                                } else {
                                    is_hidden
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "created" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref _val) => {
                        meta = update_meta(entry, meta, follow_symlinks);

                        match meta {
                            Some(ref metadata) => {
                                match metadata.created() {
                                    Ok(sdt) => {
                                        let dt: DateTime<Local> = DateTime::from(sdt);
                                        let start = expr.dt_from.unwrap();
                                        let finish = expr.dt_to.unwrap();

                                        result = match expr.op {
                                            Some(Op::Eq) => dt >= start && dt <= finish,
                                            Some(Op::Ne) => dt < start || dt > finish,
                                            Some(Op::Gt) => dt > finish,
                                            Some(Op::Gte) => dt >= start,
                                            Some(Op::Lt) => dt < start,
                                            Some(Op::Lte) => dt <= finish,
                                            _ => false
                                        };
                                    },
                                    _ => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "accessed" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref _val) => {
                        meta = update_meta(entry, meta, follow_symlinks);

                        match meta {
                            Some(ref metadata) => {
                                match metadata.accessed() {
                                    Ok(sdt) => {
                                        let dt: DateTime<Local> = DateTime::from(sdt);
                                        let start = expr.dt_from.unwrap();
                                        let finish = expr.dt_to.unwrap();

                                        result = match expr.op {
                                            Some(Op::Eq) => dt >= start && dt <= finish,
                                            Some(Op::Ne) => dt < start || dt > finish,
                                            Some(Op::Gt) => dt > finish,
                                            Some(Op::Gte) => dt >= start,
                                            Some(Op::Lt) => dt < start,
                                            Some(Op::Lte) => dt <= finish,
                                            _ => false
                                        };
                                    },
                                    _ => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "modified" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref _val) => {
                        meta = update_meta(entry, meta, follow_symlinks);

                        match meta {
                            Some(ref metadata) => {
                                match metadata.modified() {
                                    Ok(sdt) => {
                                        let dt: DateTime<Local> = DateTime::from(sdt);
                                        let start = expr.dt_from.unwrap();
                                        let finish = expr.dt_to.unwrap();

                                        result = match expr.op {
                                            Some(Op::Eq) => dt >= start && dt <= finish,
                                            Some(Op::Ne) => dt < start || dt > finish,
                                            Some(Op::Gt) => dt > finish,
                                            Some(Op::Gte) => dt >= start,
                                            Some(Op::Lt) => dt < start,
                                            Some(Op::Lte) => dt <= finish,
                                            _ => false
                                        };
                                    },
                                    _ => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "width" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                if !is_image_dim_readable(&entry.file_name().to_string_lossy()) {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref val) => {
                        if !dim.is_some() {
                            dim = match imagesize::size(entry.path()) {
                                Ok(dimensions) => Some((dimensions.width, dimensions.height)),
                                _ => None
                            };
                        }

                        match dim {
                            Some((width, _)) => {
                                let val = val.parse::<usize>();
                                match val {
                                    Ok(val) => {
                                        result = match expr.op {
                                            Some(Op::Eq) | Some(Op::Eeq) => width == val,
                                            Some(Op::Ne) | Some(Op::Ene) => width != val,
                                            Some(Op::Gt) => width > val,
                                            Some(Op::Gte) => width >= val,
                                            Some(Op::Lt) => width < val,
                                            Some(Op::Lte) => width <= val,
                                            _ => false
                                        };
                                    },
                                    _ => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "height" {
                if file_info.is_some() {
                    return (false, meta, dim)
                }

                if !is_image_dim_readable(&entry.file_name().to_string_lossy()) {
                    return (false, meta, dim)
                }

                match expr.val {
                    Some(ref val) => {
                        if !dim.is_some() {
                            dim = match imagesize::size(entry.path()) {
                                Ok(dimensions) => Some((dimensions.width, dimensions.height)),
                                _ => None
                            };
                        }

                        match dim {
                            Some((_, height)) => {
                                let val = val.parse::<usize>();
                                match val {
                                    Ok(val) => {
                                        result = match expr.op {
                                            Some(Op::Eq) | Some(Op::Eeq) => height == val,
                                            Some(Op::Ne) | Some(Op::Ene) => height != val,
                                            Some(Op::Gt) => height > val,
                                            Some(Op::Gte) => height >= val,
                                            Some(Op::Lt) => height < val,
                                            Some(Op::Lte) => height <= val,
                                            _ => false
                                        };
                                    },
                                    _ => { }
                                }
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "is_archive" {
                match expr.val {
                    Some(ref val) => {
                        let file_name = match file_info {
                            &Some(ref file_info) => file_info.name.clone(),
                            _ => String::from(entry.file_name().to_str().unwrap())
                        };

                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_archive(&file_name)
                                } else {
                                    !is_archive(&file_name)
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_archive(&file_name)
                                } else {
                                    is_archive(&file_name)
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "is_audio" {
                match expr.val {
                    Some(ref val) => {
                        let file_name = match file_info {
                            &Some(ref file_info) => file_info.name.clone(),
                            _ => String::from(entry.file_name().to_str().unwrap())
                        };

                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_audio(&file_name)
                                } else {
                                    !is_audio(&file_name)
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_audio(&file_name)
                                } else {
                                    is_audio(&file_name)
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "is_doc" {
                match expr.val {
                    Some(ref val) => {
                        let file_name = match file_info {
                            &Some(ref file_info) => file_info.name.clone(),
                            _ => String::from(entry.file_name().to_str().unwrap())
                        };

                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_doc(&file_name)
                                } else {
                                    !is_doc(&file_name)
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_doc(&file_name)
                                } else {
                                    is_doc(&file_name)
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "is_image" {
                match expr.val {
                    Some(ref val) => {
                        let file_name = match file_info {
                            &Some(ref file_info) => file_info.name.clone(),
                            _ => String::from(entry.file_name().to_str().unwrap())
                        };

                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_image(&file_name)
                                } else {
                                    !is_image(&file_name)
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_image(&file_name)
                                } else {
                                    is_image(&file_name)
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "is_source" {
                match expr.val {
                    Some(ref val) => {
                        let file_name = match file_info {
                            &Some(ref file_info) => file_info.name.clone(),
                            _ => String::from(entry.file_name().to_str().unwrap())
                        };

                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_source(&file_name)
                                } else {
                                    !is_source(&file_name)
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_source(&file_name)
                                } else {
                                    is_source(&file_name)
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "is_video" {
                match expr.val {
                    Some(ref val) => {
                        let file_name = match file_info {
                            &Some(ref file_info) => file_info.name.clone(),
                            _ => String::from(entry.file_name().to_str().unwrap())
                        };

                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) | Some(Op::Eeq) => {
                                if bool_val {
                                    is_video(&file_name)
                                } else {
                                    !is_video(&file_name)
                                }
                            },
                            Some(Op::Ne) | Some(Op::Ene) => {
                                if bool_val {
                                    !is_video(&file_name)
                                } else {
                                    is_video(&file_name)
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            }
        }

        (result, meta, dim)
    }
}

fn update_meta(entry: &DirEntry, meta: Option<Box<Metadata>>, follow_symlinks: bool) -> Option<Box<Metadata>> {
    if !meta.is_some() {
        let metadata = match follow_symlinks {
            false => symlink_metadata(entry.path()),
            true => fs::metadata(entry.path())
        };

        if let Ok(metadata) = metadata {
            return Some(Box::new(metadata));
        }
    }

    meta
}

fn parse_filesize(s: &str) -> Option<u64> {
    let string = s.to_string().to_ascii_lowercase();

    if string.ends_with("k") {
        match &string[..(s.len() - 1)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024),
            _ => return None
        }
    }

    if string.ends_with("kb") {
        match &string[..(s.len() - 2)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024),
            _ => return None
        }
    }

    if string.ends_with("kib") {
        match &string[..(s.len() - 3)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024),
            _ => return None
        }
    }

    if string.ends_with("m") {
        match &string[..(s.len() - 1)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("mb") {
        match &string[..(s.len() - 2)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("mib") {
        match &string[..(s.len() - 3)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("g") {
        match &string[..(s.len() - 1)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024 * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("gb") {
        match &string[..(s.len() - 2)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024 * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("gib") {
        match &string[..(s.len() - 3)].parse::<u64>() {
            &Ok(size) => return Some(size * 1024 * 1024 * 1024),
            _ => return None
        }
    }

    match string.parse::<u64>() {
        Ok(size) => return Some(size),
        _ => return None
    }
}

#[allow(unused)]
fn is_hidden(file_name: &str, metadata: &Option<Box<Metadata>>, archive_mode: bool) -> bool {
    if archive_mode {
        if !file_name.contains('\\') {
            return parse_unix_filename(file_name).starts_with('.');
        } else {
            return false;
        }
    }

    #[cfg(unix)]
    {
        return file_name.starts_with('.');
    }

    #[cfg(windows)]
    {
        if let &Some(ref metadata) = metadata {
            return mode::get_mode(metadata).contains("Hidden");
        }
    }

    #[cfg(not(unix))]
    {
        false
    }
}

fn parse_unix_filename(s: &str) -> &str {
    let last_slash = s.rfind('/');
    match last_slash {
        Some(idx) => &s[idx..],
        _ => s
    }
}

const ZIP_ARCHIVE: &'static [&'static str] = &[".zip", ".jar", ".war", ".ear" ];

fn is_zip_archive(file_name: &str) -> bool {
    has_extension(file_name, &ZIP_ARCHIVE)
}

const ARCHIVE: &'static [&'static str] = &[".7z", ".bzip2", ".gz", ".gzip", ".rar", ".tar", ".xz", ".zip"];

fn is_archive(file_name: &str) -> bool {
    has_extension(file_name, &ARCHIVE)
}

const AUDIO: &'static [&'static str] = &[".aac", ".aiff", ".amr", ".flac", ".gsm", ".m4a", ".m4b", ".m4p", ".mp3", ".ogg", ".wav", ".wma"];

fn is_audio(file_name: &str) -> bool {
    has_extension(file_name, &AUDIO)
}

const DOC: &'static [&'static str] = &[".accdb", ".doc", ".docx", ".dot", ".dotx", ".mdb", ".ods", ".odt", ".pdf", ".ppt", ".pptx", ".rtf", ".xls", ".xlt", ".xlsx", ".xps"];

fn is_doc(file_name: &str) -> bool {
    has_extension(file_name, &DOC)
}

const IMAGE: &'static [&'static str] = &[".bmp", ".gif", ".jpeg", ".jpg", ".png", ".tiff", ".webp"];

fn is_image(file_name: &str) -> bool {
    has_extension(file_name, &IMAGE)
}

const IMAGE_DIM: &'static [&'static str] = &[".bmp", ".gif", ".jpeg", ".jpg", ".png", ".webp"];

fn is_image_dim_readable(file_name: &str) -> bool {
    has_extension(file_name, &IMAGE_DIM)
}

const SOURCE: &'static [&'static str] = &[".asm", ".c", ".cpp", ".cs", ".java", ".js", ".h", ".hpp", ".pas", ".php", ".pl", ".pm", ".py", ".rb", ".rs", ".swift"];

fn is_source(file_name: &str) -> bool {
    has_extension(file_name, &SOURCE)
}

const VIDEO: &'static [&'static str] = &[".3gp", ".avi", ".flv", ".m4p", ".m4v", ".mkv", ".mov", ".mp4", ".mpeg", ".mpg", ".webm", ".wmv"];

fn is_video(file_name: &str) -> bool {
    has_extension(file_name, &VIDEO)
}

fn has_extension(file_name: &str, extensions: &[&str]) -> bool {
    let s = file_name.to_ascii_lowercase();

    for ext in extensions {
        if s.ends_with(ext) {
            return true
        }
    }

    false
}

fn error_message(p: &Path, e: io::Error, t: &mut Box<StdoutTerminal>) {
    t.fg(term::color::YELLOW).unwrap();
    eprint!("{}", p.to_string_lossy());
    t.reset().unwrap();

    eprint!(": ");

    t.fg(term::color::RED).unwrap();
    eprintln!("{}", e.description());
    t.reset().unwrap();
}

struct FileInfo {
    name: String,
    size: u64,
    mode: Option<u32>,
}

fn to_file_info(zipped_file: &zip::read::ZipFile) -> FileInfo {
    FileInfo {
        name: zipped_file.name().to_string(),
        size: zipped_file.size(),
        mode: zipped_file.unix_mode(),
    }
}

#[cfg(windows)]
use std;

#[cfg(windows)]
struct UsersCache;

#[cfg(windows)]
impl UsersCache {
    fn new() -> Self {
        UsersCache { }
    }

    fn get_user_by_uid(&self, _: u32) -> Option< std::sync::Arc<User>> {
        None
    }

    fn get_group_by_gid(&self, _: u32) -> Option< std::sync::Arc<Group>> {
        None
    }
}

#[cfg(windows)]
struct User;

#[cfg(windows)]
impl User {
    fn name(&self) -> &str {
        ""
    }
}

#[cfg(windows)]
struct Group;

#[cfg(windows)]
impl Group {
    fn name(&self) -> &str {
        ""
    }
}