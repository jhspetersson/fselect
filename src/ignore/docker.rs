//! Handles .dockerignore parsing

use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::ops::Add;
use std::path::Path;

use regex::Regex;

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
                update_dockerignore_filters(dockerignore_filters, &path);
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
        let regexes = parse_dockerignore(&dockerignore_file, path);
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

        if is_match {
            matched = !dockerignore_filter.negate;
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
        for (index, line) in reader.lines().enumerate() {
            if err.is_empty()
                && let Ok(line) = line {
                    // Docker strips a UTF-8 BOM from the first line and
                    // trims whitespace from every line before parsing.
                    let line = match index {
                        0 => line.trim_start_matches('\u{feff}'),
                        _ => line.as_str(),
                    };
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let pattern = convert_dockerignore_pattern(line, dir_path);
                    match pattern {
                        Ok(pattern) => result.push(pattern),
                        Err(parse_err) => err = parse_err,
                    }
                }
        }
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
    let mut pattern = pattern;

    let mut negate = false;
    if let Some(rest) = pattern.strip_prefix('!') {
        // Docker trims whitespace again after removing the `!`.
        pattern = rest.trim_start();
        negate = true;
    }

    match convert_dockerignore_glob(pattern, file_path) {
        Ok(regex) => Ok(DockerignoreFilter::new(regex, negate)),
        _ => Err("Error creating regex while parsing .dockerignore glob: "
            .to_string()
            .add(pattern)),
    }
}

