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
        .and_then(|pd| pd.config_dir().parent().map(|p| p.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_project_dir_does_not_panic() {
        // Should return Some on most systems, but must never panic
        let _ = get_project_dir();
    }
}