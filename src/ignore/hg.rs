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
                update_hgignore_filters(hgignore_filters, &path);
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
        let mut regexes = parse_hgignore(&hgignore_file, path);
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

pub fn matches_hgignore_filter(hgignore_filters: &[HgignoreFilter], file_name: &str) -> bool {
    hgignore_filters
        .iter()
        .any(|filter| filter.regex.is_match(file_name))
}

enum Syntax {
    Regexp,
    Glob,
    RootGlob,
}

impl Syntax {
    fn from(s: &str) -> Option<Syntax> {
        match s {
            "regexp" | "re" => Some(Syntax::Regexp),
            "glob" => Some(Syntax::Glob),
            "rootglob" => Some(Syntax::RootGlob),
            _ => None,
        }
    }
}

// Mercurial's readpatternfile: a `#` preceded by an even number of
// backslashes starts a comment; `\#` is an escaped literal hash.
static HG_COMMENT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("((?:^|[^\\\\])(?:\\\\\\\\)*)#").unwrap());

fn strip_hg_comment(line: &str) -> String {
    if line.contains('#') {
        let mut line = line.to_string();
        if let Some(caps) = HG_COMMENT_REGEX.captures(&line) {
            line.truncate(caps.get(1).unwrap().end());
        }
        line.replace("\\#", "#")
    } else {
        line.to_string()
    }
}

fn parse_hgignore(file_path: &Path, dir_path: &Path) -> Result<Vec<HgignoreFilter>, String> {
    let mut result = vec![];
    let mut err = String::new();

    if let Ok(file) = File::open(file_path) {
        let mut syntax = Syntax::Regexp;

        let reader = BufReader::new(file);
        for line in reader.lines() {
            if err.is_empty()
                && let Ok(line) = line {
                    let line = strip_hg_comment(&line);
                    // Mercurial rstrip()s each line (but keeps leading
                    // whitespace, which makes a directive an ordinary
                    // pattern).
                    let line = line.trim_end();
                    if line.is_empty() {
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("syntax:") {
                        let syntax_directive = rest.trim();
                        match Syntax::from(syntax_directive) {
                            Some(parsed_syntax) => syntax = parsed_syntax,
                            // An unknown syntax only skips this line;
                            // Mercurial warns and keeps the previous
                            // syntax in effect.
                            None => eprintln!(
                                "{}: ignoring invalid syntax '{}'",
                                file_path.to_string_lossy(),
                                syntax_directive
                            ),
                        }
                    } else if let Some(rest) = line.strip_prefix("subinclude:") {
                        let include = rest.trim();
                        #[cfg(windows)]
                        let include = include.replace('/', "\\");
                        // Relative paths resolve against the directory of
                        // the file containing the subinclude, and the
                        // subincluded patterns are rooted at the subinclude
                        // file's directory (Mercurial `subinclude:`).
                        let include_path = match file_path.parent() {
                            Some(parent) => parent.join(&include),
                            None => Path::new(&include).to_path_buf(),
                        };
                        let sub_dir_path = include_path
                            .parent()
                            .map(Path::to_path_buf)
                            .unwrap_or_else(|| dir_path.to_path_buf());
                        let mut parse_result = parse_hgignore(&include_path, &sub_dir_path);
                        match parse_result {
                            Ok(ref mut filters) => {
                                result.append(filters);
                            }
                            Err(parse_err) => {
                                err = parse_err;
                            }
                        };
                    } else {
                        let pattern = convert_hgignore_pattern(line, dir_path, &syntax);
                        match pattern {
                            Ok(pattern) => result.push(pattern),
                            Err(parse_err) => err = parse_err,
                        }
                    }
                }
        }
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
        Syntax::Glob => match convert_hgignore_glob(pattern, file_path, false) {
            Ok(regex) => Ok(HgignoreFilter::new(regex)),
            Err(e) => Err(e),
        },
        Syntax::RootGlob => match convert_hgignore_glob(pattern, file_path, true) {
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
    Regex::new("(\\*\\*|\\?|\\.|\\[|\\]|\\(|\\)|\\^|\\$|\\*|\\+|\\{|\\}|\\||\\\\|/)").unwrap()
});

fn convert_hgignore_glob(glob: &str, file_path: &Path, rooted: bool) -> Result<Regex, String> {
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
                    "+" => "\\+",
                    "{" => "\\{",
                    "}" => "\\}",
                    "|" => "\\|",
                    "\\" => "\\\\",
                    "/" => "/",
                    _ => "",
                }
                .to_string()
            })
            .to_string();

        if pattern.is_empty() {
            return Err("Error parsing .hgignore pattern: ".to_string() + glob);
        }

        // `**/` matches any number of leading directories, including none
        // (the `.*` token can only originate from `**`).
        pattern = pattern.replace(".*/", "(?:.*/)?");

        // Glob patterns are unrooted (they match at any directory level)
        // while `rootglob` patterns anchor at the repo root; both must cover
        // whole path components: like Mercurial itself, a match ends at a
        // separator or the end of the path, so `foo` matches `foo` and
        // `foo/bar` but not `foobar`.
        pattern = String::from("^")
            .add(&regex::escape(&file_path.to_string_lossy()))
            .add(if rooted { "/" } else { "/([^/]+/)*" })
            .add(&pattern)
            .add("(?:/|$)");

        Regex::new(&pattern).map_err(|_| "Error creating regex pattern: ".to_string() + pattern.as_str())
    }

    #[cfg(windows)]
    {
        let mut pattern = HG_CONVERT_REPLACE_REGEX
            .replace_all(glob, |c: &Captures| {
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
                    "+" => "\\+",
                    "{" => "\\{",
                    "}" => "\\}",
                    "|" => "\\|",
                    "\\" => "\\\\",
                    // Mercurial patterns always use forward slashes; paths
                    // are matched with native separators on Windows.
                    "/" => "\\\\",
                    _ => "",
                }
                .to_string()
            })
            .to_string();

        if pattern.is_empty() {
            return Err("Error parsing .hgignore pattern: ".to_string() + glob);
        }

        // `**/` matches any number of leading directories, including none
        // (the `.*` token can only originate from `**`).
        pattern = pattern.replace(".*\\\\", "(?:.*\\\\)?");

        // See the Unix branch: unrooted (or root-anchored for `rootglob`),
        // but matches whole path components.
        pattern = String::from("^")
            .add(&regex::escape(&file_path.to_string_lossy()))
            .add(if rooted { "\\\\" } else { "\\\\([^\\\\]+\\\\)*" })
            .add(&pattern)
            .add("(?:\\\\|$)");

        Regex::new(&pattern).map_err(|_| "Error creating regex pattern: ".to_string() + pattern.as_str())
    }
}

