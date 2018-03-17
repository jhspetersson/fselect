use std::collections::HashMap;
use std::fs;
use std::fs::DirEntry;
#[cfg(unix)]
use std::fs::File;
use std::fs::Metadata;
use std::fs::symlink_metadata;
use std::path::Path;
use std::io;
use std::process;

use chrono::DateTime;
use chrono::Local;
use chrono::TimeZone;
use csv;
use humansize::{FileSize, file_size_opts};
use imagesize;
use mp3_metadata;
use mp3_metadata::MP3Metadata;
use serde_json;
use term::StdoutTerminal;
use time::Tm;
#[cfg(unix)]
use users::{Groups, Users, UsersCache};
#[cfg(unix)]
use xattr::FileExt;
use zip;

use mode;
use parser::Query;
use parser::Expr;
use parser::LogicalOp;
use parser::Op;
use parser::OutputFormat;
use util::*;

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

        let need_mp3 = self.query.fields.iter()
            .filter(|s| {
                let str = s.as_str();
                str.eq("bitrate") || str.eq("freq") ||
                    str.eq("title") || str.eq("artist") ||
                    str.eq("album") || str.eq("year") || str.eq("genre")
            }).count() > 0;

        if let OutputFormat::Json = self.query.output_format {
            print!("[");
        }

        for root in &self.query.clone().roots {
            let root_dir = Path::new(&root.path);
            let max_depth = root.depth;
            let search_archives = root.archives;
            let follow_symlinks = root.symlinks;
            let _result = self.visit_dirs(
                root_dir,
                need_metadata,
                need_dim,
                need_mp3,
                max_depth,
                1,
                search_archives,
                follow_symlinks,
                t
            );
        }

        if let OutputFormat::Json = self.query.output_format {
            print!("]");
        }

        Ok(())
    }

    fn visit_dirs(&mut self,
                  dir: &Path,
                  need_metadata: bool,
                  need_dim: bool,
                  need_mp3: bool,
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

                                            self.check_file(&entry, &None, need_metadata, need_dim, need_mp3, follow_symlinks, t);

                                            if search_archives && is_zip_archive(&path.to_string_lossy()) {
                                                if let Ok(file) = fs::File::open(&path) {
                                                    if let Ok(mut archive) = zip::ZipArchive::new(file) {
                                                        for i in 0..archive.len() {
                                                            if self.query.limit > 0 && self.query.limit <= self.found {
                                                                break;
                                                            }

                                                            if let Ok(afile) = archive.by_index(i) {
                                                                let file_info = to_file_info(&afile);
                                                                self.check_file(&entry, &Some(file_info), need_metadata, need_dim, need_mp3, false, t);
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            if path.is_dir() {
                                                let result = self.visit_dirs(&path, need_metadata, need_dim, need_mp3, max_depth, depth + 1, search_archives, follow_symlinks, t);
                                                if result.is_err() {
                                                    path_error_message(&path, result.err().unwrap(), t);
                                                }
                                            }
                                        },
                                        Err(err) => {
                                            path_error_message(dir, err, t);
                                        }
                                    }
                                }
                            },
                            Err(err) => {
                                path_error_message(dir, err, t);
                            }
                        }
                    }
                },
                Err(err) => {
                    path_error_message(dir, err, t);
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
                  need_mp3: bool,
                  follow_symlinks: bool,
                  t: &mut Box<StdoutTerminal>) {
        let mut meta = None;
        let mut dim = None;
        let mut mp3 = None;

        if let Some(ref expr) = self.query.expr.clone() {
            let (result, entry_meta, entry_dim, entry_mp3) = self.conforms(entry, file_info, expr, None, None, None, follow_symlinks);
            if !result {
                return
            }

            meta = entry_meta;
            dim = entry_dim;
            mp3 = entry_mp3;
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

        let mp3_info = match need_mp3 {
            true => {
                if mp3.is_some() {
                    mp3
                } else {
                    match mp3_metadata::read_from_file(entry.path()) {
                        Ok(mp3_meta) => Some(mp3_meta),
                        _ => None
                    }
                }
            },
            false => None
        };

        let mut csv_writer = None;
        let mut records = vec![];
        let mut file_map = HashMap::new();
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
                    match file_info {
                        &Some(ref file_info) => {
                            let dt: DateTime<Local> = to_local_datetime(&file_info.modified);
                            let format = dt.format("%Y-%m-%d %H:%M:%S");
                            record = format!("{}", format);
                        },
                        _ => {
                            if let Some(ref attrs) = attrs {
                                if let Ok(sdt) = attrs.modified() {
                                    let dt: DateTime<Local> = DateTime::from(sdt);
                                    let format = dt.format("%Y-%m-%d %H:%M:%S");
                                    record = format!("{}", format);
                                }
                            }
                        }
                    }
                },
                "has_xattr" => {
                    #[cfg(unix)]
                    {
                        if let Ok(file) = File::open(entry) {
                            if let Ok(xattrs) = file.list_xattr() {
                                let has_xattr = xattrs.count() > 0;
                            }
                        }
                    }

                    #[cfg(not(unix))]
                    {
                        record = format!("{}", false);
                    }
                }
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
                "bitrate" => {
                    if let Some(ref mp3_info) = mp3_info {
                        record = format!("{}", mp3_info.frames[0].bitrate);
                    }
                },
                "freq" => {
                    if let Some(ref mp3_info) = mp3_info {
                        record = format!("{}", mp3_info.frames[0].sampling_freq);
                    }
                },
                "title" => {
                    if let Some(ref mp3_info) = mp3_info {
                        if let Some(ref mp3_tag) = mp3_info.tag {
                            record = format!("{}", mp3_tag.title);
                        }
                    }
                },
                "artist" => {
                    if let Some(ref mp3_info) = mp3_info {
                        if let Some(ref mp3_tag) = mp3_info.tag {
                            record = format!("{}", mp3_tag.artist);
                        }
                    }
                },
                "album" => {
                    if let Some(ref mp3_info) = mp3_info {
                        if let Some(ref mp3_tag) = mp3_info.tag {
                            record = format!("{}", mp3_tag.album);
                        }
                    }
                },
                "year" => {
                    if let Some(ref mp3_info) = mp3_info {
                        if let Some(ref mp3_tag) = mp3_info.tag {
                            record = format!("{}", mp3_tag.year);
                        }
                    }
                },
                "genre" => {
                    if let Some(ref mp3_info) = mp3_info {
                        if let Some(ref mp3_tag) = mp3_info.tag {
                            record = format!("{:?}", mp3_tag.genre);
                        }
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
                unknown_field => {
                    error_message(unknown_field, "unknown search field", t);
                    process::exit(1);
                }
            };

            match self.query.output_format {
                OutputFormat::Lines => {
                    print!("{}\n", record);
                },
                OutputFormat::List => {
                    print!("{}\0", record);
                },
                OutputFormat::Json => {
                    file_map.insert(field, record);
                },
                OutputFormat::Tabs => {
                    print!("{}\t", record);
                },
                OutputFormat::Csv => {
                    records.push(record);
                },
            }
        }

        match self.query.output_format {
            OutputFormat::Lines | OutputFormat::List => {},
            OutputFormat::Tabs => {
                print!("\n");
            },
            OutputFormat::Csv => {
                if let Some(ref mut csv_writer) = csv_writer {
                    let _ = csv_writer.write_record(records);
                }
            },
            OutputFormat::Json => {
                if self.found > 1 {
                    print!(",");
                }

                print!("{}", serde_json::to_string(&file_map).unwrap());
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
                entry_mp3: Option<MP3Metadata>,
                follow_symlinks: bool) -> (bool, Option<Box<fs::Metadata>>, Option<(usize, usize)>, Option<MP3Metadata>) {
        let mut result = false;
        let mut meta = entry_meta;
        let mut dim = entry_dim;
        let mut mp3 = entry_mp3;

        if let Some(ref logical_op) = expr.logical_op {
            let mut left_result = false;
            let mut right_result = false;

            if let Some(ref left) = expr.left {
                let (left_res, left_meta, left_dim, left_mp3) = self.conforms(entry, file_info, &left, meta, dim, mp3, follow_symlinks);
                left_result = left_res;
                meta = left_meta;
                dim = left_dim;
                mp3 = left_mp3;
            }

            match logical_op {
                &LogicalOp::And => {
                    if !left_result {
                        result = false;
                    } else {
                        if let Some(ref right) = expr.right {
                            let (right_res, right_meta, right_dim, right_mp3) = self.conforms(entry, file_info, &right, meta, dim, mp3, follow_symlinks);
                            right_result = right_res;
                            meta = right_meta;
                            dim = right_dim;
                            mp3 = right_mp3;
                        }

                        result = left_result && right_result;
                    }
                },
                &LogicalOp::Or => {
                    if left_result {
                        result = true;
                    } else {
                        if let Some(ref right) = expr.right {
                            let (right_res, right_meta, right_dim, right_mp3) = self.conforms(entry, file_info, &right, meta, dim, mp3, follow_symlinks);
                            right_result = right_res;
                            meta = right_meta;
                            dim = right_dim;
                            mp3 = right_mp3;
                        }

                        result = left_result || right_result
                    }
                }
            }
        }

        if let Some(ref field) = expr.field {
            if field.to_ascii_lowercase() == "name" {
                if let Some(ref val) = expr.val {
                    let file_name = match file_info {
                        &Some(ref file_info) => file_info.name.clone(),
                        _ => entry.file_name().to_string_lossy().to_string()
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
                        Some(Op::Rx) | Some(Op::Like) => {
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
                }
            } else if field.to_ascii_lowercase() == "path" {
                if let Some(ref val) = expr.val {
                    let file_path = match file_info {
                        &Some(ref file_info) => file_info.name.clone(),
                        _ => String::from(entry.path().to_string_lossy())
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
                        Some(Op::Rx) | Some(Op::Like) => {
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
                }
            } else if field.to_ascii_lowercase() == "size" ||
                field.to_ascii_lowercase() == "hsize" ||
                field.to_ascii_lowercase() == "fsize" {
                if let Some(ref val) = expr.val {
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

                    if let Some(file_size) = file_size {
                        let size = parse_filesize(val);
                        if let Some(size) = size {
                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => file_size == size,
                                Some(Op::Ne) | Some(Op::Ene) => file_size != size,
                                Some(Op::Gt) => file_size > size,
                                Some(Op::Gte) => file_size >= size,
                                Some(Op::Lt) => file_size < size,
                                Some(Op::Lte) => file_size <= size,
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "uid" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    meta = update_meta(entry, meta, follow_symlinks);

                    if let Some(ref metadata) = meta {
                        let uid = val.parse::<u32>();
                        if let Ok(uid) = uid {
                            let file_uid = mode::get_uid(metadata);
                            if let Some(file_uid) = file_uid {
                                result = match expr.op {
                                    Some(Op::Eq) | Some(Op::Eeq) => file_uid == uid,
                                    Some(Op::Ne) | Some(Op::Ene) => file_uid != uid,
                                    Some(Op::Gt) => file_uid > uid,
                                    Some(Op::Gte) => file_uid >= uid,
                                    Some(Op::Lt) => file_uid < uid,
                                    Some(Op::Lte) => file_uid <= uid,
                                    _ => false
                                };
                            }
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "user" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    meta = update_meta(entry, meta, follow_symlinks);

                    if let Some(ref metadata) = meta {
                        let file_uid = mode::get_uid(metadata);
                        if let Some(file_uid) = file_uid {
                            if let Some(user) = self.user_cache.get_user_by_uid(file_uid) {
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
                                    Some(Op::Rx) | Some(Op::Like) => {
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
                            }
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "gid" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    meta = update_meta(entry, meta, follow_symlinks);

                    if let Some(ref metadata) = meta {
                        let gid = val.parse::<u32>();
                        if let Ok(gid) = gid {
                            let file_gid = mode::get_gid(metadata);
                            if let Some(file_gid) = file_gid {
                                result = match expr.op {
                                    Some(Op::Eq) | Some(Op::Eeq) => file_gid == gid,
                                    Some(Op::Ne) | Some(Op::Ene) => file_gid != gid,
                                    Some(Op::Gt) => file_gid > gid,
                                    Some(Op::Gte) => file_gid >= gid,
                                    Some(Op::Lt) => file_gid < gid,
                                    Some(Op::Lte) => file_gid <= gid,
                                    _ => false
                                };
                            }
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "group" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    meta = update_meta(entry, meta, follow_symlinks);

                    if let Some(ref metadata) = meta {
                        let file_gid = mode::get_gid(metadata);
                        if let Some(file_gid) = file_gid {
                            if let Some(group) = self.user_cache.get_group_by_gid(file_gid) {
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
                                    Some(Op::Rx) | Some(Op::Like) => {
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
                            }
                        }
                    }
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
                        let bool_val = str_to_bool(val);

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
                        let bool_val = str_to_bool(val);

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
                        let bool_val = str_to_bool(val);

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
                            Some(Op::Rx) | Some(Op::Like) => {
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
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_user_read);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "user_write" {
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_user_write);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "user_exec" {
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_user_exec);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "group_read" {
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_group_read);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "group_write" {
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_group_write);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "group_exec" {
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_group_exec);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "other_read" {
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_other_read);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "other_write" {
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_other_write);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "other_exec" {
                let (res_, meta_) = confirm_file_mode(&expr.op, &expr.val, &entry, meta, &file_info, follow_symlinks, &mode::mode_other_exec);
                meta = meta_;
                result = res_;
            } else if field.to_ascii_lowercase() == "is_hidden" {
                if let Some(ref val) = expr.val {
                    let is_hidden = match file_info {
                        &Some(ref file_info) => is_hidden(&file_info.name, &None, true),
                        _ => is_hidden(&entry.file_name().to_string_lossy(), &meta, false)
                    };

                    let bool_val = str_to_bool(val);

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
                }
            } else if field.to_ascii_lowercase() == "created" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref _val) = expr.val {
                    meta = update_meta(entry, meta, follow_symlinks);

                    if let Some(ref metadata) = meta {
                        if let Ok(sdt) = metadata.created() {
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
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "accessed" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref _val) = expr.val {
                    meta = update_meta(entry, meta, follow_symlinks);

                    if let Some(ref metadata) = meta {
                        if let Ok(sdt) = metadata.accessed() {
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
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "modified" {
                if let Some(ref _val) = expr.val {
                    let dt = match file_info {
                        &Some(ref file_info) => Some(to_local_datetime(&file_info.modified)),
                        _ => {
                            meta = update_meta(entry, meta, follow_symlinks);
                            match meta {
                                Some(ref metadata) => {
                                    match metadata.modified() {
                                        Ok(sdt) => Some(DateTime::from(sdt)),
                                        _ => None
                                    }
                                },
                                _ => None
                            }
                        }
                    };

                    if let Some(dt) = dt {
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
                    }
                }
            } else if field.to_ascii_lowercase() == "has_xattr" {
                #[cfg(unix)]
                {
                    if file_info.is_some() {
                        return (false, meta, dim, mp3)
                    }

                    if let Some(ref val) = expr.val {
                        if let Ok(file) = File::open(entry) {
                            if let Ok(xattrs) = file.list_xattr() {
                                let has_xattr = xattrs.count() > 0;
                                let bool_val = str_to_bool(val);

                                result = match &expr.op {
                                    &Some(Op::Eq) | &Some(Op::Eeq) => {
                                        if bool_val {
                                            has_xattr
                                        } else {
                                            !has_xattr
                                        }
                                    },
                                    &Some(Op::Ne) | &Some(Op::Ene) => {
                                        if bool_val {
                                            !has_xattr
                                        } else {
                                            has_xattr
                                        }
                                    },
                                    _ => false
                                };
                            }
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "width" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if !is_image_dim_readable(&entry.file_name().to_string_lossy()) {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    if !dim.is_some() {
                        dim = match imagesize::size(entry.path()) {
                            Ok(dimensions) => Some((dimensions.width, dimensions.height)),
                            _ => None
                        };
                    }

                    if let Some((width, _)) = dim {
                        let val = val.parse::<usize>();
                        if let Ok(val) = val {
                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => width == val,
                                Some(Op::Ne) | Some(Op::Ene) => width != val,
                                Some(Op::Gt) => width > val,
                                Some(Op::Gte) => width >= val,
                                Some(Op::Lt) => width < val,
                                Some(Op::Lte) => width <= val,
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "height" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if !is_image_dim_readable(&entry.file_name().to_string_lossy()) {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    if !dim.is_some() {
                        dim = match imagesize::size(entry.path()) {
                            Ok(dimensions) => Some((dimensions.width, dimensions.height)),
                            _ => None
                        };
                    }

                    if let Some((_, height)) = dim {
                        let val = val.parse::<usize>();
                        if let Ok(val) = val {
                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => height == val,
                                Some(Op::Ne) | Some(Op::Ene) => height != val,
                                Some(Op::Gt) => height > val,
                                Some(Op::Gte) => height >= val,
                                Some(Op::Lt) => height < val,
                                Some(Op::Lte) => height <= val,
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "bitrate" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    mp3 = update_mp3_meta(&entry, mp3);

                    if let Some(ref mp3_meta) = mp3 {
                        let val = val.parse::<usize>();
                        if let Ok(val) = val {
                            let bitrate = mp3_meta.frames[0].bitrate as usize;
                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => bitrate == val,
                                Some(Op::Ne) | Some(Op::Ene) => bitrate != val,
                                Some(Op::Gt) => bitrate > val,
                                Some(Op::Gte) => bitrate >= val,
                                Some(Op::Lt) => bitrate < val,
                                Some(Op::Lte) => bitrate <= val,
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "freq" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    mp3 = update_mp3_meta(&entry, mp3);

                    if let Some(ref mp3_meta) = mp3 {
                        let val = val.parse::<usize>();
                        if let Ok(val) = val {
                            let freq = mp3_meta.frames[0].sampling_freq as usize;
                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => freq == val,
                                Some(Op::Ne) | Some(Op::Ene) => freq != val,
                                Some(Op::Gt) => freq > val,
                                Some(Op::Gte) => freq >= val,
                                Some(Op::Lt) => freq < val,
                                Some(Op::Lte) => freq <= val,
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "title" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    mp3 = update_mp3_meta(&entry, mp3);

                    if let Some(ref mp3_meta) = mp3 {
                        if let Some(ref mp3_tag) = mp3_meta.tag {
                            let title = &mp3_tag.title;
                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => {
                                    match expr.regex {
                                        Some(ref regex) => regex.is_match(title),
                                        None => val.eq(title)
                                    }
                                },
                                Some(Op::Ne) | Some(Op::Ene) => {
                                    match expr.regex {
                                        Some(ref regex) => !regex.is_match(title),
                                        None => val.ne(title)
                                    }
                                },
                                Some(Op::Rx) | Some(Op::Like) => {
                                    match expr.regex {
                                        Some(ref regex) => regex.is_match(title),
                                        None => false
                                    }
                                },
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "artist" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    mp3 = update_mp3_meta(&entry, mp3);

                    if let Some(ref mp3_meta) = mp3 {
                        if let Some(ref mp3_tag) = mp3_meta.tag {
                            let artist = &mp3_tag.artist;

                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => {
                                    match expr.regex {
                                        Some(ref regex) => regex.is_match(artist),
                                        None => val.eq(artist)
                                    }
                                },
                                Some(Op::Ne) | Some(Op::Ene) => {
                                    match expr.regex {
                                        Some(ref regex) => !regex.is_match(artist),
                                        None => val.ne(artist)
                                    }
                                },
                                Some(Op::Rx) | Some(Op::Like) => {
                                    match expr.regex {
                                        Some(ref regex) => regex.is_match(artist),
                                        None => false
                                    }
                                },
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "album" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                        mp3 = update_mp3_meta(&entry, mp3);

                    if let Some(ref mp3_meta) = mp3 {
                        if let Some(ref mp3_tag) = mp3_meta.tag {
                            let album = &mp3_tag.album;

                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => {
                                    match expr.regex {
                                        Some(ref regex) => regex.is_match(album),
                                        None => val.eq(album)
                                    }
                                },
                                Some(Op::Ne) | Some(Op::Ene) => {
                                    match expr.regex {
                                        Some(ref regex) => !regex.is_match(album),
                                        None => val.ne(album)
                                    }
                                },
                                Some(Op::Rx) | Some(Op::Like) => {
                                    match expr.regex {
                                        Some(ref regex) => regex.is_match(album),
                                        None => false
                                    }
                                },
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "year" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    mp3 = update_mp3_meta(&entry, mp3);

                    if let Some(ref mp3_meta) = mp3 {
                        let val = val.parse::<usize>();
                        if let Ok(val) = val {
                            if let Some(ref mp3_tag) = mp3_meta.tag {
                                let year = mp3_tag.year as usize;
                                if year > 0 {
                                    result = match expr.op {
                                        Some(Op::Eq) | Some(Op::Eeq) => year == val,
                                        Some(Op::Ne) | Some(Op::Ene) => year != val,
                                        Some(Op::Gt) => year > val,
                                        Some(Op::Gte) => year >= val,
                                        Some(Op::Lt) => year < val,
                                        Some(Op::Lte) => year <= val,
                                        _ => false
                                    };
                                }
                            }
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "genre" {
                if file_info.is_some() {
                    return (false, meta, dim, mp3)
                }

                if let Some(ref val) = expr.val {
                    mp3 = update_mp3_meta(&entry, mp3);

                    if let Some(ref mp3_meta) = mp3 {
                        if let Some(ref mp3_tag) = mp3_meta.tag {
                            let genre = &format!("{:?}", &mp3_tag.genre);

                            result = match expr.op {
                                Some(Op::Eq) | Some(Op::Eeq) => {
                                    match expr.regex {
                                        Some(ref regex) => regex.is_match(genre),
                                        None => val.eq(genre)
                                    }
                                },
                                Some(Op::Ne) | Some(Op::Ene) => {
                                    match expr.regex {
                                        Some(ref regex) => !regex.is_match(genre),
                                        None => val.ne(genre)
                                    }
                                },
                                Some(Op::Rx) | Some(Op::Like) => {
                                    match expr.regex {
                                        Some(ref regex) => regex.is_match(genre),
                                        None => false
                                    }
                                },
                                _ => false
                            };
                        }
                    }
                }
            } else if field.to_ascii_lowercase() == "is_archive" {
                result = confirm_file_ext(&expr.op, &expr.val, &entry, &file_info, &is_archive);
            } else if field.to_ascii_lowercase() == "is_audio" {
                result = confirm_file_ext(&expr.op, &expr.val, &entry, &file_info, &is_audio);
            } else if field.to_ascii_lowercase() == "is_doc" {
                result = confirm_file_ext(&expr.op, &expr.val, &entry, &file_info, &is_doc);
            } else if field.to_ascii_lowercase() == "is_image" {
                result = confirm_file_ext(&expr.op, &expr.val, &entry, &file_info, &is_image);
            } else if field.to_ascii_lowercase() == "is_source" {
                result = confirm_file_ext(&expr.op, &expr.val, &entry, &file_info, &is_source);
            } else if field.to_ascii_lowercase() == "is_video" {
                result = confirm_file_ext(&expr.op, &expr.val, &entry, &file_info, &is_video);
            }
        }

        (result, meta, dim, mp3)
    }
}

fn confirm_file_mode(expr_op: &Option<Op>,
                     expr_val: &Option<String>,
                     entry: &DirEntry,
                     meta: Option<Box<Metadata>>,
                     file_info: &Option<FileInfo>,
                     follow_symlinks: bool,
                     mode_func: &Fn(u32) -> bool) -> (bool, Option<Box<Metadata>>) {
    let mut result = false;
    let mut meta = meta;

    if let &Some(ref val) = expr_val {
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
            let bool_val = str_to_bool(val);

            result = match expr_op {
                &Some(Op::Eq) => {
                    if bool_val {
                        mode_func(mode)
                    } else {
                        !mode_func(mode)
                    }
                },
                &Some(Op::Ne) => {
                    if bool_val {
                        !mode_func(mode)
                    } else {
                        mode_func(mode)
                    }
                },
                _ => false
            };
        }
    }

    (result, meta)
}

fn confirm_file_ext(expr_op: &Option<Op>,
                    expr_val: &Option<String>,
                    entry: &DirEntry,
                    file_info: &Option<FileInfo>,
                    file_ext_func: &Fn(&str) -> bool) -> bool {
    let mut result = false;

    if let &Some(ref val) = expr_val {
        let file_name = match file_info {
            &Some(ref file_info) => file_info.name.clone(),
            _ => String::from(entry.file_name().to_string_lossy())
        };

        let bool_val = str_to_bool(val);

        result = match expr_op {
            &Some(Op::Eq) | &Some(Op::Eeq) => {
                if bool_val {
                    file_ext_func(&file_name)
                } else {
                    !file_ext_func(&file_name)
                }
            },
            &Some(Op::Ne) | &Some(Op::Ene) => {
                if bool_val {
                    !file_ext_func(&file_name)
                } else {
                    file_ext_func(&file_name)
                }
            },
            _ => false
        };
    }

    result
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

fn update_mp3_meta(entry: &DirEntry, mp3: Option<MP3Metadata>) -> Option<MP3Metadata> {
    match mp3 {
        None => {
            match mp3_metadata::read_from_file(entry.path()) {
                Ok(mp3_meta) => Some(mp3_meta),
                _ => None
            }
        },
        Some(mp3_) => Some(mp3_)
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

fn str_to_bool(val: &str) -> bool {
    let str_val = val.to_ascii_lowercase();
    str_val.eq("true") || str_val.eq("1")
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

struct FileInfo {
    name: String,
    size: u64,
    mode: Option<u32>,
    modified: Tm,
}

fn to_file_info(zipped_file: &zip::read::ZipFile) -> FileInfo {
    FileInfo {
        name: zipped_file.name().to_string(),
        size: zipped_file.size(),
        mode: zipped_file.unix_mode(),
        modified: zipped_file.last_modified()
    }
}

fn to_local_datetime(tm: &Tm) -> DateTime<Local> {
    Local.ymd(tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
        .and_hms(tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32)
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