use std::path::PathBuf;
use directories::ProjectDirs;

const ORGANIZATION: &str = "jhspetersson";
const APPLICATION: &str = "fselect";

#[cfg(not(windows))]
pub(crate) fn get_project_dir() -> Option<PathBuf> {
    ProjectDirs::from("", ORGANIZATION, APPLICATION).map(|pd| pd.config_dir().to_path_buf())
}

#[cfg(windows)]
pub(crate) fn get_project_dir() -> Option<PathBuf> {
    ProjectDirs::from("", ORGANIZATION, APPLICATION)
        .map(|pd| pd.config_dir().parent().unwrap().to_path_buf())
}