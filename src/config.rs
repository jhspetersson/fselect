use std::fs;
use std::io::{Read, Write};

use app_dirs::{AppInfo, AppDataType, app_root};

const APP_INFO: AppInfo = AppInfo {
    name: "fselect",
    author: "jhspetersson",
};

const CONFIG_FILE: &str = "config.toml";

#[derive(Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct Config {
    pub no_color : bool,
}

impl Config {
    pub fn new() -> Config {
        let config_dir = app_root(AppDataType::UserConfig, &APP_INFO);

        if config_dir.is_err() {
            return Config::default();
        }

        let mut config_file = config_dir.unwrap().clone();
        config_file.push(CONFIG_FILE);

        if !config_file.exists() {
            return Config::default();
        }

        if let Ok(mut file) = fs::File::open(&config_file) {
            let mut contents = String::new();
            if let Ok(_) = file.read_to_string(&mut contents) {
                let config: Config = toml::from_str(&contents).unwrap();
                return config;
            }
        }

        Config::default()
    }

    pub fn save(&self) {
        let config_dir = app_root(AppDataType::UserConfig, &APP_INFO);

        if config_dir.is_err() {
            return;
        }

        let mut config_file = config_dir.unwrap().clone();
        config_file.push(CONFIG_FILE);

        if config_file.exists() {
            return;
        }

        let toml = toml::to_vec(&self).unwrap();

        if let Ok(mut file) = fs::File::create(&config_file) {
            let _ = file.write_all(&toml);
        }
    }
}

