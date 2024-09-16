//! Handles .gitignore parsing

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::ops::Add;
use std::ops::Index;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use regex::Captures;
use regex::Error;
use regex::Regex;

use crate::util::error_exit;

#[derive(Clone, Debug)]
pub struct GitignoreFilter {
    pub regex: Regex,
    pub only_dir: bool,
    pub negate: bool,
}

impl GitignoreFilter {
    fn new(regex: Regex, only_dir: bool, negate: bool) -> GitignoreFilter {
        GitignoreFilter {
            regex,
            only_dir,
            negate,
        }
    }
}

pub fn search_upstream_gitignore(
    gitignore_map: &mut HashMap<PathBuf, Vec<GitignoreFilter>>,
    dir: &Path,
) {
    if let Ok(canonical_path) = crate::util::canonical_path(&dir.to_path_buf()) {
        let mut path = PathBuf::from(canonical_path);
        
        if let Some(root_dir) = path.iter().next() {
            parse_global_ignore(gitignore_map, root_dir);
        }

        loop {
            let parent_found = path.pop();

            if !parent_found {
                return;
            }

            update_gitignore_map(gitignore_map, &mut path);
        }
    }
}

pub fn update_gitignore_map(
    gitignore_map: &mut HashMap<PathBuf, Vec<GitignoreFilter>>,
    path: &Path,
) {
    let gitignore_file = path.join(".gitignore");
    if gitignore_file.is_file() {
        let regexes = parse_gitignore(&gitignore_file, path);
        gitignore_map.insert(path.to_path_buf(), regexes);
    }
}

pub fn get_gitignore_filters(
    gitignore_map: &mut HashMap<PathBuf, Vec<GitignoreFilter>>,
    dir: &Path,
) -> Vec<GitignoreFilter> {
    if let Some(regexes) = gitignore_map.get(&dir.to_path_buf()) {
        return regexes.to_vec();
    }

    let mut result = vec![];

    let mut path = dir.to_path_buf();

    loop {
        let parent_found = path.pop();

        if !parent_found {
            return result;
        }

        if let Some(regexes) = gitignore_map.get(&path) {
            result = vec![regexes.to_vec(), result].concat();
        }
    }
}

pub fn matches_gitignore_filter(
    gitignore_filters: &Option<Vec<GitignoreFilter>>,
    file_name: &str,
    is_dir: bool,
) -> bool {
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
        }
        _ => false,
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

