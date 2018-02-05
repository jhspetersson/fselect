extern crate chrono;
extern crate regex;
extern crate term;

use std::error::Error;
use std::env;
use std::fs;
use std::fs::DirEntry;
use std::path::Path;
use std::io;

use chrono::DateTime;
use chrono::Local;
use regex::Regex;
use term::StdoutTerminal;

mod lexer;
mod mode;
mod parser;

use parser::Query;
use parser::Expr;
use parser::LogicalOp;
use parser::Op;

fn main() {
    let mut t = term::stdout().unwrap();

    if env::args().len() == 1 {
        usage_info(&mut t);
        return;
    }

	let mut args: Vec<String> = env::args().collect();
	args.remove(0);
	let query = args.join(" ");

    let mut p = parser::Parser::new();
	let q = p.parse(&query);

    match q {
        Ok(q) => list_search_results(q, &mut t).unwrap(),
        Err(s) => panic!(s)
    }
}

fn usage_info(t: &mut Box<StdoutTerminal>) {
    print!("FSelect utility v");
    t.fg(term::color::BRIGHT_YELLOW).unwrap();
    println!("0.0.6");
    t.reset().unwrap();

    println!("Find files with SQL-like queries.");

    t.fg(term::color::BRIGHT_CYAN).unwrap();
    println!("https://github.com/jhspetersson/fselect");
    t.reset().unwrap();

    println!("Usage: fselect COLUMN[, COLUMN...] from ROOT [where EXPR]");
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

fn list_search_results(query: Query, t: &mut Box<StdoutTerminal>) -> io::Result<()> {
    let need_metadata = query.fields.iter()
        .filter(|s| s.as_str().ne("name")).count() > 0;

    for root in &query.roots {
        let root_dir = Path::new(&root.path);
        let max_depth = root.depth;
        let _result = visit_dirs(root_dir, &check_file, &query, need_metadata, max_depth, 1, t);
    }

	Ok(())
}

fn visit_dirs(dir: &Path, cb: &Fn(&DirEntry, &Query, bool), query: &Query, need_metadata: bool, max_depth: u32, depth: u32, t: &mut Box<StdoutTerminal>) -> io::Result<()> {
    if max_depth == 0 || (max_depth > 0 && depth <= max_depth) {
        let metadata = dir.metadata();
        match metadata {
            Ok(metadata) => {
                if metadata.is_dir() {
                    match fs::read_dir(dir) {
                        Ok(entry_list) => {
                            for entry in entry_list {
                                match entry {
                                    Ok(entry) => {
                                        let path = entry.path();
                                        cb(&entry, query, need_metadata);
                                        if path.is_dir() {
                                            let result = visit_dirs(&path, cb, query, need_metadata, max_depth, depth + 1, t);
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

fn check_file(entry: &DirEntry, query: &Query, need_metadata: bool) {
    let mut meta = None;
    if let Some(ref expr) = query.expr {
        let (result, entry_meta) = conforms(entry, expr, None);
        if !result {
            return
        }

        meta = entry_meta;
    }

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

    for field in query.fields.iter() {
        match field.as_str() {
            "name" => {
                println!("{}", entry.file_name().to_string_lossy())
            },
            "path" => {
                println!("{}", entry.path().to_string_lossy())
            },
            "is_dir" => {
                if let Some(ref attrs) = attrs {
                    println!("{}", attrs.is_dir());
                }
            },
            "is_file" => {
                if let Some(ref attrs) = attrs {
                    println!("{}", attrs.is_file());
                }
            },
            "mode" => {
                if let Some(ref attrs) = attrs {
                    println!("{}", mode::get_mode(attrs));
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
            "size" => {
                if let Some(ref attrs) = attrs {
                    println!("{}", attrs.len())
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

fn conforms(entry: &DirEntry, expr: &Box<Expr>, entry_meta: Option<Box<fs::Metadata>>) -> (bool, Option<Box<fs::Metadata>>) {
    let mut result = false;
    let mut meta = entry_meta;

    if let Some(ref logical_op) = expr.logical_op {
        let mut left_result = false;
        let mut right_result = false;

        if let Some(ref left) = expr.left {
            let (left_res, left_meta) = conforms(entry, &left, meta);
            left_result = left_res;
            meta = left_meta;
        }

        match logical_op {
            &LogicalOp::And => {
                if !left_result {
                    result = false;
                } else {
                    if let Some(ref right) = expr.right {
                        let (right_res, right_meta) = conforms(entry, &right, meta);
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
                        let (right_res, right_meta) = conforms(entry, &right, meta);
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
        } else if field.to_ascii_lowercase() == "size" {
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
                Some(ref val) => {
                    if !meta.is_some() {
                        let metadata = entry.metadata().unwrap();
                        meta = Some(Box::new(metadata));
                    }

                    match meta {
                        Some(ref metadata) => {
                            match metadata.created() {
                                Ok(sdt) => {
                                    let dt: DateTime<Local> = DateTime::from(sdt);
                                    match parse_datetime(val.as_str()) {
                                        Ok((start, finish)) => {
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
                Some(ref val) => {
                    if !meta.is_some() {
                        let metadata = entry.metadata().unwrap();
                        meta = Some(Box::new(metadata));
                    }

                    match meta {
                        Some(ref metadata) => {
                            match metadata.accessed() {
                                Ok(sdt) => {
                                    let dt: DateTime<Local> = DateTime::from(sdt);
                                    match parse_datetime(val.as_str()) {
                                        Ok((start, finish)) => {
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
                Some(ref val) => {
                    if !meta.is_some() {
                        let metadata = entry.metadata().unwrap();
                        meta = Some(Box::new(metadata));
                    }

                    match meta {
                        Some(ref metadata) => {
                            match metadata.modified() {
                                Ok(sdt) => {
                                    let dt: DateTime<Local> = DateTime::from(sdt);
                                    match parse_datetime(val.as_str()) {
                                        Ok((start, finish)) => {
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

fn parse_datetime(s: &str) -> Result<(DateTime<Local>, DateTime<Local>), &str> {
    use chrono::TimeZone;

    let regex = Regex::new("(\\d{4})-(\\d{1,2})-(\\d{1,2}) ?(\\d{1,2})?:?(\\d{1,2})?:?(\\d{1,2})?").unwrap();
    match regex.captures(s) {
        Some(cap) => {
            let year: i32 = cap[1].parse().unwrap();
            let month: u32 = cap[2].parse().unwrap();
            let day: u32 = cap[3].parse().unwrap();

            let hour_start: u32;
            let hour_finish: u32;
            match cap.get(4) {
                Some(val) => {
                    hour_start = val.as_str().parse().unwrap();
                    hour_finish = hour_start;
                },
                None => {
                    hour_start = 0;
                    hour_finish = 23;
                }
            }

            let min_start: u32;
            let min_finish: u32;
            match cap.get(5) {
                Some(val) => {
                    min_start = val.as_str().parse().unwrap();
                    min_finish = min_start;
                },
                None => {
                    min_start = 0;
                    min_finish = 23;
                }
            }

            let sec_start: u32;
            let sec_finish: u32;
            match cap.get(6) {
                Some(val) => {
                    sec_start = val.as_str().parse().unwrap();
                    sec_finish = min_start;
                },
                None => {
                    sec_start = 0;
                    sec_finish = 23;
                }
            }

            let date = Local.ymd(year, month, day);
            let start = date.and_hms(hour_start, min_start, sec_start);
            let finish = date.and_hms(hour_finish, min_finish, sec_finish);

            Ok((start, finish))
        },
        None => {
            Err("Error parsing date/time")
        }
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

    match string.parse::<u64>() {
        Ok(size) => return Some(size),
        _ => return None
    }
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