fn convert_hgignore_regexp(regexp: &str, file_path: &Path) -> Result<Regex, String> {
    // Mercurial matches regexp patterns with `re.search` against the
    // repo-relative path: unanchored unless the pattern starts with `^`.
    // Only the repository-root prefix is anchored here; no end anchor is
    // added so the user's regex keeps full control.
    #[cfg(not(windows))]
    {
        let mut pattern = String::from("^") + &regex::escape(&file_path.to_string_lossy());
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
        // Mercurial regexps use `/` as the path separator while matched
        // paths are native `\`-separated on Windows, so an (optionally
        // escaped) `/` in the pattern must match a literal backslash.
        let regexp = regexp.replace("\\/", "/").replace('/', "\\\\");

        let mut pattern = String::from("^") + &regex::escape(&file_path.to_string_lossy());
        if !regexp.starts_with("^") {
            pattern = pattern.add("\\\\([^\\\\]+\\\\)*");
            pattern = pattern.add(".*");
        } else {
            pattern = pattern.add("\\\\");
        }

        pattern = pattern.add(regexp.trim_start_matches("^"));

        Regex::new(&pattern).map_err(|_| "Error creating regex pattern: ".to_string() + pattern.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn glob_question_mark_matches_exactly_one_char() {
        let regex = convert_hgignore_glob("a?b", Path::new("/tmp"), false).unwrap();
        assert!(regex.is_match("/tmp/axb"), "? should match single char");
        assert!(
            !regex.is_match("/tmp/axxb"),
            "? should not match two chars but got match"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn glob_path_with_dots_is_regex_escaped() {
        let result = convert_hgignore_glob("*.txt", Path::new("/home/user/my.project"), false);
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
        let result = convert_hgignore_glob("[test]", Path::new("/tmp"), false);
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
        let regex = convert_hgignore_glob("a?b", Path::new("C:\\tmp"), false).unwrap();
        assert!(regex.is_match("C:\\tmp\\axb"), "? should match single char");
        assert!(
            !regex.is_match("C:\\tmp\\axxb"),
            "? should not match two chars but got match"
        );
    }

    #[cfg(windows)]
    #[test]
    fn glob_path_with_dots_is_regex_escaped_windows() {
        let result = convert_hgignore_glob("*.txt", Path::new("C:\\Users\\user\\my.project"), false);
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
        let result = convert_hgignore_glob("[test]", Path::new("C:\\tmp"), false);
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
    fn glob_plus_and_pipe_are_escaped() {
        let regex = convert_hgignore_glob("c++", Path::new("/repo"), false).unwrap();
        assert!(regex.is_match("/repo/c++"), "+ should be literal");
        assert!(!regex.is_match("/repo/ccc"), "+ should not repeat");

        let regex = convert_hgignore_glob("a|b*", Path::new("/repo"), false).unwrap();
        assert!(regex.is_match("/repo/a|bc"), "| should be literal");
        assert!(!regex.is_match("/repo/ab.txt"), "| should not alternate");
    }

    #[cfg(not(windows))]
    #[test]
    fn glob_double_star_slash_matches_zero_dirs() {
        let regex = convert_hgignore_glob("**/foo", Path::new("/repo"), false).unwrap();
        assert!(regex.is_match("/repo/foo"), "**/ should match zero dirs");
        assert!(regex.is_match("/repo/a/b/foo"), "**/ should match many dirs");
    }

    #[cfg(windows)]
    #[test]
    fn glob_plus_and_pipe_are_escaped_windows() {
        let regex = convert_hgignore_glob("c++", Path::new("C:\\repo"), false).unwrap();
        assert!(regex.is_match("C:\\repo\\c++"), "+ should be literal");
        assert!(!regex.is_match("C:\\repo\\ccc"), "+ should not repeat");

        let regex = convert_hgignore_glob("a|b*", Path::new("C:\\repo"), false).unwrap();
        assert!(!regex.is_match("C:\\repo\\ab.txt"), "| should not alternate");
    }

    #[cfg(windows)]
    #[test]
    fn glob_double_star_slash_matches_zero_dirs_windows() {
        let regex = convert_hgignore_glob("**/foo", Path::new("C:\\repo"), false).unwrap();
        assert!(regex.is_match("C:\\repo\\foo"), "**/ should match zero dirs");
        assert!(regex.is_match("C:\\repo\\a\\b\\foo"), "**/ should match many dirs");
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
        assert!(
            regex.is_match("C:\\repo\\src\\main.rs"),
            "^-anchored pattern with / should match a native path"
        );
        assert!(!regex.is_match("C:\\repo\\srcmain.rs"), "should not match without separator");
    }

    #[cfg(windows)]
    #[test]
    fn regexp_slash_matches_native_separator_windows() {
        let regex = convert_hgignore_regexp("target/debug", Path::new("C:\\repo")).unwrap();
        assert!(
            regex.is_match("C:\\repo\\x\\target\\debug\\foo.o"),
            "unanchored regexp with / should match a native path"
        );

        let regex = convert_hgignore_regexp("^src\\/main", Path::new("C:\\repo")).unwrap();
        assert!(
            regex.is_match("C:\\repo\\src\\main.rs"),
            "escaped \\/ should also match the native separator"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn glob_matches_whole_component_not_prefix() {
        let regex = convert_hgignore_glob("foo", Path::new("/repo"), false).unwrap();
        assert!(regex.is_match("/repo/foo"));
        assert!(regex.is_match("/repo/sub/foo"), "hg globs are unrooted");
        assert!(
            regex.is_match("/repo/foo/inner.txt"),
            "a matched directory covers its contents"
        );
        assert!(!regex.is_match("/repo/foobar"), "must not match by prefix");
    }

    #[cfg(not(windows))]
    #[test]
    fn glob_repo_prefix_is_start_anchored() {
        let regex = convert_hgignore_glob("foo", Path::new("/repo"), false).unwrap();
        assert!(
            !regex.is_match("/elsewhere/repo/foo"),
            "repo prefix must match from the start of the path"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn regexp_repo_prefix_is_start_anchored() {
        let regex = convert_hgignore_regexp("foo", Path::new("/repo")).unwrap();
        assert!(regex.is_match("/repo/x/myfoo.txt"), "re.search semantics within the repo");
        assert!(!regex.is_match("/elsewhere/repo/x/foo"));
    }

    #[cfg(windows)]
    #[test]
    fn glob_matches_whole_component_not_prefix_windows() {
        let regex = convert_hgignore_glob("foo", Path::new("C:\\repo"), false).unwrap();
        assert!(regex.is_match("C:\\repo\\foo"));
        assert!(regex.is_match("C:\\repo\\sub\\foo"), "hg globs are unrooted");
        assert!(
            regex.is_match("C:\\repo\\foo\\inner.txt"),
            "a matched directory covers its contents"
        );
        assert!(!regex.is_match("C:\\repo\\foobar"), "must not match by prefix");
    }

    #[cfg(windows)]
    #[test]
    fn glob_repo_prefix_is_start_anchored_windows() {
        let regex = convert_hgignore_glob("foo", Path::new("C:\\repo"), false).unwrap();
        assert!(
            !regex.is_match("X:\\zzzC:\\repo\\foo"),
            "repo prefix must match from the start of the path"
        );
    }

    #[cfg(windows)]
    #[test]
    fn regexp_repo_prefix_is_start_anchored_windows() {
        let regex = convert_hgignore_regexp("foo", Path::new("C:\\repo")).unwrap();
        assert!(regex.is_match("C:\\repo\\x\\myfoo.txt"), "re.search semantics within the repo");
        assert!(!regex.is_match("X:\\zzzC:\\repo\\x\\foo"));
    }

    #[test]
    fn matches_hgignore_filter_returns_true_when_any_matches() {
        let filters = vec![
            HgignoreFilter::new(Regex::new("never_matches_xyz").unwrap()),
            HgignoreFilter::new(Regex::new("foo").unwrap()),
            HgignoreFilter::new(Regex::new("also_never_xyz").unwrap()),
        ];
        assert!(matches_hgignore_filter(&filters, "some/foo/path"));
    }

    #[test]
    fn matches_hgignore_filter_returns_false_when_none_match() {
        let filters = vec![
            HgignoreFilter::new(Regex::new("never_a").unwrap()),
            HgignoreFilter::new(Regex::new("never_b").unwrap()),
        ];
        assert!(!matches_hgignore_filter(&filters, "some/path"));
    }

    fn make_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn inline_comment_is_stripped() {
        let dir = make_test_dir("fselect_hg_inline_comment_test");
        let file_path = dir.join(".hgignore");
        std::fs::write(&file_path, "syntax: glob\n*.log # build logs\n").unwrap();

        let filters = parse_hgignore(&file_path, &dir).unwrap();
        assert_eq!(filters.len(), 1);
        let target = dir.join("x.log");
        assert!(
            matches_hgignore_filter(&filters, &target.to_string_lossy()),
            "inline comment and trailing whitespace should be stripped"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn escaped_hash_is_literal() {
        let dir = make_test_dir("fselect_hg_escaped_hash_test");
        let file_path = dir.join(".hgignore");
        std::fs::write(&file_path, "syntax: glob\nfoo\\#bar\n").unwrap();

        let filters = parse_hgignore(&file_path, &dir).unwrap();
        assert_eq!(filters.len(), 1);
        let target = dir.join("foo#bar");
        assert!(
            matches_hgignore_filter(&filters, &target.to_string_lossy()),
            "\\# should be an escaped literal hash, not a comment"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_syntax_directive_skips_line_only() {
        let dir = make_test_dir("fselect_hg_unknown_syntax_test");
        let file_path = dir.join(".hgignore");
        std::fs::write(&file_path, "syntax: glob\n*.log\nsyntax: bogus\n*.tmp\n").unwrap();

        let filters = parse_hgignore(&file_path, &dir)
            .expect("an unknown syntax directive should not fail the whole file");
        assert_eq!(filters.len(), 2, "patterns after the bad directive should survive");
        assert!(matches_hgignore_filter(
            &filters,
            &dir.join("x.log").to_string_lossy()
        ));
        assert!(
            matches_hgignore_filter(&filters, &dir.join("x.tmp").to_string_lossy()),
            "the previous syntax should stay in effect after the bad directive"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rootglob_is_root_anchored() {
        let dir = make_test_dir("fselect_hg_rootglob_test");
        let file_path = dir.join(".hgignore");
        std::fs::write(&file_path, "syntax: rootglob\nbuild\n").unwrap();

        let filters = parse_hgignore(&file_path, &dir).unwrap();
        assert_eq!(filters.len(), 1);
        assert!(matches_hgignore_filter(
            &filters,
            &dir.join("build").to_string_lossy()
        ));
        assert!(
            matches_hgignore_filter(&filters, &dir.join("build").join("o.txt").to_string_lossy()),
            "a matched directory covers its contents"
        );
        assert!(
            !matches_hgignore_filter(&filters, &dir.join("sub").join("build").to_string_lossy()),
            "rootglob patterns are anchored at the repo root"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn subinclude_resolves_relative_to_including_file() {
        let dir = make_test_dir("fselect_hg_subinclude_test");
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let file_path = dir.join(".hgignore");
        std::fs::write(&file_path, "subinclude:sub/.hgignore\n").unwrap();
        std::fs::write(sub.join(".hgignore"), "syntax: glob\n*.o\n").unwrap();

        let filters = parse_hgignore(&file_path, &dir).unwrap();
        assert_eq!(
            filters.len(),
            1,
            "the include path should resolve against the including file's directory, not the CWD"
        );
        assert!(matches_hgignore_filter(
            &filters,
            &sub.join("a.o").to_string_lossy()
        ));
        assert!(
            !matches_hgignore_filter(&filters, &dir.join("a.o").to_string_lossy()),
            "subincluded patterns are rooted at the subinclude file's directory"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