fn parse_global_ignore(
    gitignore_map: &mut HashMap<PathBuf, Vec<GitignoreFilter>>,
    root_dir: &OsStr
) {
    let mut regexes: Vec<GitignoreFilter> = Vec::new();
    
    let home_env_var;
    #[cfg(windows)]
    {
        home_env_var = "USERPROFILE";
    }
    #[cfg(not(windows))]
    {
        home_env_var = "HOME";
    }

    if let Ok(user_profile) = std::env::var(home_env_var) {
        let mut global_config_found = false;
        if !user_profile.is_empty() {
            let user_profile_gitconfig = user_profile + "/.gitconfig";
            if let Ok(file) = File::open(user_profile_gitconfig) {
                use std::io::BufRead;
                use std::io::BufReader;
                let reader = BufReader::new(file);
                let excludes_file = reader
                    .lines()
                    .filter(|line| match line {
                        Ok(line) => !line.starts_with(";") && !line.starts_with("#") && line.contains("excludesFile"),
                        _ => false,
                    })
                    .next();
                if let Some(Ok(excludes_file)) = excludes_file {
                    let excludes_file: Vec<&str> = excludes_file.split("=").collect();
                    if excludes_file.len() == 2 {
                        let excludes_file = excludes_file[1].trim();
                        regexes.append(&mut parse_file(Path::new(excludes_file), Path::new("/")));
                        global_config_found = true;
                    }
                }
            }
        }
        
        if !global_config_found {
            if let Ok(xdg_home) = std::env::var("XDG_CONFIG_HOME") {
                if !xdg_home.is_empty() {
                    let xdg_home_ignore = xdg_home + "/git/ignore";
                    regexes.append(&mut parse_file(Path::new(&xdg_home_ignore), Path::new("/")));
                    global_config_found = true;
                }
            }
            
            if !global_config_found {
                if let Ok(home) = std::env::var("HOME") {
                    if !home.is_empty() {
                        let home_ignore = home + "/.config/git/ignore";
                        regexes.append(&mut parse_file(Path::new(&home_ignore), Path::new("/")));
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        gitignore_map.insert(Path::new((root_dir.to_string_lossy() + "\\").as_ref()).to_path_buf(), regexes);
    }

    #[cfg(not(windows))]
    {
        gitignore_map.insert(Path::new(root_dir).to_path_buf(), regexes);
    }
}

fn parse_file(file_path: &Path, dir_path: &Path) -> Vec<GitignoreFilter> {
    let mut result = vec![];

    if let Ok(file) = File::open(file_path) {
        use std::io::BufRead;
        use std::io::BufReader;
        let reader = BufReader::new(file);
        reader
            .lines()
            .filter(|line| match line {
                Ok(line) => !line.trim().is_empty() && !line.starts_with("#"),
                _ => false,
            })
            .for_each(|line| {
                if let Ok(line) = line {
                    result.append(&mut convert_gitignore_pattern(&line, dir_path))
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
        if let Ok(regex) = regex {
            result.push(GitignoreFilter::new(regex, true, negate));
        }

        pattern = pattern.add("/**");
    }

    let regex = convert_gitignore_glob(&pattern, file_path);
    if let Ok(regex) = regex {
        result.push(GitignoreFilter::new(regex, false, negate))
    }

    result
}

static GIT_CONVERT_REPLACE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(\\*\\*|\\?|\\.|\\*)").unwrap()
});

fn convert_gitignore_glob(glob: &str, file_path: &Path) -> Result<Regex, Error> {
    let mut pattern = GIT_CONVERT_REPLACE_REGEX
        .replace_all(&glob, |c: &Captures| {
            match c.index(0) {
                "**" => ".*",
                "." => "\\.",
                "*" => "[^/]*",
                "?" => "[^/]+",
                _ => error_exit(".gitignore", "Error parsing pattern"),
            }
            .to_string()
        })
        .to_string();

    while pattern.starts_with("/") || pattern.starts_with("\\") {
        pattern.remove(0);
    }

    #[allow(unused_mut)]
    let mut file_path_pattern = file_path
        .to_string_lossy()
        .to_string()
        .replace("\\", "\\\\")
        .add("/([^/]+/)*");

    #[cfg(windows)]
    {
        file_path_pattern = file_path_pattern.replace("\\", "/").replace("//", "/");
    }
    
    pattern = file_path_pattern.add(&pattern);

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

        assert_eq!(
            filter.regex.as_str(),
            "/home/user/projects/testprj/([^/]+/)*foo"
        );
        assert!(!filter.only_dir);
        assert!(!filter.negate);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_dir_pattern() {
        let file_path = Path::new("/home/user/projects/testprj");
        let glob = "foo/";

        let result = convert_gitignore_pattern(glob, file_path);

        assert_eq!(result.len(), 2);

        let filter = &result[0];

        assert_eq!(
            filter.regex.as_str(),
            "/home/user/projects/testprj/([^/]+/)*foo"
        );
        assert!(filter.only_dir);
        assert!(!filter.negate);

        let filter = &result[1];

        assert_eq!(
            filter.regex.as_str(),
            "/home/user/projects/testprj/([^/]+/)*foo/.*"
        );
        assert!(!filter.only_dir);
        assert!(!filter.negate);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_negate_pattern() {
        let file_path = Path::new("/home/user/projects/testprj");
        let glob = "!foo";

        let result = convert_gitignore_pattern(glob, file_path);

        assert_eq!(result.len(), 1);

        let filter = &result[0];

        assert_eq!(
            filter.regex.as_str(),
            "/home/user/projects/testprj/([^/]+/)*foo"
        );
        assert!(!filter.only_dir);
        assert!(filter.negate);
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
        assert!(!filter.only_dir);
        assert!(!filter.negate);
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
        assert!(filter.only_dir);
        assert!(!filter.negate);

        let filter = &result[1];

        assert_eq!(filter.regex.as_str(), "C:/Projects/testprj/([^/]+/)*foo/.*");
        assert!(!filter.only_dir);
        assert!(!filter.negate);
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
        assert!(!filter.only_dir);
        assert!(filter.negate);
    }
}
