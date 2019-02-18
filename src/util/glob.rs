use std::ops::Add;
use std::ops::Index;
use std::path::Path;

use regex::Captures;
use regex::Error;
use regex::Regex;

pub fn convert_glob_to_regex(glob: &str, file_path: &Path) -> Result<Regex, Error> {
    #[cfg(not(windows))]
        {
            let replace_regex = Regex::new("(\\*\\*|\\?|\\.|\\*)").unwrap();
            let mut pattern = replace_regex.replace_all(&glob, |c: &Captures| {
                match c.index(0) {
                    "**" => ".*",
                    "." => "\\.",
                    "*" => "[^/]*",
                    "?" => "[^/]+",
                    _ => panic!("Error parsing pattern")
                }.to_string()
            }).to_string();

            pattern = file_path.to_string_lossy().to_string()
                .replace("\\", "\\\\")
                .add("/([^/]+/)*").add(&pattern);

            Regex::new(&pattern)
        }

    #[cfg(windows)]
        {
            let replace_regex = Regex::new("(\\*\\*|\\?|\\.|\\*)").unwrap();
            let mut pattern = replace_regex.replace_all(&glob, |c: &Captures| {
                match c.index(0) {
                    "**" => ".*",
                    "." => "\\.",
                    "*" => "[^\\\\]*",
                    "?" => "[^\\\\]+",
                    _ => panic!("Error parsing pattern")
                }.to_string()
            }).to_string();

            pattern = file_path.to_string_lossy().to_string()
                .replace("\\", "\\\\")
                .add("\\\\([^\\\\]+\\\\)*").add(&pattern);

            Regex::new(&pattern)
        }
}