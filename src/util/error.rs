use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorLevel {
    Normal,
    Fatal,
}

#[derive(Debug, Clone)]
pub struct SearchError {
    pub description: String,
    pub error_level: ErrorLevel,
}

impl SearchError {
    pub fn normal(description: impl Into<String>) -> Self {
        SearchError { description: description.into(), error_level: ErrorLevel::Normal }
    }

    pub fn fatal(description: impl Into<String>) -> Self {
        SearchError { description: description.into(), error_level: ErrorLevel::Fatal }
    }

    pub fn is_fatal(&self) -> bool {
        self.error_level == ErrorLevel::Fatal
    }
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description)
    }
}

impl From<String> for SearchError {
    fn from(s: String) -> Self {
        SearchError::normal(s)
    }
}

impl From<io::Error> for SearchError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::BrokenPipe {
            SearchError::fatal(e.to_string())
        } else {
            SearchError::normal(e.to_string())
        }
    }
}

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