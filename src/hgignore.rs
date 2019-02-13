use std::path::Path;

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

pub fn parse_hgignore(file_path: &Path, dir_path: &Path) -> Vec<HgignoreFilter> {
    let mut result = vec![];

    //TODO

    result
}