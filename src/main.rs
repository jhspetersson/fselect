extern crate chrono;
extern crate regex;
extern crate term;

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
mod parser;

use parser::Query;
use parser::Expr;
use parser::LogicalOp;
use parser::Op;

fn main() {
    if env::args().len() == 1 {
        usage_info();
        return;
    }

	let mut args: Vec<String> = env::args().collect();
	args.remove(0);
	let query = args.join(" ");

    let mut t = term::stdout().unwrap();

    let mut p = parser::Parser::new();
	let q = p.parse(&query);

    match q {
        Ok(q) => list_search_results(q, &mut t).unwrap(),
        Err(s) => panic!(s)
    }
}

fn usage_info() {
    println!("FSelect utility v0.0.4");
    println!("Find files with SQL-like queries.");
    println!("https://github.com/jhspetersson/fselect");
    println!("Usage: fselect COLUMN[, COLUMN...] from ROOT [where EXPR]");
}

fn list_search_results(query: Query, t: &mut Box<StdoutTerminal>) -> io::Result<()> {
    let need_metadata = query.fields.iter()
        .filter(|s| s.as_str().ne("name")).count() > 0;

    for root in &query.roots {
        let root_dir = Path::new(&root.path);
        let max_depth = root.depth;
        visit_dirs(root_dir, &check_file, &query, need_metadata, max_depth, 1);
    }

	t.reset().unwrap();	
	
	Ok(())
}

fn visit_dirs(dir: &Path, cb: &Fn(&DirEntry, &Query, bool), query: &Query, need_metadata: bool, max_depth: u32, depth: u32) -> io::Result<()> {
    if max_depth == 0 || (max_depth > 0 && depth <= max_depth) {
        if dir.is_dir() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    visit_dirs(&path, cb, query, need_metadata, max_depth, depth + 1)?;
                } else {
                    cb(&entry, query, need_metadata);
                }
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

                            let size: Result<u64, _> = val.parse();
                            match size {
                                Ok(size) => {
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