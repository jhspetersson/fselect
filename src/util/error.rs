use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use nu_ansi_term::Color::Yellow;

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorLevel {
    Normal,
    Fatal,
}

#[derive(Debug, Clone)]
pub struct SearchError {
    pub source: String,
    pub description: String,
    pub error_level: ErrorLevel,
}

impl SearchError {
    pub fn normal(description: impl Into<String>) -> Self {
        SearchError { source: String::new(), description: description.into(), error_level: ErrorLevel::Normal }
    }

    pub fn fatal(description: impl Into<String>) -> Self {
        SearchError { source: String::new(), description: description.into(), error_level: ErrorLevel::Fatal }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    pub fn is_fatal(&self) -> bool {
        self.error_level == ErrorLevel::Fatal
    }

    pub fn print(&self) {
        error_message(&self.source, &self.description);
    }
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.source.is_empty() {
            write!(f, "{}", self.description)
        } else {
            write!(f, "{}: {}", self.source, self.description)
        }
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
static USE_COLORS: Mutex<bool> = Mutex::new(false);

pub fn get_no_errors() -> bool {
    *NO_ERRORS.lock().unwrap()
}

pub fn set_no_errors(value: bool) {
    let mut no_errors = NO_ERRORS.lock().unwrap();
    *no_errors = value;
}

pub fn set_use_colors(value: bool) {
    let mut use_colors = USE_COLORS.lock().unwrap();
    *use_colors = value;
}

pub fn path_error_message(p: &Path, e: io::Error) {
    error_message(&p.to_string_lossy(), &e.to_string());
}

pub fn error_message(source: &str, description: &str) {
    let guard = NO_ERRORS.lock().unwrap();
    let no_errors = *guard;
    drop(guard);

    if !no_errors {
        let guard = USE_COLORS.lock().unwrap();
        let use_colors = *guard;
        drop(guard);

        if use_colors {
            eprintln!("{}: {}", Yellow.paint(source), description);
        } else {
            eprintln!("{}: {}", source, description);
        }
    }
}
