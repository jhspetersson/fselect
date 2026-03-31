//! Handles .hgignore parsing (Mercurial)

use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::ops::Add;
use std::ops::Index;
use std::path::Path;
use std::sync::LazyLock;

use regex::Captures;
use regex::Regex;

#[derive(Clone, Debug)]
pub struct HgignoreFilter {
    pub regex: Regex,
}

impl HgignoreFilter {
    fn new(regex: Regex) -> HgignoreFilter {
        HgignoreFilter { regex }
    }
}

pub fn search_upstream_hgignore(hgignore_filters: &mut Vec<HgignoreFilter>, dir: &Path) {
    if let Ok(canonical_path) = crate::util::canonical_path(&dir.to_path_buf()) {
        let mut path = std::path::PathBuf::from(canonical_path);

        loop {
            let hgignore_file = path.join(".hgignore");
            let hg_directory = path.join(".hg");

            if hgignore_file.is_file() && hg_directory.is_dir() {
                update_hgignore_filters(hgignore_filters, &mut path);
                return;
            }

            let parent_found = path.pop();

            if !parent_found {
                return;
            }
        }
    }
}

fn update_hgignore_filters(hgignore_filters: &mut Vec<HgignoreFilter>, path: &Path) {
    let hgignore_file = path.join(".hgignore");
    if hgignore_file.is_file() {
        let mut regexes = parse_hgignore(&hgignore_file, &path);
        match regexes {
            Ok(ref mut regexes) => {
                hgignore_filters.append(regexes);
            }
            Err(err) => {
                eprintln!("{}: {}", path.to_string_lossy(), err);
            }
        }
    }
}

pub fn matches_hgignore_filter(hgignore_filters: &Vec<HgignoreFilter>, file_name: &str) -> bool {
    let mut matched = false;

    for hgignore_filter in hgignore_filters {
        let is_match = hgignore_filter.regex.is_match(file_name);

        if is_match {
            matched = true;
        }
    }

    matched
}

enum Syntax {
    Regexp,
    Glob,
}

impl Syntax {
    fn from(s: &str) -> Result<Syntax, String> {
        if s == "regexp" {
            return Ok(Syntax::Regexp);
        } else if s == "glob" {
            return Ok(Syntax::Glob);
        } else {
            return Err("Error parsing syntax directive".to_string());
        }
    }
}

fn parse_hgignore(file_path: &Path, dir_path: &Path) -> Result<Vec<HgignoreFilter>, String> {
    let mut result = vec![];
    let mut err = String::new();

    if let Ok(file) = File::open(file_path) {
        let mut syntax = Syntax::Regexp;

        let reader = BufReader::new(file);
        reader
            .lines()
            .filter(|line| match line {
                Ok(line) => !line.trim().is_empty() && !line.starts_with("#"),
                _ => false,
            })
            .for_each(|line| {
                if err.is_empty() {
                    match line {
                        Ok(line) => {
                            if line.starts_with("syntax:") {
                                let line = line.replace("syntax:", "");
                                let syntax_directive = line.trim();
                                match Syntax::from(syntax_directive) {
                                    Ok(parsed_syntax) => syntax = parsed_syntax,
                                    Err(parse_err) => err = parse_err,
                                }
                            } else if line.starts_with("subinclude:") {
                                let include = line.replace("subinclude:", "");
                                let mut parse_result =
                                    parse_hgignore(&Path::new(&include), dir_path);
                                match parse_result {
                                    Ok(ref mut filters) => {
                                        result.append(filters);
                                    }
                                    Err(parse_err) => {
                                        err = parse_err;
                                    }
                                };
                            } else {
                                let pattern = convert_hgignore_pattern(&line, dir_path, &syntax);
                                match pattern {
                                    Ok(pattern) => result.push(pattern),
                                    Err(parse_err) => err = parse_err,
                                }
                            }
                        }
                        _ => {}
                    }
                }
            });
    };

    match err.is_empty() {
        true => Ok(result),
        false => Err(err),
    }
}

fn convert_hgignore_pattern(
    pattern: &str,
    file_path: &Path,
    syntax: &Syntax,
) -> Result<HgignoreFilter, String> {
    match syntax {
        Syntax::Glob => match convert_hgignore_glob(pattern, file_path) {
            Ok(regex) => Ok(HgignoreFilter::new(regex)),
            Err(e) => Err(e),
        },
        Syntax::Regexp => match convert_hgignore_regexp(pattern, file_path) {
            Ok(regex) => Ok(HgignoreFilter::new(regex)),
            Err(e) => Err(e),
        },
    }
}

static HG_CONVERT_REPLACE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(\\*\\*|\\?|\\.|\\[|\\]|\\(|\\)|\\^|\\$|\\*)").unwrap()
});

fn convert_hgignore_glob(glob: &str, file_path: &Path) -> Result<Regex, String> {
    #[cfg(not(windows))]
    {
        let mut pattern = HG_CONVERT_REPLACE_REGEX
            .replace_all(&glob, |c: &Captures| {
                match c.index(0) {
                    "**" => ".*",
                    "." => "\\.",
                    "*" => "[^/]*",
                    "?" => "[^/]",
                    "[" => "\\[",
                    "]" => "\\]",
                    "(" => "\\(",
                    ")" => "\\)",
                    "^" => "\\^",
                    "$" => "\\$",
                    _ => "",
                }
                .to_string()
            })
            .to_string();

        if pattern.is_empty() {
            return Err("Error parsing .hgignore pattern: ".to_string() + glob);
        }

        pattern = regex::escape(&file_path.to_string_lossy())
            .add("/([^/]+/)*")
            .add(&pattern);

        Regex::new(&pattern).map_err(|_| "Error creating regex pattern: ".to_string() + pattern.as_str())
    }

    #[cfg(windows)]
    {
        let mut pattern = HG_CONVERT_REPLACE_REGEX
            .replace_all(&glob, |c: &Captures| {
                match c.index(0) {
                    "**" => ".*",
                    "." => "\\.",
                    "*" => "[^\\\\]*",
                    "?" => "[^\\\\]",
                    "[" => "\\[",
                    "]" => "\\]",
                    "(" => "\\(",
                    ")" => "\\)",
                    "^" => "\\^",
                    "$" => "\\$",
                    _ => "",
                }
                .to_string()
            })
            .to_string();

        if pattern.is_empty() {
            return Err("Error parsing .hgignore pattern: ".to_string() + glob);
        }

        pattern = regex::escape(&file_path.to_string_lossy())
            .add("\\\\([^\\\\]+\\\\)*")
            .add(&pattern);

        Regex::new(&pattern).map_err(|_| "Error creating regex pattern: ".to_string() + pattern.as_str())
    }
}

