use std::io;
use std::path::Path;
use std::sync::Mutex;

static NO_ERRORS: Mutex<bool> = Mutex::new(false);

pub fn get_no_errors() -> bool {
    *NO_ERRORS.lock().unwrap()
}

pub fn set_no_errors(value: bool) {
    let mut no_errors = NO_ERRORS.lock().unwrap();
    *no_errors = value;
}

pub fn path_error_message(p: &Path, e: io::Error) {
    error_message(&p.to_string_lossy(), &e.to_string());
}

pub fn error_message(source: &str, description: &str) {
    let guard = NO_ERRORS.lock().unwrap();
    let no_errors = *guard;
    drop(guard);

    if !no_errors {
        eprintln!("{}: {}", source, description);
    }
}