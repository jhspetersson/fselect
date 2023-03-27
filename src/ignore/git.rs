use std::collections::HashMap;
use std::fs::File;
use std::ops::Add;
use std::ops::Index;
use std::path::{Path, PathBuf};

use regex::Captures;
use regex::Error;
use regex::Regex;

#[derive(Clone, Debug)]
pub struct GitignoreFilter {
    pub regex: Regex,
    pub only_dir: bool,
    pub negate: bool,
}

impl GitignoreFilter {
    fn new(regex: Regex, only_dir: bool, negate: bool) -> GitignoreFilter {
        GitignoreFilter {
            regex, only_dir, negate
        }
    }
}

pub fn search_upstream_gitignore(gitignore_map: &mut HashMap<PathBuf, Vec<GitignoreFilter>>, dir: &Path) {
    if let Ok(canonical_path) = crate::util::canonical_path(&dir.to_path_buf()) {
        let mut path = PathBuf::from(canonical_path);

        loop {
            let parent_found = path.pop();

            if !parent_found {
                return;
            }

            update_gitignore_map(gitignore_map, &mut path);
        }
    }
}

pub fn update_gitignore_map(gitignore_map: &mut HashMap<PathBuf, Vec<GitignoreFilter>>, path: &Path) {
    let gitignore_file = path.join(".gitignore");
    if gitignore_file.is_file() {
        let regexes = parse_gitignore(&gitignore_file, &path);
        gitignore_map.insert(path.to_path_buf(), regexes);
    }
}

pub fn get_gitignore_filters(gitignore_map: &mut HashMap<PathBuf, Vec<GitignoreFilter>>, dir: &Path) -> Vec<GitignoreFilter> {
    let mut result = vec![];

    for (dir_path, regexes) in &mut *gitignore_map {
        if dir.to_path_buf() == *dir_path {
            for ref mut rx in regexes {
                result.push(rx.clone());
            }

            return result;
        }
    }

    let mut path = dir.to_path_buf();

    loop {
        let parent_found = path.pop();

        if !parent_found {
            return result;
        }

        for (dir_path, regexes) in &mut *gitignore_map {
            if path == *dir_path {
                let mut tmp = vec![];
                for ref mut rx in regexes {
                    tmp.push(rx.clone());
                }
                tmp.append(&mut result);
                result.clear();
                result.append(&mut tmp);
            }
        }
    }
}

pub fn matches_gitignore_filter(gitignore_filters: &Option<Vec<GitignoreFilter>>, file_name: &str, is_dir: bool) -> bool {
    match gitignore_filters {
        Some(gitignore_filters) => {
            let mut matched = false;

            for gitignore_filter in gitignore_filters {
                if gitignore_filter.only_dir && !is_dir {
                    continue;
                }

                let file_name_prepared = convert_file_name_for_matcher(file_name);
                let is_match = gitignore_filter.regex.is_match(&file_name_prepared);

                if is_match && gitignore_filter.negate {
                    return false;
                }

                if is_match {
                    matched = true;
                }
            }

            matched
        },
        _ => false
    }
}

fn convert_file_name_for_matcher(file_name: &str) -> String {
    #[cfg(windows)]
    {
        return String::from(file_name).replace("\\", "/");
    }

    #[cfg(not(windows))]
    {
        return String::from(file_name);
    }
}

fn parse_gitignore(file_path: &Path, dir_path: &Path) -> Vec<GitignoreFilter> {
    let mut result = vec![];

    let git_dir = dir_path.join(".git");
    if git_dir.is_dir() {
        let info_dir = git_dir.join("info");
        if info_dir.is_dir() {
            let exclude_file = info_dir.join("exclude");
            if exclude_file.exists() {
                result.append(&mut parse_file(&exclude_file, dir_path));
            }
        }
    }

    result.append(&mut convert_gitignore_pattern(".git/", dir_path));

    result.append(&mut parse_file(file_path, dir_path));

    result
}

fn parse_file(file_path: &Path, dir_path: &Path) -> Vec<GitignoreFilter> {
    let mut result = vec![];

    if let Ok(file) = File::open(file_path) {
        use std::io::BufRead;
        use std::io::BufReader;
        let reader = BufReader::new(file);
        reader.lines()
            .filter(|line| {
                match line {
                    Ok(line) => !line.trim().is_empty() && !line.starts_with("#"),
                    _ => false
                }
            })
            .for_each(|line| {
                match line {
                    Ok(line) => result.append(&mut convert_gitignore_pattern(&line, dir_path)),
                    _ => { }
                }
            });
    }

    result
}

