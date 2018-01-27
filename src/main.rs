extern crate regex;
extern crate term;

use std::env;
use std::fs;
use std::fs::DirEntry;
use std::path::Path;
use std::io;

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
    println!("FSelect utility v0.0.1");
    println!("Usage: fselect COLUMN[, COLUMN] from ROOT [where EXPR]");
}

fn list_search_results(query: Query, t: &mut Box<StdoutTerminal>) -> io::Result<()> {
    let need_metadata = query.fields.iter()
        .filter(|s| s.as_str().ne("name")).count() > 0;

    let root = Path::new(&query.root);

    visit_dirs(root, &check_file, &query, need_metadata);

	t.reset().unwrap();	
	
	Ok(())
}

fn visit_dirs(dir: &Path, cb: &Fn(&DirEntry, &Query, bool), query: &Query, need_metadata: bool) -> io::Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit_dirs(&path, cb, query, need_metadata)?;
            } else {
                cb(&entry, query, need_metadata);
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
            }
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
        }
    }

    (result, meta)
}