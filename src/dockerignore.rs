use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::path::Path;

use regex::Error;
use regex::Regex;

#[derive(Clone, Debug)]
pub struct DockerignoreFilter {
    pub regex: Regex,
}

impl DockerignoreFilter {
    fn new(regex: Regex) -> DockerignoreFilter {
        DockerignoreFilter {
            regex
        }
    }
}

pub fn matches_dockerignore_filter(dockerignore_filters: &Vec<DockerignoreFilter>, file_name: &str) -> bool {
    let mut matched = false;

    for dockerignore_filter in dockerignore_filters {
        let is_match = dockerignore_filter.regex.is_match(file_name);

        if is_match {
            matched = true;
        }
    }

    matched
}

pub fn parse_dockerignore(file_path: &Path, dir_path: &Path) -> Result<Vec<DockerignoreFilter>, String> {
    let mut result = vec![];
    let mut err = String::new();

    if let Ok(file) = File::open(file_path) {
        let reader = BufReader::new(file);
        reader.lines()
            .filter(|line| {
                match line {
                    Ok(line) => !line.trim().is_empty() && !line.starts_with("#"),
                    _ => false
                }
            })
            .for_each(|line| {
                if err.is_empty() {
                    match line {
                        Ok(line) => {
                            let pattern = convert_dockerignore_pattern(&line, dir_path);
                            match pattern {
                                Ok(pattern) => result.push(pattern),
                                Err(parse_err) => err = parse_err
                            }
                        },
                        _ => { }
                    }
                }
            });
    };

    match err.is_empty() {
        true => Ok(result),
        false => Err(err)
    }
}

fn convert_dockerignore_pattern(pattern: &str, file_path: &Path) -> Result<DockerignoreFilter, String> {
    match convert_dockerignore_glob(pattern, file_path) {
        Ok(regex) => Ok(DockerignoreFilter::new(regex)),
        _ => Err("Error creating regex while parsing .dockerignore glob: ".to_string() + pattern)
    }
}

fn convert_dockerignore_glob(_glob: &str, _file_path: &Path) -> Result<Regex, Error> {
    Err(Error::Syntax(String::from("Not implemented")))
}