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
    H265,
    Av1,
    Vp9,
}

impl CodecPreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Авто",
            Self::H264 => "H264",
            Self::H265 => "H265",
            Self::Av1 => "AV1",
            Self::Vp9 => "VP9",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EncoderPreference {
    #[default]
    Auto,
    Nvenc,
    Software,
}

impl EncoderPreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Авто",
            Self::Nvenc if cfg!(target_os = "macos") => "VideoToolbox",
            Self::Nvenc => "NVENC",
            Self::Software => "Software",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FsrQualitySetting {
    /// FSR выключен — захват в нативном разрешении.
    #[default]
    Off,
    /// Нативное разрешение + только RCAS обострение.
    Native,
    /// 77% от нативного → апскейл 1.3×.
    UltraQuality,
    /// 67% от нативного → апскейл 1.5× (рекомендуется).
    Quality,
    /// 59% от нативного → апскейл 1.7×.
    Balanced,
    /// 50% от нативного → апскейл 2×.
    Performance,
}

impl FsrQualitySetting {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Выключен",
            Self::Native => "Native (только RCAS)",
            Self::UltraQuality => "Ultra Quality (1.3×)",
            Self::Quality => "Quality (1.5×)",
            Self::Balanced => "Balanced (1.7×)",
            Self::Performance => "Performance (2×)",
        }
    }

    /// Конвертация в enum из крейта fsr.
    pub fn to_fsr_quality(self) -> Option<crate::fsr::FsrQuality> {
        match self {
            Self::Off => None,
            Self::Native => Some(crate::fsr::FsrQuality::Native),
            Self::UltraQuality => Some(crate::fsr::FsrQuality::UltraQuality),
            Self::Quality => Some(crate::fsr::FsrQuality::Quality),
            Self::Balanced => Some(crate::fsr::FsrQuality::Balanced),
            Self::Performance => Some(crate::fsr::FsrQuality::Performance),
        }
    }

    pub fn is_enabled(self) -> bool {
        self != Self::Off
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StreamingMode {
    #[default]
    Support,
    Interactive,
    Game,
}

impl StreamingMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Support => "Support",
            Self::Interactive => "Interactive",
            Self::Game => "Game",
        }
    }

    pub fn allows_static_skip(self) -> bool {
        !matches!(self, Self::Game)
    }
}

fn default_fsr_sharpness() -> f32 {
    0.875
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default)]
    pub codec: CodecPreference,
    #[serde(default)]
    pub encoder: EncoderPreference,
    #[serde(default = "default_target_fps")]
    pub target_fps: u32,
    #[serde(default = "default_adaptive_quality")]
    pub adaptive_quality: bool,
    #[serde(default = "default_min_fps")]
    pub min_fps: u32,
    #[serde(default)]
    pub streaming_mode: StreamingMode,

    /// AMD FidelityFX Super Resolution — режим качества апскейла.
    /// `Off` = FSR не используется (нативный захват).
    #[serde(default)]
    pub fsr_quality: FsrQualitySetting,

    /// Сила обострения RCAS: 0.0 = максимум, 1.0 = выключено.
    /// Применяется только когда `fsr_quality != Off`.
    #[serde(default = "default_fsr_sharpness")]
    pub fsr_sharpness: f32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            codec: CodecPreference::Auto,
            encoder: EncoderPreference::Auto,
            target_fps: default_target_fps(),
            adaptive_quality: default_adaptive_quality(),
            min_fps: default_min_fps(),
            streaming_mode: StreamingMode::Support,
            fsr_quality: FsrQualitySetting::Off,
            fsr_sharpness: default_fsr_sharpness(),
        }
    }
}

// ── LLM terminal assistant configuration ──────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    #[default]
    Ollama,
    OpenAi,
    YandexGpt,
}

