use std::io;
use std::path::Path;

pub fn path_error_message(p: &Path, e: io::Error) {
    error_message(&p.to_string_lossy(), &e.to_string());
}

pub fn error_message(source: &str, description: &str) {
    eprintln!("{}: {}", source, description);
}