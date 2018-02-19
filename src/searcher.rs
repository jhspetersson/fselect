use std::error::Error;
use std::fs;
use std::fs::DirEntry;
use std::path::Path;
use std::io;

use chrono::DateTime;
use chrono::Local;
use humansize::{FileSize, file_size_opts};
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

        for root in &self.query.clone().roots {
            let root_dir = Path::new(&root.path);
            let max_depth = root.depth;
            let search_archives = root.archives;
            let _result = self.visit_dirs(root_dir, need_metadata, max_depth, 1, search_archives, t);
        }

        Ok(())
    }

    fn visit_dirs(&mut self,
                  dir: &Path,
                  need_metadata: bool,
                  max_depth: u32,
                  depth: u32,
                  search_archives: bool,
                  t: &mut Box<StdoutTerminal>) -> io::Result<()> {
        if max_depth == 0 || (max_depth > 0 && depth <= max_depth) {
            let metadata = dir.metadata();
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

                                            self.check_file(&entry, &None, need_metadata);

                                            if search_archives && is_zip_archive(&path.to_string_lossy()) {
                                                if let Ok(file) = fs::File::open(&path) {
                                                    if let Ok(mut archive) = zip::ZipArchive::new(file) {
                                                        for i in 0..archive.len() {
                                                            if let Ok(afile) = archive.by_index(i) {
                                                                let file_info = to_file_info(&afile);
                                                                self.check_file(&entry, &Some(file_info), need_metadata);
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            if path.is_dir() {
                                                let result = self.visit_dirs(&path, need_metadata, max_depth, depth + 1, search_archives, t);
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

    fn check_file(&mut self, entry: &DirEntry, file_info: &Option<FileInfo>, need_metadata: bool) {
        let mut meta = None;
        if let Some(ref expr) = self.query.expr.clone() {
            let (result, entry_meta) = self.conforms(entry, file_info, expr, None);
            if !result {
                return
            }

            meta = entry_meta;
        }

        self.found += 1;

        let attrs = match need_metadata {
            true =>  {
                if meta.is_some() {
                    meta
                } else {
                    let result = fs::metadata(entry.path());
                    match result {
                        Ok(meta) => Some(Box::new(meta)),
                        _ => None
                    }
                }
            },
            false => None
        };

        for field in self.query.fields.iter() {
            match field.as_str() {
                "name" => {
                    match file_info {
                        &Some(ref file_info) => {
                            println!("[{}] {}", entry.path().to_string_lossy(), file_info.name)
                        },
                        _ => {
                            println!("{}", entry.file_name().to_string_lossy())
                        }
                    }
                },
                "path" => {
                    match file_info {
                        &Some(ref file_info) => {
                            println!("[{}] {}", entry.path().to_string_lossy(), file_info.name)
                        },
                        _ => {
                            println!("{}", entry.path().to_string_lossy())
                        }
                    }
                },
                "size" => {
                    match file_info {
                        &Some(ref file_info) => {
                            println!("{}", file_info.size)
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                println!("{}", attrs.len());
                            }
                        }
                    }
                },
                "hsize" | "fsize" => {
                    match file_info {
                        &Some(ref file_info) => {
                            println!("{}", file_info.size.file_size(file_size_opts::BINARY).unwrap())
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                println!("{}", attrs.len().file_size(file_size_opts::BINARY).unwrap());
                            }
                        }
                    }
                },
                "is_dir" => {
                    match file_info {
                        &Some(ref file_info) => {
                            //TODO
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                println!("{}", attrs.is_dir());
                            }
                        }
                    }
                },
                "is_file" => {
                    match file_info {
                        &Some(ref file_info) => {
                            //TODO
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                println!("{}", attrs.is_file());
                            }
                        }
                    }
                },
                "mode" => {
                    match file_info {
                        &Some(ref file_info) => {
                            if let Some(mode) = file_info.mode {
                                println!("{}", mode::format_mode(mode));
                            }
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                println!("{}", mode::get_mode(attrs));
                            }
                        }
                    }
                },
                "user_read" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::user_read(attrs));
                    }
                },
                "user_write" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::user_write(attrs));
                    }
                },
                "user_exec" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::user_exec(attrs));
                    }
                },
                "group_read" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::group_read(attrs));
                    }
                },
                "group_write" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::group_write(attrs));
                    }
                },
                "group_exec" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::group_exec(attrs));
                    }
                },
                "other_read" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::other_read(attrs));
                    }
                },
                "other_write" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::other_write(attrs));
                    }
                },
                "other_exec" => {
                    if let Some(ref attrs) = attrs {
                        println!("{}", mode::other_exec(attrs));
                    }
                },
                "uid" => {
                    if let Some(ref attrs) = attrs {
                        match mode::get_uid(attrs) {
                            Some(uid) => println!("{}", uid),
                            None => { }
                        }
                    }
                },
                "gid" => {
                    if let Some(ref attrs) = attrs {
                        match mode::get_gid(attrs) {
                            Some(gid) => println!("{}", gid),
                            None => { }
                        }
                    }
                },
                "user" => {
                    if let Some(ref attrs) = attrs {
                        match mode::get_uid(attrs) {
                            Some(uid) => {
                                match self.user_cache.get_user_by_uid(uid) {
                                    Some(user) => {
                                        println!("{}", user.name());
                                    },
                                    None => { }
                                }
                            },
                            None => { }
                        }
                    }
                },
                "group" => {
                    if let Some(ref attrs) = attrs {
                        match mode::get_gid(attrs) {
                            Some(gid) => {
                                match self.user_cache.get_group_by_gid(gid) {
                                    Some(group) => {
                                        println!("{}", group.name());
                                    },
                                    None => { }
                                }
                            },
                            None => { }
                        }
                    }
                },
                "created" => {
                    if let Some(ref attrs) = attrs {
                        match attrs.created() {
                            Ok(sdt) => {
                                let dt: DateTime<Local> = DateTime::from(sdt);
                                let format = dt.format("%Y-%m-%d %H:%M:%S");
                                println!("{}", format);
                            },
                            _ => { }
                        }
                    }
                },
                "accessed" => {
                    if let Some(ref attrs) = attrs {
                        match attrs.accessed() {
                            Ok(sdt) => {
                                let dt: DateTime<Local> = DateTime::from(sdt);
                                let format = dt.format("%Y-%m-%d %H:%M:%S");
                                println!("{}", format);
                            },
                            _ => { }
                        }
                    }
                },
                "modified" => {
                    if let Some(ref attrs) = attrs {
                        match attrs.modified() {
                            Ok(sdt) => {
                                let dt: DateTime<Local> = DateTime::from(sdt);
                                let format = dt.format("%Y-%m-%d %H:%M:%S");
                                println!("{}", format);
                            },
                            _ => { }
                        }
                    }
                },
                "is_archive" => {
                    let is_archive = is_archive(&entry.file_name().to_string_lossy());
                    println!("{}", is_archive);
                },
                "is_audio" => {
                    let is_audio = is_audio(&entry.file_name().to_string_lossy());
                    println!("{}", is_audio);
                },
                "is_doc" => {
                    let is_doc = is_doc(&entry.file_name().to_string_lossy());
                    println!("{}", is_doc);
                },
                "is_image" => {
                    let is_image = is_image(&entry.file_name().to_string_lossy());
                    println!("{}", is_image);
                },
                "is_video" => {
                    let is_video = is_video(&entry.file_name().to_string_lossy());
                    println!("{}", is_video);
                },
                _ => {

                }
            }
        }
    }

    fn conforms(&mut self,
                entry: &DirEntry,
                file_info: &Option<FileInfo>,
                expr: &Box<Expr>,
                entry_meta: Option<Box<fs::Metadata>>) -> (bool, Option<Box<fs::Metadata>>) {
        let mut result = false;
        let mut meta = entry_meta;

        if let Some(ref logical_op) = expr.logical_op {
            let mut left_result = false;
            let mut right_result = false;

            if let Some(ref left) = expr.left {
                let (left_res, left_meta) = self.conforms(entry, file_info, &left, meta);
                left_result = left_res;
                meta = left_meta;
            }

            match logical_op {
                &LogicalOp::And => {
                    if !left_result {
                        result = false;
                    } else {
                        if let Some(ref right) = expr.right {
                            let (right_res, right_meta) = self.conforms(entry, file_info, &right, meta);
                            right_result = right_res;
                            meta = right_meta;
                        }

                        result = left_result && right_result;
                    }
                },
                &LogicalOp::Or => {
                    if left_result {
                        result = true;
                    } else {
                        if let Some(ref right) = expr.right {
                            let (right_res, right_meta) = self.conforms(entry, file_info, &right, meta);
                            right_result = right_res;
                            meta = right_meta;
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
                        let file_name = &entry.file_name().into_string().unwrap();
                        result = match expr.op {
                            Some(Op::Eq) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(file_name),
                                    None => val.eq(file_name)
                                }
                            },
                            Some(Op::Ne) => {
                                match expr.regex {
                                    Some(ref regex) => !regex.is_match(file_name),
                                    None => val.ne(file_name)
                                }
                            },
                            Some(Op::Rx) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(file_name),
                                    None => false
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "path" {
                match expr.val {
                    Some(ref val) => {
                        let file_path = &String::from(entry.path().to_str().unwrap());
                        result = match expr.op {
                            Some(Op::Eq) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(file_path),
                                    None => val.eq(file_path)
                                }
                            },
                            Some(Op::Ne) => {
                                match expr.regex {
                                    Some(ref regex) => !regex.is_match(file_path),
                                    None => val.ne(file_path)
                                }
                            },
                            Some(Op::Rx) => {
                                match expr.regex {
                                    Some(ref regex) => regex.is_match(file_path),
                                    None => false
                                }
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
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let file_size = metadata.len();

                                let size = parse_filesize(val);
                                match size {
                                    Some(size) => {
                                        result = match expr.op {
                                            Some(Op::Eq) => file_size == size,
                                            Some(Op::Ne) => file_size != size,
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
                            None => {

                            }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "uid" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let uid = val.parse::<u32>();
                                match uid {
                                    Ok(uid) => {
                                        let file_uid = mode::get_uid(metadata);
                                        match file_uid {
                                            Some(file_uid) => {
                                                result = match expr.op {
                                                    Some(Op::Eq) => file_uid == uid,
                                                    Some(Op::Ne) => file_uid != uid,
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
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

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
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let gid = val.parse::<u32>();
                                match gid {
                                    Ok(gid) => {
                                        let file_gid = mode::get_gid(metadata);
                                        match file_gid {
                                            Some(file_gid) => {
                                                result = match expr.op {
                                                    Some(Op::Eq) => file_gid == gid,
                                                    Some(Op::Ne) => file_gid != gid,
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
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

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
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            metadata.is_dir()
                                        } else {
                                            !metadata.is_dir()
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !metadata.is_dir()
                                        } else {
                                            metadata.is_dir()
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => {

                            }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "is_file" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            metadata.is_file()
                                        } else {
                                            !metadata.is_file()
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !metadata.is_file()
                                        } else {
                                            metadata.is_file()
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => {

                            }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "mode" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let mode = mode::get_mode(metadata);

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
                            },
                            None => {}
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "user_read" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::user_read(metadata)
                                        } else {
                                            !mode::user_read(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::user_read(metadata)
                                        } else {
                                            mode::user_read(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "user_write" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::user_write(metadata)
                                        } else {
                                            !mode::user_write(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::user_write(metadata)
                                        } else {
                                            mode::user_write(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "user_exec" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::user_exec(metadata)
                                        } else {
                                            !mode::user_exec(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::user_read(metadata)
                                        } else {
                                            mode::user_read(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "group_read" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::group_read(metadata)
                                        } else {
                                            !mode::group_read(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::group_read(metadata)
                                        } else {
                                            mode::group_read(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "group_write" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::group_write(metadata)
                                        } else {
                                            !mode::group_write(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::group_write(metadata)
                                        } else {
                                            mode::group_write(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "group_exec" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::group_exec(metadata)
                                        } else {
                                            !mode::group_exec(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::group_exec(metadata)
                                        } else {
                                            mode::group_exec(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "other_read" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::other_read(metadata)
                                        } else {
                                            !mode::other_read(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::other_read(metadata)
                                        } else {
                                            mode::other_read(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "other_write" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::other_write(metadata)
                                        } else {
                                            !mode::other_write(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::other_write(metadata)
                                        } else {
                                            mode::other_write(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "other_exec" {
                match expr.val {
                    Some(ref val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

                        match meta {
                            Some(ref metadata) => {
                                let str_val = val.to_ascii_lowercase();
                                let bool_val = str_val.eq("true") || str_val.eq("1");

                                result = match expr.op {
                                    Some(Op::Eq) => {
                                        if bool_val {
                                            mode::other_exec(metadata)
                                        } else {
                                            !mode::other_exec(metadata)
                                        }
                                    },
                                    Some(Op::Ne) => {
                                        if bool_val {
                                            !mode::other_exec(metadata)
                                        } else {
                                            mode::other_exec(metadata)
                                        }
                                    },
                                    _ => false
                                };
                            },
                            None => { }
                        }
                    },
                    None => { }
                }
            } else if field.to_ascii_lowercase() == "created" {
                match expr.val {
                    Some(ref _val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

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
                match expr.val {
                    Some(ref _val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

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
                match expr.val {
                    Some(ref _val) => {
                        if !meta.is_some() {
                            let metadata = entry.metadata().unwrap();
                            meta = Some(Box::new(metadata));
                        }

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
            } else if field.to_ascii_lowercase() == "is_archive" {
                match expr.val {
                    Some(ref val) => {
                        let file_name = &entry.file_name().into_string().unwrap();
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    is_archive(file_name)
                                } else {
                                    !is_archive(file_name)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !is_archive(file_name)
                                } else {
                                    is_archive(file_name)
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
                        let file_name = &entry.file_name().into_string().unwrap();
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    is_audio(file_name)
                                } else {
                                    !is_audio(file_name)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !is_audio(file_name)
                                } else {
                                    is_audio(file_name)
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
                        let file_name = &entry.file_name().into_string().unwrap();
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    is_doc(file_name)
                                } else {
                                    !is_doc(file_name)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !is_doc(file_name)
                                } else {
                                    is_doc(file_name)
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
                        let file_name = &entry.file_name().into_string().unwrap();
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    is_image(file_name)
                                } else {
                                    !is_image(file_name)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !is_image(file_name)
                                } else {
                                    is_image(file_name)
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
                        let file_name = &entry.file_name().into_string().unwrap();
                        let str_val = val.to_ascii_lowercase();
                        let bool_val = str_val.eq("true") || str_val.eq("1");

                        result = match expr.op {
                            Some(Op::Eq) => {
                                if bool_val {
                                    is_video(file_name)
                                } else {
                                    !is_video(file_name)
                                }
                            },
                            Some(Op::Ne) => {
                                if bool_val {
                                    !is_video(file_name)
                                } else {
                                    is_video(file_name)
                                }
                            },
                            _ => false
                        };
                    },
                    None => { }
                }
            }
        }

        (result, meta)
    }
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

const ZIP_ARCHIVE: &'static [&'static str] = &[".zip", ".jar", ".war", ".ear" ];

fn is_zip_archive(file_name: &str) -> bool {
    has_extension(file_name, &ZIP_ARCHIVE)
}

const ARCHIVE: &'static [&'static str] = &[".7zip", ".bzip2", ".gz", ".gzip", ".rar", ".tar", ".xz", ".zip"];

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

const IMAGE: &'static [&'static str] = &[".bmp", ".gif", ".jpeg", ".jpg", ".png", ".tiff"];

fn is_image(file_name: &str) -> bool {
    has_extension(file_name, &IMAGE)
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