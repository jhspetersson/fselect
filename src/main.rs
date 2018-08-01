extern crate chrono;
extern crate chrono_english;
extern crate csv;
extern crate humansize;
extern crate imagesize;
#[macro_use]
extern crate lazy_static;
extern crate mp3_metadata;
extern crate regex;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate term;
extern crate time;
#[cfg(unix)]
extern crate users;
#[cfg(unix)]
extern crate xattr;
extern crate zip;

use std::env;

use term::StdoutTerminal;

mod field;
mod fileinfo;
mod gitignore;
mod lexer;
mod mode;
mod parser;
mod searcher;
mod util;

use parser::Parser;
use searcher::Searcher;
use util::error_message;

fn main() {
    let mut t = term::stdout().unwrap();

    if env::args().len() == 1 {
        usage_info(&mut t);
        return;
    }

    let mut args: Vec<String> = env::args().collect();
    args.remove(0);

    let first_arg = args[0].to_ascii_lowercase();
    if first_arg.contains("help") || first_arg.contains("-h") || first_arg.contains("/?") {
        usage_info(&mut t);
        return;
    }

    let query = args.join(" ");

    let mut p = Parser::new();
    let query = p.parse(&query);

    match query {
        Ok(query) => {
            let mut searcher = Searcher::new(query);
            searcher.list_search_results(&mut t).unwrap()
        },
        Err(err) => error_message("query", &err, &mut t)
    }
}

fn usage_info(t: &mut Box<StdoutTerminal>) {
    print!("FSelect utility v");
    t.fg(term::color::BRIGHT_YELLOW).unwrap();
    println!("0.4.2");
    t.reset().unwrap();

    println!("Find files with SQL-like queries.");

    t.fg(term::color::BRIGHT_CYAN).unwrap();
    println!("https://github.com/jhspetersson/fselect");
    t.reset().unwrap();

    println!("Usage: fselect COLUMN[, COLUMN...] [from ROOT[, ROOT...]] [where EXPR] [order by COLUMNS] [limit N] [into FORMAT]");
}
