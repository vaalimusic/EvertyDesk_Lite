use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub api_url: String,
    pub id_server: String,
    pub relay_server: String,
    pub public_key: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            api_url: "https://desk.everty.ru".to_owned(),
            id_server: "edesk.server1.everty.ru".to_owned(),
            relay_server: "edesk.server1.everty.ru".to_owned(),
            public_key: "MrGdbay3g8Qr84YYnxr4qLjw5zLWM1oAOdfehbBnlRs=".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub local_id: String,
    pub local_password: String,
    #[serde(default)]
    pub ui: UiConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub last_remote_id: String,
    #[serde(default)]
    pub recent_remote_ids: Vec<String>,
    #[serde(default = "default_auto_refresh")]
    pub auto_refresh: bool,
    #[serde(default = "default_refresh_millis")]
    pub refresh_millis: u64,
    #[serde(default = "default_fit_to_window")]
    pub fit_to_window: bool,
    #[serde(default)]
    pub coordinate_mode: CoordinateMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoordinateMode {
    Auto,
    Absolute,
    Local,
}

impl Default for CoordinateMode {
    fn default() -> Self {
        Self::Auto
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            last_remote_id: String::new(),
            recent_remote_ids: Vec::new(),
            auto_refresh: default_auto_refresh(),
            refresh_millis: default_refresh_millis(),
            fit_to_window: default_fit_to_window(),
            coordinate_mode: CoordinateMode::default(),
        }
    }
}

impl AppConfig {
    pub fn load_or_create() -> Self {
        let path = config_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(config) = serde_json::from_str(&raw) {
                return config;
            }
        }

        let config = Self {
            server: ServerConfig::default(),
            local_id: generate_numeric_token(9),
            local_password: generate_numeric_token(6),
            ui: UiConfig::default(),
        };

        config.save();
        config
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(raw) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, raw);
        }
    }
}

fn default_auto_refresh() -> bool {
    true
}

fn default_refresh_millis() -> u64 {
    80
}

fn default_fit_to_window() -> bool {
    true
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("EvertyDesk Lite").join("config.json")
}

pub fn generate_numeric_token(len: usize) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut value = nanos ^ (std::process::id() as u128);
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        out.push(char::from(b'0' + (value % 10) as u8));
        value = value / 10 + 17;
    }
    out
}