// Finds the index of the `]` closing the character class that starts at
// `start` (which must hold `[`), honoring `\` escapes; None if unterminated.
fn find_char_class_end(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            ']' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn convert_dockerignore_glob(glob: &str, file_path: &Path) -> Result<Regex, String> {
    // Patterns are relative to the context root; leading and trailing
    // separators carry no meaning.
    let glob_trimmed = glob.trim_start_matches(['/', '\\']).trim_end_matches('/');

    if glob_trimmed.is_empty() {
        return Err("Error parsing .dockerignore pattern: ".to_string() + glob);
    }

    let mut pattern = String::new();
    let chars: Vec<char> = glob_trimmed.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                // `**/` matches any number of leading directories,
                // including none.
                if chars.get(i + 2) == Some(&'/') {
                    pattern.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    pattern.push_str(".*");
                    i += 2;
                }
            }
            '*' => {
                pattern.push_str("[^/]*");
                i += 1;
            }
            '?' => {
                pattern.push_str("[^/]");
                i += 1;
            }
            '[' => match find_char_class_end(&chars, i) {
                // Go filepath.Match character classes (`[abc]`, ranges like
                // `[a-z]`, negated `[^abc]`) pass through as regex classes,
                // exactly as Docker's patternmatcher does.
                Some(end) => {
                    pattern.extend(&chars[i..=end]);
                    i = end + 1;
                }
                // An unterminated `[` is ErrBadPattern in Go (never
                // matches); escaping it as a literal is the safer choice
                // here since an invalid pattern would otherwise discard
                // the whole .dockerignore file.
                None => {
                    pattern.push_str("\\[");
                    i += 1;
                }
            },
            c @ ('.' | ']' | '(' | ')' | '^' | '$' | '+' | '{' | '}' | '|' | '\\') => {
                pattern.push('\\');
                pattern.push(c);
                i += 1;
            }
            c => {
                pattern.push(c);
                i += 1;
            }
        }
    }

    #[cfg(windows)]
    let path = file_path
        .to_string_lossy()
        .to_string()
        .replace("\\", "/")
        .replace("//", "/");

    #[cfg(not(windows))]
    let path = file_path.to_string_lossy().to_string();

    // Docker patterns are anchored at the context root (the directory holding
    // the .dockerignore) rather than floating to any depth, and a pattern that
    // matches a directory also excludes everything beneath it.
    let pattern = String::from("^")
        .add(&regex::escape(&path))
        .add("/")
        .add(&pattern)
        .add("(?:/.*)?$");

    Regex::new(&pattern).map_err(|_| "Error creating regex pattern: ".to_string() + pattern.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negate_pattern_removes_only_leading_exclamation() {
        let filter = convert_dockerignore_pattern("!foo!bar", Path::new("/tmp")).unwrap();
        assert!(filter.negate);
        let regex_str = filter.regex.as_str();
        assert!(
            regex_str.contains("foo!bar") || regex_str.contains("foo\\!bar"),
            "pattern should preserve non-leading ! but got: {}",
            regex_str
        );
    }

    #[test]
    fn last_matching_rule_wins() {
        let filters = vec![
            DockerignoreFilter::new(Regex::new(".*\\.log$").unwrap(), false),
            DockerignoreFilter::new(Regex::new(".*important\\.log$").unwrap(), true),
            DockerignoreFilter::new(Regex::new(".*\\.log$").unwrap(), false),
        ];
        assert!(
            matches_dockerignore_filter(&filters, "important.log"),
            "last non-negated *.log should override the negation"
        );
    }

    #[test]
    fn path_with_dots_is_regex_escaped() {
        let result = convert_dockerignore_glob("*.txt", Path::new("/home/user/my.project"));
        let regex = result.unwrap();
        let regex_str = regex.as_str();
        assert!(
            regex_str.contains("my\\.project"),
            "dots in path should be escaped but got: {}",
            regex_str
        );
    }

    #[test]
    fn char_class_matches_listed_characters() {
        let filter = convert_dockerignore_pattern("*.[ch]", Path::new("/ctx")).unwrap();
        assert!(filter.regex.is_match("/ctx/main.c"));
        assert!(filter.regex.is_match("/ctx/util.h"));
        assert!(!filter.regex.is_match("/ctx/main.o"));
    }

    #[test]
    fn char_class_supports_ranges() {
        let filter = convert_dockerignore_pattern("file[a-c].txt", Path::new("/ctx")).unwrap();
        assert!(filter.regex.is_match("/ctx/filea.txt"));
        assert!(filter.regex.is_match("/ctx/filec.txt"));
        assert!(!filter.regex.is_match("/ctx/filed.txt"));
    }

    #[test]
    fn char_class_supports_negation() {
        let filter = convert_dockerignore_pattern("file[^a].txt", Path::new("/ctx")).unwrap();
        assert!(filter.regex.is_match("/ctx/fileb.txt"));
        assert!(!filter.regex.is_match("/ctx/filea.txt"));
    }

    #[test]
    fn unterminated_char_class_is_treated_as_literal() {
        let filter = convert_dockerignore_pattern("foo[bar", Path::new("/ctx")).unwrap();
        assert!(filter.regex.is_match("/ctx/foo[bar"));
        assert!(!filter.regex.is_match("/ctx/foobar"));
    }

    #[test]
    fn glob_with_plus_is_escaped() {
        let result = convert_dockerignore_glob("a+b.txt", Path::new("/tmp"));
        assert!(result.is_ok());
        let regex = result.unwrap();
        let regex_str = regex.as_str();
        assert!(
            regex_str.contains("\\+"),
            "plus should be escaped but got: {}",
            regex_str
        );
    }

    #[test]
    fn pattern_matches_whole_component_not_prefix() {
        let filter = convert_dockerignore_pattern("foo", Path::new("/ctx")).unwrap();
        assert!(filter.regex.is_match("/ctx/foo"));
        assert!(
            filter.regex.is_match("/ctx/foo/sub/file.txt"),
            "a matched directory excludes its subtree"
        );
        assert!(
            !filter.regex.is_match("/ctx/foobar"),
            "pattern must not match by prefix"
        );
    }

    #[test]
    fn pattern_is_anchored_to_context_root() {
        let filter = convert_dockerignore_pattern("foo", Path::new("/ctx")).unwrap();
        assert!(
            !filter.regex.is_match("/ctx/a/foo"),
            "plain patterns are root-relative in Docker, not any-depth"
        );
        assert!(
            !filter.regex.is_match("/other/ctx/foo"),
            "pattern must not match under a different root"
        );
    }

    #[test]
    fn doublestar_matches_any_depth_including_root() {
        let filter = convert_dockerignore_pattern("**/foo", Path::new("/ctx")).unwrap();
        assert!(filter.regex.is_match("/ctx/foo"), "`**/` includes zero directories");
        assert!(filter.regex.is_match("/ctx/a/b/foo"));
        assert!(!filter.regex.is_match("/ctx/a/foobar"));
    }

    #[test]
    fn star_does_not_cross_separators() {
        let filter = convert_dockerignore_pattern("*.md", Path::new("/ctx")).unwrap();
        assert!(filter.regex.is_match("/ctx/readme.md"));
        assert!(
            !filter.regex.is_match("/ctx/sub/readme.md"),
            "`*.md` matches only at the context root in Docker"
        );
    }

    #[test]
    fn trailing_slash_matches_directory_and_contents() {
        let filter = convert_dockerignore_pattern("build/", Path::new("/ctx")).unwrap();
        assert!(filter.regex.is_match("/ctx/build"));
        assert!(filter.regex.is_match("/ctx/build/out/app.bin"));
        assert!(!filter.regex.is_match("/ctx/builder"));
    }

    #[test]
    fn test_all_slashes_pattern_rejected() {
        let result = convert_dockerignore_glob("///", Path::new("/tmp"));
        assert!(result.is_err(), "pattern of only slashes should be rejected");
    }

    #[test]
    fn indented_comment_is_ignored() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("fselect_docker_indented_comment_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join(".dockerignore");
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            writeln!(f, "   # this is an indented comment").unwrap();
            writeln!(f, "real_pattern").unwrap();
        }

        let filters = parse_dockerignore(&file_path, &dir).unwrap();
        assert_eq!(
            filters.len(),
            1,
            "indented comment should not be parsed as a pattern, got {} filters",
            filters.len()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_are_whitespace_trimmed() {
        let dir = std::env::temp_dir().join("fselect_docker_trim_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join(".dockerignore");
        std::fs::write(&file_path, "foo \n  !bar\n").unwrap();

        let filters = parse_dockerignore(&file_path, &dir).unwrap();
        assert_eq!(filters.len(), 2);

        let target_foo = dir.join("foo").to_string_lossy().to_string();
        assert!(
            matches_dockerignore_filter(&filters, &target_foo),
            "trailing whitespace should be trimmed before compilation"
        );
        assert!(filters[1].negate, "indented negation should be detected");
        let target_bar = dir.join("bar").to_string_lossy().replace('\\', "/");
        assert!(
            filters[1].regex.is_match(&target_bar),
            "leading whitespace should be trimmed before compilation"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utf8_bom_is_stripped_from_first_line() {
        let dir = std::env::temp_dir().join("fselect_docker_bom_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join(".dockerignore");
        std::fs::write(&file_path, b"\xEF\xBB\xBFfoo\n").unwrap();

        let filters = parse_dockerignore(&file_path, &dir).unwrap();
        assert_eq!(filters.len(), 1);
        let target = dir.join("foo").to_string_lossy().to_string();
        assert!(
            matches_dockerignore_filter(&filters, &target),
            "first pattern should match despite the BOM"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
