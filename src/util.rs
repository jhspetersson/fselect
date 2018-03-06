use std::error::Error;
use std::io;
use std::path::Path;

use term;
use term::StdoutTerminal;

pub fn path_error_message(p: &Path, e: io::Error, t: &mut Box<StdoutTerminal>) {
    error_message(&p.to_string_lossy(), e.description(), t);
}

pub fn error_message(source: &str, description: &str, t: &mut Box<StdoutTerminal>) {
    t.fg(term::color::YELLOW).unwrap();
    eprint!("{}", source);
    t.reset().unwrap();

    eprint!(": ");

    t.fg(term::color::RED).unwrap();
    eprintln!("{}", description);
    t.reset().unwrap();
}