use std::fs::File;
use std::ops::Add;
use std::ops::Index;
use std::path::Path;

use regex::Captures;
use regex::Error;
use regex::Regex;

#[derive(Clone)]
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

pub fn matches_gitignore_filter(gitignore_filters: &Option<Vec<GitignoreFilter>>, file_name: &str, is_dir: bool) -> bool {
    match gitignore_filters {
        Some(gitignore_filters) => {
            let mut matched = false;

            for gitignore_filter in gitignore_filters {
                if gitignore_filter.only_dir && !is_dir {
                    continue;
                }

                let is_match = gitignore_filter.regex.is_match(file_name);

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

pub fn parse_gitignore(file_path: &Path) -> Vec<GitignoreFilter> {
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
                    Ok(line) => result.append(&mut convert_gitignore_pattern(&line, file_path)),
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

fn convert_gitignore_glob(glob: &str, file_path: &Path) -> Result<Regex, Error> {
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

    pattern = file_path.to_string_lossy().to_string().add("/([^/]+/)*").add(&pattern);

    Regex::new(&pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
}