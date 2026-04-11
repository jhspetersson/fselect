use std::path::PathBuf;
#[cfg(feature = "interactive")]
use directories::ProjectDirs;

#[cfg(feature = "interactive")]
const ORGANIZATION: &str = "jhspetersson";
#[cfg(feature = "interactive")]
const APPLICATION: &str = "fselect";

#[cfg(all(not(windows), feature = "interactive"))]
pub(crate) fn get_project_dir() -> Option<PathBuf> {
    ProjectDirs::from("", ORGANIZATION, APPLICATION).map(|pd| pd.config_dir().to_path_buf())
}

#[cfg(all(windows, feature = "interactive"))]
pub(crate) fn get_project_dir() -> Option<PathBuf> {
    ProjectDirs::from("", ORGANIZATION, APPLICATION)
        .and_then(|pd| pd.config_dir().parent().map(|p| p.to_path_buf()))
}

#[cfg(not(feature = "interactive"))]
pub(crate) fn get_project_dir() -> Option<PathBuf> {
    None
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
