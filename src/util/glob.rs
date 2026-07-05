use std::ops::Index;
use std::sync::LazyLock;

use regex::Captures;
use regex::Regex;

static GLOB_SPECIAL_CHARS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(\\?|\\.|\\*|\\[|\\]|\\(|\\)|\\^|\\$|\\+|\\{|\\}|\\||\\\\)").unwrap()
});

pub fn is_glob(s: &str) -> bool {
    s.contains("*") || s.contains('?')
}

pub fn convert_glob_to_pattern(s: &str) -> Result<String, String> {
    let string = GLOB_SPECIAL_CHARS.replace_all(s, |c: &Captures| {
        match c.index(0) {
            "." => "\\.",
            "*" => ".*",
            "?" => ".",
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
            _ => "",
        }
        .to_string()
    });

    if string.is_empty() {
        return Err("Error parsing glob expression: ".to_string() + s);
    }

    Ok(format!("^(?i){}$", string))
}

pub fn convert_like_to_pattern(s: &str) -> Result<String, String> {
    fn push_literal(c: char, out: &mut String) {
        match c {
            '?' | '.' | '*' | '[' | ']' | '(' | ')' | '^' | '$' | '+' | '{' | '}' | '|' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }

    let mut string = String::with_capacity(s.len() + 8);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // MySQL default escape character: \_ , \% and \\ match the literal
            // character; a backslash before anything else stays a literal backslash
            '\\' => match chars.peek() {
                Some(&next @ ('_' | '%' | '\\')) => {
                    chars.next();
                    push_literal(next, &mut string);
                }
                _ => string.push_str("\\\\"),
            },
            '%' => string.push_str(".*"),
            '_' => string.push('.'),
            _ => push_literal(c, &mut string),
        }
    }

    if string.is_empty() {
        return Err("Error parsing LIKE expression: ".to_string() + s);
    }

    Ok(format!("^(?i){}$", string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_glob_with_asterisk() {
        assert!(is_glob("file*.txt"));
        assert!(is_glob("*file.txt"));
        assert!(is_glob("file.txt*"));
    }

    #[test]
    fn test_is_glob_with_question_mark() {
        assert!(is_glob("file?.txt"));
        assert!(is_glob("?file.txt"));
        assert!(is_glob("file.txt?"));
    }

    #[test]
    fn test_is_glob_with_no_glob_chars() {
        assert!(!is_glob("file.txt"));
        assert!(!is_glob("path/to/file.txt"));
        assert!(!is_glob(""));
    }

    #[test]
    fn test_convert_glob_to_pattern_asterisk() {
        let pattern = convert_glob_to_pattern("*.txt").unwrap();
        assert_eq!(pattern, "^(?i).*\\.txt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_question_mark() {
        let pattern = convert_glob_to_pattern("file?.txt").unwrap();
        assert_eq!(pattern, "^(?i)file.\\.txt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_mixed() {
        let pattern = convert_glob_to_pattern("file-*.?xt").unwrap();
        assert_eq!(pattern, "^(?i)file-.*\\..xt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_special_chars() {
        let pattern = convert_glob_to_pattern("file[1-3].txt").unwrap();
        assert_eq!(pattern, "^(?i)file\\[1-3\\]\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_percent() {
        let pattern = convert_like_to_pattern("%.txt").unwrap();
        assert_eq!(pattern, "^(?i).*\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_underscore() {
        let pattern = convert_like_to_pattern("file_.txt").unwrap();
        assert_eq!(pattern, "^(?i)file.\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_mixed() {
        let pattern = convert_like_to_pattern("file-%.txt").unwrap();
        assert_eq!(pattern, "^(?i)file-.*\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_question_mark() {
        let pattern = convert_like_to_pattern("file?.txt").unwrap();
        assert_eq!(pattern, "^(?i)file\\?\\.txt$");
    }

    #[test]
    fn test_convert_like_question_mark_is_literal() {
        // In SQL LIKE, '?' has no special meaning and should be treated as literal
        let pattern = convert_like_to_pattern("file?.txt").unwrap();
        let re = regex::Regex::new(&pattern).unwrap();
        assert!(re.is_match("file?.txt"));
        assert!(!re.is_match("filex.txt"));
        assert!(!re.is_match("file.txt"));
    }

    #[test]
    fn test_convert_like_to_pattern_special_chars() {
        let pattern = convert_like_to_pattern("file*.txt").unwrap();
        assert_eq!(pattern, "^(?i)file\\*\\.txt$");
    }

    #[test]
    fn test_convert_glob_escapes_plus() {
        let pattern = convert_glob_to_pattern("a+b.txt").unwrap();
        assert_eq!(pattern, "^(?i)a\\+b\\.txt$");
    }

    #[test]
    fn test_convert_glob_escapes_braces() {
        let pattern = convert_glob_to_pattern("file{1}.txt").unwrap();
        assert_eq!(pattern, "^(?i)file\\{1\\}\\.txt$");
    }

    #[test]
    fn test_convert_glob_escapes_pipe() {
        let pattern = convert_glob_to_pattern("a|b.txt").unwrap();
        assert_eq!(pattern, "^(?i)a\\|b\\.txt$");
    }

    #[test]
    fn test_convert_like_escapes_plus() {
        let pattern = convert_like_to_pattern("a+b").unwrap();
        assert_eq!(pattern, "^(?i)a\\+b$");
    }

    #[test]
    fn test_convert_like_escapes_braces() {
        let pattern = convert_like_to_pattern("file{1}").unwrap();
        assert_eq!(pattern, "^(?i)file\\{1\\}$");
    }

    #[test]
    fn test_convert_like_escaped_underscore() {
        let pattern = convert_like_to_pattern("a\\_b.txt").unwrap();
        assert_eq!(pattern, "^(?i)a_b\\.txt$");
        let re = regex::Regex::new(&pattern).unwrap();
        assert!(re.is_match("a_b.txt"));
        assert!(!re.is_match("axb.txt"));
    }

    #[test]
    fn test_convert_like_escaped_percent() {
        let pattern = convert_like_to_pattern("100\\%").unwrap();
        assert_eq!(pattern, "^(?i)100%$");
        let re = regex::Regex::new(&pattern).unwrap();
        assert!(re.is_match("100%"));
        assert!(!re.is_match("100200"));
    }

    #[test]
    fn test_convert_like_escaped_backslash() {
        let pattern = convert_like_to_pattern("a\\\\b").unwrap();
        assert_eq!(pattern, "^(?i)a\\\\b$");
        let re = regex::Regex::new(&pattern).unwrap();
        assert!(re.is_match("a\\b"));
        assert!(!re.is_match("ab"));
    }

    #[test]
    fn test_convert_like_backslash_before_other_char_is_literal() {
        // MySQL keeps the backslash when it does not precede a wildcard or backslash
        let pattern = convert_like_to_pattern("a\\b").unwrap();
        assert_eq!(pattern, "^(?i)a\\\\b$");
        let re = regex::Regex::new(&pattern).unwrap();
        assert!(re.is_match("a\\b"));
        assert!(!re.is_match("ab"));
    }

    #[test]
    fn test_convert_like_trailing_backslash_is_literal() {
        let pattern = convert_like_to_pattern("dir\\").unwrap();
        assert_eq!(pattern, "^(?i)dir\\\\$");
        let re = regex::Regex::new(&pattern).unwrap();
        assert!(re.is_match("dir\\"));
        assert!(!re.is_match("dir"));
    }

    #[test]
    fn test_convert_like_unescaped_wildcards_still_work() {
        let pattern = convert_like_to_pattern("a\\_b%").unwrap();
        let re = regex::Regex::new(&pattern).unwrap();
        assert!(re.is_match("a_b"));
        assert!(re.is_match("a_b-anything"));
        assert!(!re.is_match("aXb"));
    }
}
