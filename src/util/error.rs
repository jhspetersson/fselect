use std::io;
use std::path::Path;

pub fn path_error_message(p: &Path, e: io::Error) {
    error_message(&p.to_string_lossy(), &e.to_string());
}

pub fn error_message(source: &str, description: &str) {
    eprintln!("{}: {}", source, description);
}

pub fn error_exit(source: &str, description: &str) -> ! {
    error_message(source, description);
    std::process::exit(2);
}