fn convert_hgignore_regexp(regexp: &str, file_path: &Path) -> Result<Regex, String> {
    #[cfg(not(windows))]
    {
        let mut pattern = regex::escape(&file_path.to_string_lossy());
        if !regexp.starts_with("^") {
            pattern = pattern.add("/([^/]+/)*");
            pattern = pattern.add(".*");
        } else {
            pattern = pattern.add("/");
        }

        pattern = pattern.add(&regexp.trim_start_matches("^"));

        Regex::new(&pattern).map_err(|_| "Error creating regex pattern: ".to_string() + pattern.as_str())
    }

    #[cfg(windows)]
    {
        let mut pattern = regex::escape(&file_path.to_string_lossy());
        if !regexp.starts_with("^") {
            pattern = pattern.add("\\\\([^\\\\]+\\\\)*");
            pattern = pattern.add(".*");
        } else {
            pattern = pattern.add("\\\\");
        }

        pattern = pattern.add(&regexp.trim_start_matches("^"));

        Regex::new(&pattern).map_err(|_| "Error creating regex pattern: ".to_string() + pattern.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn glob_question_mark_matches_exactly_one_char() {
        let regex = convert_hgignore_glob("a?b", Path::new("/tmp")).unwrap();
        assert!(regex.is_match("/tmp/axb"), "? should match single char");
        assert!(
            !regex.is_match("/tmp/axxb"),
            "? should not match two chars but got match"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn glob_path_with_dots_is_regex_escaped() {
        let result = convert_hgignore_glob("*.txt", Path::new("/home/user/my.project"));
        let regex = result.unwrap();
        let regex_str = regex.as_str();
        assert!(
            regex_str.contains("my\\.project"),
            "dots in path should be escaped but got: {}",
            regex_str
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn regexp_path_with_dots_is_regex_escaped() {
        let result = convert_hgignore_regexp("foo", Path::new("/home/user/my.project"));
        let regex = result.unwrap();
        let regex_str = regex.as_str();
        assert!(
            regex_str.contains("my\\.project"),
            "dots in path should be escaped but got: {}",
            regex_str
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn glob_brackets_are_escaped() {
        let result = convert_hgignore_glob("[test]", Path::new("/tmp"));
        let regex = result.unwrap();
        let regex_str = regex.as_str();
        assert!(
            regex_str.contains("\\[") && regex_str.contains("\\]"),
            "brackets should be escaped but got: {}",
            regex_str
        );
    }

    #[cfg(windows)]
    #[test]
    fn glob_question_mark_matches_exactly_one_char_windows() {
        let regex = convert_hgignore_glob("a?b", Path::new("C:\\tmp")).unwrap();
        assert!(regex.is_match("C:\\tmp\\axb"), "? should match single char");
        assert!(
            !regex.is_match("C:\\tmp\\axxb"),
            "? should not match two chars but got match"
        );
    }

    #[cfg(windows)]
    #[test]
    fn glob_path_with_dots_is_regex_escaped_windows() {
        let result = convert_hgignore_glob("*.txt", Path::new("C:\\Users\\user\\my.project"));
        let regex = result.unwrap();
        let regex_str = regex.as_str();
        assert!(
            regex_str.contains("my\\.project"),
            "dots in path should be escaped but got: {}",
            regex_str
        );
    }

    #[cfg(windows)]
    #[test]
    fn regexp_path_with_dots_is_regex_escaped_windows() {
        let result = convert_hgignore_regexp("foo", Path::new("C:\\Users\\user\\my.project"));
        let regex = result.unwrap();
        let regex_str = regex.as_str();
        assert!(
            regex_str.contains("my\\.project"),
            "dots in path should be escaped but got: {}",
            regex_str
        );
    }

    #[cfg(windows)]
    #[test]
    fn glob_brackets_are_escaped_windows() {
        let result = convert_hgignore_glob("[test]", Path::new("C:\\tmp"));
        let regex = result.unwrap();
        let regex_str = regex.as_str();
        assert!(
            regex_str.contains("\\[") && regex_str.contains("\\]"),
            "brackets should be escaped but got: {}",
            regex_str
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn regexp_caret_anchored_includes_separator() {
        let regex = convert_hgignore_regexp("^src/main", Path::new("/repo")).unwrap();
        assert!(regex.is_match("/repo/src/main.rs"), "^-anchored pattern should match");
        assert!(!regex.is_match("/reposrc/main.rs"), "should not match without separator");
    }

    #[cfg(windows)]
    #[test]
    fn regexp_caret_anchored_includes_separator_windows() {
        let regex = convert_hgignore_regexp("^src/main", Path::new("C:\\repo")).unwrap();
        assert!(regex.is_match("C:\\repo\\src/main.rs"), "^-anchored pattern should match");
    }
}
