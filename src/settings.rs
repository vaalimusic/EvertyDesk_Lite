use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

// ── Server configuration ─────────────────────────────────────────────────────

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

// ── Security configuration ───────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Show a confirmation dialog before accepting an incoming remote session.
    #[serde(default)]
    pub require_confirmation: bool,
    /// Let the remote side send mouse / keyboard events.
    #[serde(default = "default_true")]
    pub allow_keyboard_mouse: bool,
    /// Let the remote side read / paste the local clipboard.
    #[serde(default = "default_true")]
    pub allow_clipboard: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            require_confirmation: false,
            allow_keyboard_mouse: true,
            allow_clipboard: true,
        }
    }
}

// ── Display / codec configuration ────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CodecPreference {
    #[default]
    Auto,
    H264,
    Vp9,
}

impl CodecPreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Авто",
            Self::H264 => "H264",
            Self::Vp9 => "VP9",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default)]
    pub codec: CodecPreference,
    #[serde(default = "default_target_fps")]
    pub target_fps: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            codec: CodecPreference::Auto,
            target_fps: 30,
        }
    }
}

// ── UI / session configuration ───────────────────────────────────────────────

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

// ── Root configuration ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub local_id: String,
    pub local_password: String,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub ui: UiConfig,
    /// Optional: bind the hbbs UDP socket to this specific local port.
    /// Useful when `--bind-port` is passed on the CLI so we can reuse the
    /// port that was previously registered with the ID server.
    #[serde(default)]
    pub udp_bind_port: u16,
    /// Optional: 32-byte Ed25519 public key (raw bytes) to send in
    /// RegisterPk instead of the SHA-256 derived fake key.  Set by
    /// `--use-everty-keys` to pick up the key from the installed EvertyDesk.
    #[serde(default)]
    pub host_pk: Vec<u8>,
    /// Stable Ed25519 *sign* public key (32 bytes) identifying this host.
    /// Registered with the rendezvous server and used by peers to verify the
    /// host's `SignedId` during the secure handshake.
    #[serde(default)]
    pub host_sign_pk: Vec<u8>,
    /// Matching Ed25519 *sign* secret key (64 bytes) — used to sign `SignedId`.
    #[serde(default)]
    pub host_sign_sk: Vec<u8>,
}

impl AppConfig {
    pub fn load_or_create() -> Self {
        let path = config_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(mut config) = serde_json::from_str::<Self>(&raw) {
                // Lazily generate the stable sign key pair for older configs.
                if config.host_sign_pk.len() != 32 || config.host_sign_sk.len() != 64 {
                    let (pk, sk) = crate::crypto::gen_sign_keypair();
                    config.host_sign_pk = pk;
                    config.host_sign_sk = sk;
                    config.save();
                }
                return config;
            }
        }

        let (sign_pk, sign_sk) = crate::crypto::gen_sign_keypair();
        let config = Self {
            server: ServerConfig::default(),
            local_id: generate_numeric_token(9),
            local_password: generate_numeric_token(6),
            security: SecurityConfig::default(),
            display: DisplayConfig::default(),
            ui: UiConfig::default(),
            udp_bind_port: 0,
            host_pk: Vec::new(),
            host_sign_pk: sign_pk,
            host_sign_sk: sign_sk,
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

// ── Private helpers ───────────────────────────────────────────────────────────

fn default_auto_refresh() -> bool {
    true
}
fn default_refresh_millis() -> u64 {
    80
}
fn default_fit_to_window() -> bool {
    true
}
fn default_target_fps() -> u32 {
    30
}
fn default_true() -> bool {
    true
}

pub fn config_path() -> PathBuf {
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
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut value = nanos ^ (std::process::id() as u128);
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        out.push(char::from(b'0' + (value % 10) as u8));
        value = value / 10 + 17;
    }
    out
}