impl LlmProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::OpenAi => "OpenAI",
            Self::YandexGpt => "YandexGPT",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: LlmProvider,
    #[serde(default = "default_llm_openai_base_url")]
    pub openai_base_url: String,
    #[serde(default)]
    pub openai_api_key: String,
    #[serde(default = "default_llm_openai_model")]
    pub openai_model: String,
    #[serde(default = "default_llm_yandex_base_url")]
    pub yandex_base_url: String,
    #[serde(default)]
    pub yandex_api_key: String,
    #[serde(default)]
    pub yandex_folder_id: String,
    #[serde(default = "default_llm_yandex_model_uri")]
    pub yandex_model_uri: String,
    #[serde(default = "default_llm_ollama_base_url")]
    pub ollama_base_url: String,
    #[serde(default = "default_llm_ollama_model")]
    pub ollama_model: String,
    #[serde(default = "default_llm_system_prompt")]
    pub system_prompt: String,
    #[serde(default = "default_llm_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_llm_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub auto_suggest: bool,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: LlmProvider::default(),
            openai_base_url: default_llm_openai_base_url(),
            openai_api_key: String::new(),
            openai_model: default_llm_openai_model(),
            yandex_base_url: default_llm_yandex_base_url(),
            yandex_api_key: String::new(),
            yandex_folder_id: String::new(),
            yandex_model_uri: default_llm_yandex_model_uri(),
            ollama_base_url: default_llm_ollama_base_url(),
            ollama_model: default_llm_ollama_model(),
            system_prompt: default_llm_system_prompt(),
            max_tokens: default_llm_max_tokens(),
            temperature: default_llm_temperature(),
            auto_suggest: false,
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
    #[serde(default = "default_true")]
    pub show_connection_details: bool,
    #[serde(default)]
    pub history: Vec<ConnectionHistoryEntry>,
    #[serde(default)]
    pub contacts: Vec<ContactEntry>,
    #[serde(default)]
    pub address_book_signed_in: bool,
    #[serde(default)]
    pub address_book_account: String,
    #[serde(default)]
    pub address_book_token: String,
    #[serde(default)]
    pub address_book_access_token: String,
    #[serde(default)]
    pub address_book_guid: String,
    #[serde(default)]
    pub agent_machine_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConnectionHistoryEntry {
    pub remote_id: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub last_connected_unix: u64,
    #[serde(default)]
    pub connect_count: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContactEntry {
    pub name: String,
    pub remote_id: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub last_seen: String,
    #[serde(default)]
    pub online: bool,
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
            show_connection_details: true,
            history: Vec::new(),
            contacts: Vec::new(),
            address_book_signed_in: false,
            address_book_account: String::new(),
            address_book_token: String::new(),
            address_book_access_token: String::new(),
            address_book_guid: String::new(),
            agent_machine_id: String::new(),
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
    pub llm: LlmConfig,
    #[serde(default)]
    pub ui: UiConfig,
    /// Optional: bind the hbbs UDP socket to this specific local port.
    #[serde(default)]
    pub udp_bind_port: u16,

    /// Порт для выделенного EVRT UDP сокета (прямой стриминг).
    /// 0 = случайный свободный порт.
    /// Рекомендуется зафиксировать (например 45123) чтобы открыть правило файрвола.
    #[serde(default)]
    pub evrt_udp_port: u16,
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
                if config.ui.agent_machine_id.trim().is_empty() {
                    config.ui.agent_machine_id = generate_agent_machine_id();
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
            llm: LlmConfig::default(),
            ui: UiConfig::default(),
            udp_bind_port: 0,
            evrt_udp_port: 0,
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
    60
}
fn default_adaptive_quality() -> bool {
    true
}
fn default_min_fps() -> u32 {
    15
}
fn default_true() -> bool {
    true
}
fn default_llm_openai_base_url() -> String {
    "https://api.openai.com/v1/chat/completions".to_owned()
}
fn default_llm_openai_model() -> String {
    "gpt-4o-mini".to_owned()
}
fn default_llm_yandex_base_url() -> String {
    "https://llm.api.cloud.yandex.net/foundationModels/v1/completion".to_owned()
}
fn default_llm_yandex_model_uri() -> String {
    "gpt://{folder_id}/yandexgpt/latest".to_owned()
}
fn default_llm_ollama_base_url() -> String {
    "http://localhost:11434".to_owned()
}
fn default_llm_ollama_model() -> String {
    "llama3.1".to_owned()
}
fn default_llm_system_prompt() -> String {
    "Ты встроенный помощник терминала EvertyDesk Lite. Отвечай коротко, по делу, на русском. Если предлагаешь команду, объясни риск и не предлагай разрушительные действия без явного предупреждения.".to_owned()
}
fn default_llm_max_tokens() -> u32 {
    700
}
fn default_llm_temperature() -> f32 {
    0.2
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

pub fn generate_agent_machine_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