fn convert_gitignore_pattern(pattern: &str, file_path: &Path) -> Vec<GitignoreFilter> {
    let mut result = vec![];

    let mut pattern = String::from(pattern);

    let mut negate = false;
    if pattern.starts_with("!") {
        pattern = pattern.replace("!", "");
        negate = true;
    }

    if pattern.ends_with("/") {
        pattern.pop();

        let regex = convert_gitignore_glob(&pattern, file_path);
        if regex.is_ok() {
            result.push(GitignoreFilter::new(regex.unwrap(), true, negate));
        }

        pattern = pattern.add("/**");
    }

    let regex = convert_gitignore_glob(&pattern, file_path);
    if regex.is_ok() {
        result.push(GitignoreFilter::new(regex.unwrap(), false, negate))
    }

    result
}

lazy_static! {
    static ref GIT_CONVERT_REPLACE_REGEX: Regex = Regex::new("(\\*\\*|\\?|\\.|\\*)").unwrap();
}

fn convert_gitignore_glob(glob: &str, file_path: &Path) -> Result<Regex, Error> {
    let mut pattern = GIT_CONVERT_REPLACE_REGEX.replace_all(&glob, |c: &Captures| {
        match c.index(0) {
            "**" => ".*",
            "." => "\\.",
            "*" => "[^/]*",
            "?" => "[^/]+",
            _ => panic!("Error parsing pattern")
        }.to_string()
    }).to_string();

    while pattern.starts_with("/") || pattern.starts_with("\\") {
        pattern.remove(0);
    }

    pattern = file_path.to_string_lossy().to_string()
        .replace("\\", "\\\\")
        .add("/([^/]+/)*").add(&pattern);

    #[cfg(windows)]
    {
        pattern = pattern.replace("\\", "/").replace("//", "/");
    }

    Regex::new(&pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    // *nix

    #[test]
    #[cfg(not(windows))]
    fn test_simple_pattern() {
        let file_path = Path::new("/home/user/projects/testprj");
        let glob = "foo";

        let result = convert_gitignore_pattern(glob, file_path);

        assert_eq!(result.len(), 1);

        let filter = &result[0];

        assert_eq!(filter.regex.as_str(), "/home/user/projects/testprj/([^/]+/)*foo");
        assert_eq!(filter.only_dir, false);
        assert_eq!(filter.negate, false);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_dir_pattern() {
        let file_path = Path::new("/home/user/projects/testprj");
        let glob = "foo/";

        let result = convert_gitignore_pattern(glob, file_path);

        assert_eq!(result.len(), 2);

        let filter = &result[0];

        assert_eq!(filter.regex.as_str(), "/home/user/projects/testprj/([^/]+/)*foo");
        assert_eq!(filter.only_dir, true);
        assert_eq!(filter.negate, false);

        let filter = &result[1];

        assert_eq!(filter.regex.as_str(), "/home/user/projects/testprj/([^/]+/)*foo/.*");
        assert_eq!(filter.only_dir, false);
        assert_eq!(filter.negate, false);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_negate_pattern() {
        let file_path = Path::new("/home/user/projects/testprj");
        let glob = "!foo";

        let result = convert_gitignore_pattern(glob, file_path);

        assert_eq!(result.len(), 1);

        let filter = &result[0];

        assert_eq!(filter.regex.as_str(), "/home/user/projects/testprj/([^/]+/)*foo");
        assert_eq!(filter.only_dir, false);
        assert_eq!(filter.negate, true);
    }

    // Windows

    #[test]
    #[cfg(windows)]
    fn test_simple_pattern() {
        let file_path = Path::new("C:\\Projects\\testprj");
        let glob = "foo";

        let result = convert_gitignore_pattern(glob, file_path);

        assert_eq!(result.len(), 1);

        let filter = &result[0];

        assert_eq!(filter.regex.as_str(), "C:/Projects/testprj/([^/]+/)*foo");
        assert_eq!(filter.only_dir, false);
        assert_eq!(filter.negate, false);
    }

    #[test]
    #[cfg(windows)]
    fn test_dir_pattern() {
        let file_path = Path::new("C:\\Projects\\testprj");
        let glob = "foo/";

        let result = convert_gitignore_pattern(glob, file_path);

        assert_eq!(result.len(), 2);

        let filter = &result[0];

        assert_eq!(filter.regex.as_str(), "C:/Projects/testprj/([^/]+/)*foo");
        assert_eq!(filter.only_dir, true);
        assert_eq!(filter.negate, false);

        let filter = &result[1];

        assert_eq!(filter.regex.as_str(), "C:/Projects/testprj/([^/]+/)*foo/.*");
        assert_eq!(filter.only_dir, false);
        assert_eq!(filter.negate, false);
    }

    #[test]
    #[cfg(windows)]
    fn test_negate_pattern() {
        let file_path = Path::new("C:\\Projects\\testprj");
        let glob = "!foo";

        let result = convert_gitignore_pattern(glob, file_path);

        assert_eq!(result.len(), 1);

        let filter = &result[0];

        assert_eq!(filter.regex.as_str(), "C:/Projects/testprj/([^/]+/)*foo");
        assert_eq!(filter.only_dir, false);
        assert_eq!(filter.negate, true);
    }
}