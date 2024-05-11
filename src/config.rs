//! Handles configuration loading and saving

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use directories::ProjectDirs;

const ORGANIZATION: &str = "jhspetersson";
const APPLICATION: &str = "fselect";
const CONFIG_FILE: &str = "config.toml";

macro_rules! vec_of_strings {
    ($($str:literal),*) => {
        Some(vec![
            $(String::from($str)),*
        ])
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Config {
    pub no_color: Option<bool>,
    pub gitignore: Option<bool>,
    pub hgignore: Option<bool>,
    pub dockerignore: Option<bool>,
    pub is_zip_archive: Option<Vec<String>>,
    pub is_archive: Option<Vec<String>>,
    pub is_audio: Option<Vec<String>>,
    pub is_book: Option<Vec<String>>,
    pub is_doc: Option<Vec<String>>,
    pub is_font: Option<Vec<String>>,
    pub is_image: Option<Vec<String>>,
    pub is_source: Option<Vec<String>>,
    pub is_video: Option<Vec<String>>,
    pub default_file_size_format: Option<String>,
    pub check_for_updates: Option<bool>,
    #[serde(skip_serializing, default = "get_false")]
    pub debug: bool,
    #[serde(skip)]
    save: bool,
}

fn get_false() -> bool {
    false
}

impl Config {
    pub fn new() -> Result<Config, String> {
        let mut config_file;

        if let Some(cf) = Self::get_current_dir_config() {
            config_file = cf;
        } else {
            let config_dir = Self::get_project_dir();

            if config_dir.is_none() {
                return Ok(Config::default());
            }

            config_file = config_dir.unwrap();
            config_file.push(CONFIG_FILE);

            if !config_file.exists() {
                return Ok(Config::default());
            }
        }

        Config::from(config_file)
    }

    pub fn from(config_file: PathBuf) -> Result<Config, String> {
        if let Ok(mut file) = fs::File::open(config_file) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                toml::from_str(&contents).map_err(|err| err.to_string())
            } else {
                Err("Could not read config file. Using default settings.".to_string())
            }
        } else {
            Err("Could not open config file. Using default settings.".to_string())
        }
    }

    fn get_current_dir_config() -> Option<PathBuf> {
        if let Ok(mut pb) = std::env::current_exe() {
            pb.pop();
            pb.push(CONFIG_FILE);
            if pb.exists() {
                return Some(pb);
            }
        }

        None
    }

    #[cfg(not(windows))]
    fn get_project_dir() -> Option<PathBuf> {
        ProjectDirs::from("", ORGANIZATION, APPLICATION).map(|pd| pd.config_dir().to_path_buf())
    }

    #[cfg(windows)]
    fn get_project_dir() -> Option<PathBuf> {
        ProjectDirs::from("", ORGANIZATION, APPLICATION)
            .map(|pd| pd.config_dir().parent().unwrap().to_path_buf())
    }

    pub fn save(&self) {
        if !self.save {
            return;
        }

        let config_dir = Self::get_project_dir();

        if config_dir.is_none() {
            return;
        }

        let mut config_file = config_dir.unwrap();
        let _ = fs::create_dir_all(&config_file);
        config_file.push(CONFIG_FILE);

        if config_file.exists() {
            return;
        }

        let toml = toml::to_string_pretty(&self).unwrap();

        if let Ok(mut file) = fs::File::create(&config_file) {
            let _ = file.write_all(toml.as_bytes());
        }
    }

    pub fn default() -> Config {
        Config {
            no_color: Some(false),
            gitignore: Some(false),
            hgignore: Some(false),
            dockerignore: Some(false),
            is_zip_archive: vec_of_strings![".zip", ".jar", ".war", ".ear"],
            is_archive: vec_of_strings![
                ".7z", ".bz2", ".bzip2", ".gz", ".gzip", ".lz", ".rar", ".tar", ".xz", ".zip"
            ],
            is_audio: vec_of_strings![
                ".aac", ".aiff", ".amr", ".flac", ".gsm", ".m4a", ".m4b", ".m4p", ".mp3", ".ogg",
                ".wav", ".wma"
            ],
            is_book: vec_of_strings![
                ".azw3", ".chm", ".djv", ".djvu", ".epub", ".fb2", ".mobi", ".pdf"
            ],
            is_doc: vec_of_strings![
                ".accdb", ".doc", ".docm", ".docx", ".dot", ".dotm", ".dotx", ".mdb", ".odp",
                ".ods", ".odt", ".pdf", ".potm", ".potx", ".ppt", ".pptm", ".pptx", ".rtf", ".xlm",
                ".xls", ".xlsm", ".xlsx", ".xlt", ".xltm", ".xltx", ".xps"
            ],
            is_font: vec_of_strings![
                ".eot", ".fon", ".otc", ".otf", ".ttc", ".ttf", ".woff", ".woff2"
            ],
            is_image: vec_of_strings![
                ".bmp", ".exr", ".gif", ".heic", ".jpeg", ".jpg", ".jxl", ".png", ".psb", ".psd",
                ".svg", ".tga", ".tiff", ".webp"
            ],
            is_source: vec_of_strings![
                ".asm", ".bas", ".c", ".cc", ".ceylon", ".clj", ".coffee", ".cpp", ".cs", ".d",
                ".dart", ".elm", ".erl", ".go", ".gradle", ".groovy", ".h", ".hh", ".hpp", ".java",
                ".jl", ".js", ".jsp", ".jsx", ".kt", ".kts", ".lua", ".nim", ".pas", ".php", ".pl",
                ".pm", ".py", ".rb", ".rs", ".scala", ".sol", ".swift", ".tcl", ".ts", ".tsx",
                ".vala", ".vb", ".zig"
            ],
            is_video: vec_of_strings![
                ".3gp", ".avi", ".flv", ".m4p", ".m4v", ".mkv", ".mov", ".mp4", ".mpeg", ".mpg",
                ".webm", ".wmv"
            ],
            default_file_size_format: Some(String::new()),
            check_for_updates: Some(false),
            debug: false,
            save: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = Config::default();

        assert!(config.is_source.unwrap().contains(&String::from(".rs")));
    }
}
