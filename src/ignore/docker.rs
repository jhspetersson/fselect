//! Handles .dockerignore parsing

use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::ops::Add;
use std::ops::Index;
use std::path::Path;
use std::sync::LazyLock;
use regex::Captures;
use regex::Error;
use regex::Regex;

use crate::util::error_exit;

#[derive(Clone, Debug)]
pub struct DockerignoreFilter {
    pub regex: Regex,
    pub negate: bool,
}

impl DockerignoreFilter {
    fn new(regex: Regex, negate: bool) -> DockerignoreFilter {
        DockerignoreFilter { regex, negate }
    }
}

pub fn search_upstream_dockerignore(
    dockerignore_filters: &mut Vec<DockerignoreFilter>,
    dir: &Path,
) {
    if let Ok(canonical_path) = crate::util::canonical_path(&dir.to_path_buf()) {
        let mut path = std::path::PathBuf::from(canonical_path);

        loop {
            let dockerignore_file = path.join(".dockerignore");

            if dockerignore_file.is_file() {
                update_dockerignore_filters(dockerignore_filters, &mut path);
                return;
            }

            let parent_found = path.pop();

            if !parent_found {
                return;
            }
        }
    }
}

fn update_dockerignore_filters(dockerignore_filters: &mut Vec<DockerignoreFilter>, path: &Path) {
    let dockerignore_file = path.join(".dockerignore");
    if dockerignore_file.is_file() {
        let regexes = parse_dockerignore(&dockerignore_file, &path);
        match regexes {
            Ok(ref regexes) => {
                dockerignore_filters.append(&mut regexes.clone());
            }
            Err(err) => {
                eprintln!("{}: {}", path.to_string_lossy(), err);
            }
        }
    }
}

pub fn matches_dockerignore_filter(
    dockerignore_filters: &Vec<DockerignoreFilter>,
    file_name: &str,
) -> bool {
    let mut matched = false;

    let file_name = file_name.to_string().replace("\\", "/").replace("//", "/");

    for dockerignore_filter in dockerignore_filters {
        let is_match = dockerignore_filter.regex.is_match(&file_name);

        if is_match && dockerignore_filter.negate {
            return false;
        }

        if is_match {
            matched = true;
        }
    }

    matched
}

fn parse_dockerignore(
    file_path: &Path,
    dir_path: &Path,
) -> Result<Vec<DockerignoreFilter>, String> {
    let mut result = vec![];
    let mut err = String::new();

    if let Ok(file) = File::open(file_path) {
        let reader = BufReader::new(file);
        reader
            .lines()
            .filter(|line| match line {
                Ok(line) => !line.trim().is_empty() && !line.starts_with("#"),
                _ => false,
            })
            .for_each(|line| {
                if err.is_empty() {
                    if let Ok(line) = line {
                        let pattern = convert_dockerignore_pattern(&line, dir_path);
                        match pattern {
                            Ok(pattern) => result.push(pattern),
                            Err(parse_err) => err = parse_err,
                        }
                    }
                }
            });
    };

    match err.is_empty() {
        true => Ok(result),
        false => Err(err),
    }
}

fn convert_dockerignore_pattern(
    pattern: &str,
    file_path: &Path,
) -> Result<DockerignoreFilter, String> {
    let mut pattern = String::from(pattern);

    let mut negate = false;
    if pattern.starts_with("!") {
        pattern = pattern.replace("!", "");
        negate = true;
    }

    match convert_dockerignore_glob(&pattern, file_path) {
        Ok(regex) => Ok(DockerignoreFilter::new(regex, negate)),
        _ => Err("Error creating regex while parsing .dockerignore glob: "
            .to_string()
            .add(&pattern)),
    }
}

static DOCKER_CONVERT_REPLACE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(\\*\\*|\\?|\\.|\\*)").unwrap()
});

fn convert_dockerignore_glob(glob: &str, file_path: &Path) -> Result<Regex, Error> {
    let mut pattern = DOCKER_CONVERT_REPLACE_REGEX
        .replace_all(glob, |c: &Captures| {
            match c.index(0) {
                "**" => ".*",
                "." => "\\.",
                "*" => "[^/]*",
                "?" => "[^/]",
                _ => error_exit(".dockerignore", "Error parsing pattern"),
            }
            .to_string()
        })
        .to_string();

    while pattern.starts_with("/") || pattern.starts_with("\\") {
        pattern.remove(0);
    }

    #[cfg(windows)]
    let path = file_path
        .to_string_lossy()
        .to_string()
        .replace("\\", "/")
        .replace("//", "/");

    #[cfg(not(windows))]
    let path = file_path.to_string_lossy().to_string();

    pattern = path.replace("\\", "\\\\").add("/([^/]+/)*").add(&pattern);

    Regex::new(&pattern)
}
