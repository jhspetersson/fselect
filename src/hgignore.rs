use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::ops::Add;
use std::ops::Index;
use std::path::Path;

use regex::Captures;
use regex::Error;
use regex::Regex;

#[derive(Clone, Debug)]
pub struct HgignoreFilter {
    pub regex: Regex,
}

impl HgignoreFilter {
    fn new(regex: Regex) -> HgignoreFilter {
        HgignoreFilter {
            regex
        }
    }
}

enum Syntax {
    Regexp, Glob
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

pub fn parse_hgignore(file_path: &Path, dir_path: &Path) -> Result<Vec<HgignoreFilter>, String> {
    let mut result = vec![];

    if let Ok(file) = File::open(file_path) {
        let mut syntax = Syntax::Regexp;

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
                    Ok(line) => {
                        if line.starts_with("syntax:") {
                            let line = line.replace("syntax:", "");
                            let syntax_directive = line.trim();
                            syntax = Syntax::from(syntax_directive).unwrap();
                        } else if line.starts_with("subinclude:") {
                            //TODO
                        } else {
                            let pattern = convert_hgignore_pattern(&line, dir_path, &syntax).unwrap();
                            result.push(pattern);
                        }
                    },
                    _ => { }
                }
            });
    };

    Ok(result)
}

fn convert_hgignore_pattern(pattern: &str, file_path: &Path, syntax: &Syntax) -> Result<HgignoreFilter, String> {
    match syntax {
        Syntax::Glob => {
            match convert_hgignore_glob(pattern, file_path) {
                Ok(regex) => Ok(HgignoreFilter::new(regex)),
                _ => Err("Error creating regex while parsing .hgignore pattern: ".to_string() + pattern)
            }
        },
        Syntax::Regexp => Err("Not supported".to_string())
    }
}

fn convert_hgignore_glob(glob: &str, file_path: &Path) -> Result<Regex, Error> {
    #[cfg(not(windows))]
        {
            let replace_regex = Regex::new("(\\*\\*|\\?|\\.|\\*|\\[|\\]|\\(|\\)|\\^|\\$)").unwrap();
            let mut pattern = replace_regex.replace_all(&glob, |c: &Captures| {
                match c.index(0) {
                    "**" => ".*",
                    "." => "\\.",
                    "*" => "[^/]*",
                    "?" => "[^/]+",
                    "[" => "\\[",
                    "]" => "\\]",
                    "(" => "\\(",
                    ")" => "\\)",
                    "^" => "\\^",
                    "$" => "\\$",
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
            let replace_regex = Regex::new("(\\*\\*|\\?|\\.|\\*|\\[|\\]|\\(|\\)|\\^|\\$)").unwrap();
            let mut pattern = replace_regex.replace_all(&glob, |c: &Captures| {
                match c.index(0) {
                    "**" => ".*",
                    "." => "\\.",
                    "*" => "[^\\\\]*",
                    "?" => "[^\\\\]+",
                    "[" => "\\[",
                    "]" => "\\]",
                    "(" => "\\(",
                    ")" => "\\)",
                    "^" => "\\^",
                    "$" => "\\$",
                    _ => panic!("Error parsing pattern")
                }.to_string()
            }).to_string();

            pattern = file_path.to_string_lossy().to_string()
                .replace("\\", "\\\\")
                .add("\\\\([^\\\\]+\\\\)*").add(&pattern);

            Regex::new(&pattern)
        }
}
