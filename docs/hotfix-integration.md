# EvertyDesk Lite — Интеграция AI Hotfix Pipeline

**Язык:** Rust  
**GUI:** eframe / egui  
**HTTP:** ureq (уже в Cargo.toml)  
**Crypto:** dryoc (уже в Cargo.toml)  
**Версия протокола:** 1

---

## Обзор

Добавить один новый модуль `src/hotfix.rs` плюс минимальные правки в `settings.rs`, `lib.rs` и UI.

Полный цикл:
```
[Компонент упал] → hotfix::report(incident) → POST /api/v1/incidents
                                               ↓ фоновый поток polling
                                         GET .../analysis
                                               ↓
                                         GET .../remediation-plans/{id}
                                               ↓ верификация
                                         egui диалог согласия
                                               ↓
                                         apply_actions() → POST .../applied
                                               ↓ 10 мин
                                         POST .../result
                                               ↓ TTL истёк
                                         rollback() → POST .../rollback
```

---

## 1. Изменения в settings.rs

Добавить в конец блока `use` и определений:

```rust
// ── AI Hotfix Pipeline configuration ─────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HotfixConfig {
    /// Включить отправку инцидентов и получение планов.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Bearer-токен из переменной HOTFIX_API_KEY на сервере.
    /// Пустая строка — принимает сервер без ключа (dev mode).
    #[serde(default)]
    pub api_key: String,
    /// Base64 (standard, 32 байта) публичного Ed25519-ключа сервера.
    /// Извлечь: берём env HOTFIX_SIGNING_KEY_PRIVATE с сервера,
    /// декодируем base64 (64 байта), последние 32 байта — это публичный ключ,
    /// кодируем их обратно в base64 и вставляем сюда.
    #[serde(default)]
    pub signing_public_key_b64: String,
    /// Не отправлять один и тот же тип инцидента чаще этого интервала (сек).
    #[serde(default = "default_hotfix_rate_limit_secs")]
    pub rate_limit_secs: u64,
}

fn default_hotfix_rate_limit_secs() -> u64 { 300 }

impl Default for HotfixConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: String::new(),
            signing_public_key_b64: String::new(),
            rate_limit_secs: default_hotfix_rate_limit_secs(),
        }
    }
}
```

Добавить поле в `AppConfig`:

```rust
pub struct AppConfig {
    // ... существующие поля ...
    #[serde(default)]
    pub hotfix: HotfixConfig,
}
```

И инициализацию в `AppConfig::load_or_create` (дефолт подхватится автоматически через `#[serde(default)]`).

---

## 2. Создать src/hotfix.rs

```rust
//! AI Hotfix Pipeline client — EvertyDesk Lite.
//!
//! Точка входа: [`report`]. Остальное — внутренняя машинерия.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::settings::{AppConfig, HotfixConfig};

// ── Константы ─────────────────────────────────────────────────────────────────

const POLL_INITIAL_SECS: u64 = 10;
const POLL_MAX_SECS:     u64 = 60;
const POLL_TIMEOUT_SECS: u64 = 180;
const HTTP_TIMEOUT:      Duration = Duration::from_secs(30);

// ── Allowed actions registry ──────────────────────────────────────────────────
// Клиент должен отклонить любой op, которого нет здесь.

const ALLOWED_OPS: &[&str] = &[
    "set_config",
    "restore_config",
    "disable_feature",
    "enable_safe_mode",
    "force_fallback",
    "lower_quality",
    "lower_fps",
    "set_evrt_policy",
    "set_renderer_policy",
    "set_clipboard_policy_more_restrictive",
    "collect_diagnostics",
    "restart_component_soft",
    "rollback_plugin",
    "disable_provider_backend",
    "disable_native_console_backend",
    "clear_component_cache",
];

// ── Типы инцидентов ───────────────────────────────────────────────────────────

/// Описание инцидента — передаётся из компонента, который упал.
#[derive(Clone, Debug)]
pub struct Incident {
    /// Тип: "crash", "degradation", "connection_failure", и т.д.
    pub incident_type: &'static str,
    /// "error" | "warning" | "critical"
    pub severity: &'static str,
    /// "renderer" | "evrt" | "clipboard" | "vm_console" | "service_mode"
    pub component: &'static str,
    /// Короткий код: "RENDERER_GL_CONTEXT_INIT_FAILED"
    pub error_code: &'static str,
    /// Читаемое сообщение — не содержит PII
    pub message: String,
    /// Стабильный snake_case идентификатор: "renderer.gl_context.init.failed"
    pub crash_signature: &'static str,
}

// ── Shared state ──────────────────────────────────────────────────────────────

struct HotfixState {
    /// rate limiting: crash_signature → последнее время отправки
    last_sent: HashMap<&'static str, Instant>,
    /// Планы, применённые на этом запуске (plan_id → ActivePlan)
    active_plans: HashMap<String, ActivePlan>,
    /// Уведомление UI: план готов и ждёт согласия пользователя
    pub pending_consent: Option<RemediationPlan>,
    /// Планы, которые пользователь уже одобрил, ожидают apply
    pub pending_apply: Vec<RemediationPlan>,
}

impl HotfixState {
    fn new() -> Self {
        Self {
            last_sent: HashMap::new(),
            active_plans: HashMap::new(),
            pending_consent: None,
            pending_apply: Vec::new(),
        }
    }
}

static STATE: OnceLock<Arc<Mutex<HotfixState>>> = OnceLock::new();

fn state() -> Arc<Mutex<HotfixState>> {
    STATE.get_or_init(|| Arc::new(Mutex::new(HotfixState::new()))).clone()
}

// ── Публичный API ─────────────────────────────────────────────────────────────

/// Вызвать из любого компонента при ошибке.
/// Неблокирующий — запускает фоновый поток.
///
/// # Пример
/// ```rust
/// hotfix::report(Incident {
///     incident_type:   "crash",
///     severity:        "error",
///     component:       "renderer",
///     error_code:      "RENDERER_GL_CONTEXT_INIT_FAILED",
///     message:         "OpenGL context initialization failed".into(),
///     crash_signature: "renderer.gl_context.init.failed",
/// }, &config);
/// ```
pub fn report(incident: Incident, config: &AppConfig) {
    if !config.hotfix.enabled {
        return;
    }

    // Rate limiting
    {
        let mut st = state().lock().unwrap();
        let limit = Duration::from_secs(config.hotfix.rate_limit_secs);
        if let Some(last) = st.last_sent.get(incident.crash_signature) {
            if last.elapsed() < limit {
                return;
            }
        }
        st.last_sent.insert(incident.crash_signature, Instant::now());
    }

    let hotfix_cfg = config.hotfix.clone();
    let api_url = config.server.api_url.trim_end_matches('/').to_owned();
    let device_id = config.ui.agent_machine_id.clone();
    let tenant_id = String::new(); // заполнить из service_key при наличии

    thread::Builder::new()
        .name(format!("hotfix-{}", incident.crash_signature))
        .spawn(move || {
            run_pipeline(incident, &api_url, &device_id, &tenant_id, &hotfix_cfg);
        })
        .ok();
}

/// Вызвать из UI loop (egui update) — возвращает план, ожидающий согласия.
/// UI должен показать диалог и вызвать `user_accepted` или `user_declined`.
pub fn take_pending_consent() -> Option<RemediationPlan> {
    state().lock().unwrap().pending_consent.take()
}

/// Пользователь нажал "Применить" в диалоге.
pub fn user_accepted(plan: RemediationPlan, config: &AppConfig) {
    let cfg = config.clone();
    let api_url = config.server.api_url.trim_end_matches('/').to_owned();
    thread::spawn(move || {
        apply_plan(plan, &api_url, &cfg);
    });
}

/// Пользователь нажал "Не сейчас".
pub fn user_declined(plan: &RemediationPlan, config: &AppConfig) {
    report_decision(&plan.plan_id, "rejected", "user", &config.server.api_url, &config.hotfix);
}

/// Вызвать из UI loop — проверяет истёкшие TTL и откатывает их.
pub fn tick_ttl_rollbacks(config: &AppConfig) {
    let expired: Vec<ActivePlan> = {
        let mut st = state().lock().unwrap();
        let now = unix_now();
        let expired: Vec<_> = st.active_plans.values()
            .filter(|p| p.expires_at > 0 && now >= p.expires_at)
            .cloned()
            .collect();
        for p in &expired {
            st.active_plans.remove(&p.plan_id);
        }
        expired
    };
    for p in expired {
        rollback_plan(&p, &config.server.api_url, &config.hotfix);
    }
}

/// Список активных планов для экрана "Настройки → Диагностика".
pub fn active_plans() -> Vec<ActivePlan> {
    state().lock().unwrap().active_plans.values().cloned().collect()
}

/// Откатить конкретный план (ручной откат пользователем).
pub fn rollback_by_id(plan_id: &str, config: &AppConfig) {
    let plan = state().lock().unwrap().active_plans.remove(plan_id);
    if let Some(p) = plan {
        let api_url = config.server.api_url.clone();
        let cfg = config.hotfix.clone();
        thread::spawn(move || rollback_plan(&p, &api_url, &cfg));
    }
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

fn run_pipeline(
    incident: Incident,
    api_url: &str,
    device_id: &str,
    tenant_id: &str,
    cfg: &HotfixConfig,
) {
    // 1. Собрать fingerprint
    let env = collect_fingerprint();

    // 2. Отправить инцидент
    let server_incident_id = match submit_incident(&incident, device_id, tenant_id, &env, api_url, cfg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[hotfix] submit error: {e}");
            return;
        }
    };

    // 3. Polling анализа
    let plan = match wait_for_plan(&server_incident_id, api_url, cfg) {
        Some(p) => p,
        None => return, // no_action или timeout
    };

    // 4. Верификация
    if let Err(e) = verify_plan(&plan, device_id, &env, cfg) {
        eprintln!("[hotfix] plan rejected: {e}");
        report_decision(&plan.plan_id, e.report_code(), "client", api_url, cfg);
        return;
    }

    // 5. Если no_action или advice_only — не показываем UI
    if matches!(plan.decision.as_str(), "no_action" | "advice_only") {
        return;
    }

    // 6. Если automatic_safe_fix — применяем без диалога
    if plan.decision == "automatic_safe_fix" && !plan.requires_user_consent {
        apply_plan(plan, api_url, &AppConfig::load_or_create());
        return;
    }

    // 7. Иначе — передать в UI для показа диалога
    state().lock().unwrap().pending_consent = Some(plan);
}

// ── Fingerprint ───────────────────────────────────────────────────────────────

fn collect_fingerprint() -> Value {
    let os_family = std::env::consts::OS;            // "linux", "windows", "macos"
    let os_name   = detect_distro_name();            // "Astra Linux", "Windows 11"
    let os_version = detect_os_version();

    let (gpu_vendor, gpu_model, gpu_driver, gpu_driver_version) = detect_gpu_info();

    json!({
        "os": {
            "family":  os_family,
            "name":    os_name,
            "version": os_version,
            "kernel":  detect_kernel_version(),
        },
        "hardware": {
            "cpu": detect_cpu_model(),
            "gpu": {
                "vendor":         gpu_vendor,
                "model":          gpu_model,
                "driver":         gpu_driver,
                "driver_version": gpu_driver_version,
            }
        },
        "graphics": {
            "display_server":       detect_display_server(),  // "x11", "wayland", "win32"
            "renderer_backend":     detect_renderer_backend(), // "wgpu/vulkan", "wgpu/dx12", "glow/opengl"
            "hardware_acceleration": detect_hw_accel(),
        },
        "network": {
            "transport": "unknown", // заполнять из активной сессии если есть
        }
    })
}

/// Определить дистрибутив Linux через /etc/os-release.
fn detect_distro_name() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(raw) = std::fs::read_to_string("/etc/os-release") {
            for line in raw.lines() {
                if let Some(val) = line.strip_prefix("PRETTY_NAME=") {
                    return val.trim_matches('"').to_owned();
                }
            }
        }
        return "Linux".to_owned();
    }
    #[cfg(target_os = "windows")]
    { "Windows".to_owned() }
    #[cfg(target_os = "macos")]
    { "macOS".to_owned() }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    { std::env::consts::OS.to_owned() }
}

fn detect_os_version() -> String {
    // TODO: sys_info или winver
    String::new()
}

fn detect_kernel_version() -> String {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
            .trim()
            .to_owned()
    }
    #[cfg(not(target_os = "linux"))]
    { String::new() }
}

fn detect_cpu_model() -> String {
    // TODO: cpuid / /proc/cpuinfo
    String::new()
}

fn detect_gpu_info() -> (String, String, String, String) {
    // TODO: glGetString(GL_VENDOR/RENDERER) или DXGI adapter info
    // Вернуть ("intel"|"nvidia"|"amd"|"unknown", model, driver, version)
    (String::new(), String::new(), String::new(), String::new())
}

fn detect_display_server() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WAYLAND_DISPLAY").is_ok() { "wayland" } else { "x11" }
    }
    #[cfg(target_os = "windows")]
    { "win32" }
    #[cfg(target_os = "macos")]
    { "quartz" }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    { "unknown" }
}

fn detect_renderer_backend() -> String {
    // eframe использует wgpu — попробовать определить backend через env или хранимое состояние.
    // Пока заглушка; можно передавать из egui App::setup через channel.
    std::env::var("WGPU_BACKEND").unwrap_or_else(|_| "wgpu".to_owned())
}

fn detect_hw_accel() -> bool {
    // Считаем true если не установлен LIBGL_ALWAYS_SOFTWARE
    std::env::var("LIBGL_ALWAYS_SOFTWARE")
        .map(|v| v != "1")
        .unwrap_or(true)
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

fn api_request(method: &str, url: &str, cfg: &HotfixConfig) -> ureq::Request {
    let req = match method {
        "POST" => ureq::post(url),
        _ => ureq::get(url),
    };
    let req = req.timeout(HTTP_TIMEOUT)
        .set("Content-Type", "application/json")
        .set("X-EvertyDesk-Protocol-Version", "1");

    if !cfg.api_key.is_empty() {
        req.set("Authorization", &format!("Bearer {}", cfg.api_key))
    } else {
        req
    }
}

fn post_json_hotfix(url: &str, body: Value, cfg: &HotfixConfig) -> Result<Value, String> {
    match api_request("POST", url, cfg).send_json(body) {
        Ok(r) => r.into_json::<Value>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            Err(format!("HTTP {code}: {}", r.into_string().unwrap_or_default()))
        }
        Err(ureq::Error::Transport(e)) => Err(format!("network: {e}")),
    }
}

fn get_json_hotfix(url: &str, cfg: &HotfixConfig) -> Result<Value, String> {
    match api_request("GET", url, cfg).call() {
        Ok(r) => r.into_json::<Value>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            Err(format!("HTTP {code}: {}", r.into_string().unwrap_or_default()))
        }
        Err(ureq::Error::Transport(e)) => Err(format!("network: {e}")),
    }
}

// ── Submit incident ───────────────────────────────────────────────────────────

fn submit_incident(
    incident: &Incident,
    device_id: &str,
    tenant_id: &str,
    env: &Value,
    api_url: &str,
    cfg: &HotfixConfig,
) -> Result<String, String> {
    let client_id = Uuid::new_v4().to_string();
    let body = json!({
        "schema_version": 1,
        "incident_id": client_id,
        "device_id":   device_id,
        "tenant_id":   tenant_id,
        "app": {
            "name":    "EvertyDesk Lite",
            "version": env!("CARGO_PKG_VERSION"),
            "channel": "stable",
        },
        "incident": {
            "type":            incident.incident_type,
            "severity":        incident.severity,
            "component":       incident.component,
            "error_code":      incident.error_code,
            "message":         incident.message,
            "crash_signature": incident.crash_signature,
            "occurred_at":     iso_now(),
        },
        "environment": env,
        "component_state": {},
        "metrics": {},
        "logs": [],
    });

    let url = format!("{api_url}/api/v1/incidents");
    let resp = post_json_hotfix(&url, body, cfg)?;

    resp["server_incident_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "missing server_incident_id".to_owned())
}

// ── Polling ───────────────────────────────────────────────────────────────────

fn wait_for_plan(server_incident_id: &str, api_url: &str, cfg: &HotfixConfig) -> Option<RemediationPlan> {
    let deadline = Instant::now() + Duration::from_secs(POLL_TIMEOUT_SECS);
    let mut delay = POLL_INITIAL_SECS;

    while Instant::now() < deadline {
        thread::sleep(Duration::from_secs(delay));

        let url = format!("{api_url}/api/v1/incidents/{server_incident_id}/analysis");
        let analysis = match get_json_hotfix(&url, cfg) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[hotfix] poll error: {e}");
                continue;
            }
        };

        let status = analysis["status"].as_str().unwrap_or("");
        if status == "failed" {
            return None;
        }
        if status == "ready" && analysis["plan_available"].as_bool().unwrap_or(false) {
            let plan_id = analysis["plan_id"].as_str()?;
            let plan_url = format!("{api_url}/api/v1/remediation-plans/{plan_id}");
            return match get_json_hotfix(&plan_url, cfg) {
                Ok(v) => parse_plan(v),
                Err(e) => {
                    eprintln!("[hotfix] fetch plan error: {e}");
                    None
                }
            };
        }

        delay = (delay * 3 / 2).min(POLL_MAX_SECS);
    }
    None
}

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemediationPlan {
    pub plan_id: String,
    pub incident_id: String,
    pub cluster_id: String,
    pub schema_version: u32,
    pub decision: String,
    pub confidence: f64,
    pub risk_level: String,
    pub requires_user_consent: bool,
    pub ttl_seconds: u64,
    pub expires_at: String,
    pub scope: Value,
    pub summary: Value,
    pub actions: Vec<PlanAction>,
    pub rollback_actions: Vec<PlanAction>,
    pub signature: PlanSignature,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanAction {
    pub op: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanSignature {
    pub alg: String,
    pub key_id: String,
    pub signed_at: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivePlan {
    pub plan_id: String,
    pub summary_title: String,
    pub expires_at: u64,    // unix timestamp; 0 = no TTL
    pub rollback_actions: Vec<PlanAction>,
    pub previous_values: HashMap<String, Value>, // config_key → old value
}

fn parse_plan(v: Value) -> Option<RemediationPlan> {
    serde_json::from_value(v).ok()
}

// ── Верификация ───────────────────────────────────────────────────────────────

struct VerifyError(&'static str);

impl VerifyError {
    fn report_code(&self) -> &'static str { self.0 }
}

fn verify_plan(plan: &RemediationPlan, device_id: &str, env: &Value, cfg: &HotfixConfig) -> Result<(), VerifyError> {
    // 1. Schema
    if plan.schema_version != 1 {
        return Err(VerifyError("unsupported_action"));
    }

    // 2. Подпись Ed25519
    if !cfg.signing_public_key_b64.is_empty() {
        verify_signature(plan, cfg).map_err(|_| VerifyError("signature_invalid"))?;
    }

    // 3. TTL
    if !plan.expires_at.is_empty() {
        if let Ok(exp) = chrono_parse_or_unix(&plan.expires_at) {
            if unix_now() >= exp {
                return Err(VerifyError("expired"));
            }
        }
    }

    // 4. Scope — device_id
    if let Some(scope_device) = plan.scope["device_id"].as_str() {
        if !scope_device.is_empty() && scope_device != device_id {
            return Err(VerifyError("scope_mismatch"));
        }
    }

    // 5. Scope — os.family
    if let Some(req_family) = plan.scope["match"]["os.family"].as_str() {
        let actual = env["os"]["family"].as_str().unwrap_or("");
        if !req_family.is_empty() && req_family != actual {
            return Err(VerifyError("scope_mismatch"));
        }
    }

    // 6. Actions — все op должны быть в реестре
    for action in &plan.actions {
        if !ALLOWED_OPS.contains(&action.op.as_str()) {
            return Err(VerifyError("unsupported_action"));
        }
    }
    for action in &plan.rollback_actions {
        if !ALLOWED_OPS.contains(&action.op.as_str()) {
            return Err(VerifyError("unsupported_action"));
        }
    }

    Ok(())
}

fn verify_signature(plan: &RemediationPlan, cfg: &HotfixConfig) -> Result<(), ()> {
    // Публичный ключ из конфига (32 байта, base64)
    let pk_bytes = STANDARD.decode(&cfg.signing_public_key_b64).map_err(|_| ())?;
    let pk: [u8; 32] = pk_bytes.try_into().map_err(|_| ())?;

    // Подпись из плана (64 байта, base64)
    let sig_bytes = STANDARD.decode(&plan.signature.signature).map_err(|_| ())?;
    if sig_bytes.len() != 64 {
        return Err(());
    }

    // Воссоздать payload — ровно то, что подписывал сервер (см. hotfix_signing.go)
    let payload = serde_json::to_string(&json!({
        "plan_id":               plan.plan_id,
        "incident_id":           plan.incident_id,
        "cluster_id":            plan.cluster_id,
        "schema_version":        plan.schema_version,
        "decision":              plan.decision,
        "risk_level":            plan.risk_level,
        "requires_user_consent": plan.requires_user_consent,
        "ttl_seconds":           plan.ttl_seconds,
        "expires_at":            plan.expires_at,
        "scope":                 plan.scope,
        "actions":               plan.actions,
        "rollback_actions":      plan.rollback_actions,
    })).map_err(|_| ())?;

    // dryoc crypto_sign работает в combined mode: signed = sig (64) || msg.
    // Собираем combined и вызываем crypto_sign_open.
    let mut combined = Vec::with_capacity(64 + payload.len());
    combined.extend_from_slice(&sig_bytes);
    combined.extend_from_slice(payload.as_bytes());

    let mut out = vec![0u8; payload.len()];
    dryoc::classic::crypto_sign::crypto_sign_open(&mut out, &combined, &pk)
        .map_err(|_| ())
}

// ── Apply actions ─────────────────────────────────────────────────────────────

fn apply_plan(plan: RemediationPlan, api_url: &str, config: &AppConfig) {
    report_decision(&plan.plan_id, "accepted", "user", api_url, &config.hotfix);

    let mut previous_values: HashMap<String, Value> = HashMap::new();
    let mut applied_keys: Vec<String> = Vec::new();
    let mut all_ok = true;

    for action in &plan.actions {
        match apply_action(action, &mut previous_values, config) {
            Ok(key) => {
                if let Some(k) = key { applied_keys.push(k); }
            }
            Err(e) => {
                eprintln!("[hotfix] action failed ({}): {e}", action.op);
                all_ok = false;
                break;
            }
        }
    }

    if !all_ok {
        // Откатить уже применённые
        for action in &plan.rollback_actions {
            let _ = apply_action(action, &mut previous_values, config);
        }
        return;
    }

    // Сохранить активный план для TTL rollback
    let expires_at = if plan.ttl_seconds > 0 {
        unix_now() + plan.ttl_seconds
    } else {
        0
    };
    let summary_title = plan.summary["title"]
        .as_str()
        .unwrap_or("Hotfix")
        .to_owned();

    let active = ActivePlan {
        plan_id: plan.plan_id.clone(),
        summary_title,
        expires_at,
        rollback_actions: plan.rollback_actions.clone(),
        previous_values,
    };
    state().lock().unwrap().active_plans.insert(plan.plan_id.clone(), active);

    // Сохранить изменённый конфиг
    config.save();

    // Отчёт о применении
    let url = format!("{api_url}/api/v1/remediation-plans/{}/applied", plan.plan_id);
    let device_id = config.ui.agent_machine_id.clone();
    let body = json!({
        "device_id":      device_id,
        "applied":        true,
        "applied_at":     iso_now(),
        "actions_applied": applied_keys,
    });
    let _ = post_json_hotfix(&url, body, &config.hotfix);

    // Запланировать мониторинг результата через 10 минут
    let plan_id = plan.plan_id.clone();
    let api_url = api_url.to_owned();
    let cfg = config.hotfix.clone();
    let dev = config.ui.agent_machine_id.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(600));
        report_result_auto(&plan_id, &api_url, &dev, &cfg);
    });
}

fn apply_action(
    action: &PlanAction,
    previous_values: &mut HashMap<String, Value>,
    config: &AppConfig,
) -> Result<Option<String>, String> {
    let mut cfg = config.clone();

    match action.op.as_str() {
        "set_config" => {
            let key = action.key.as_deref().ok_or("missing key")?;
            let value = action.value.as_ref().ok_or("missing value")?;
            let old = get_config_value(&cfg, key);
            previous_values.insert(key.to_owned(), old);
            set_config_value(&mut cfg, key, value.clone())?;
            cfg.save();
            Ok(Some(key.to_owned()))
        }
        "restore_config" => {
            let key = action.key.as_deref().ok_or("missing key")?;
            if let Some(old_val) = previous_values.get(key) {
                set_config_value(&mut cfg.clone(), key, old_val.clone())?;
                cfg.save();
            }
            Ok(None)
        }
        "lower_fps" => {
            let fps = action.value.as_ref()
                .and_then(Value::as_u64)
                .ok_or("missing fps value")? as u32;
            previous_values.insert(
                "display.target_fps".to_owned(),
                json!(cfg.display.target_fps),
            );
            cfg.display.target_fps = fps;
            cfg.save();
            Ok(Some("display.target_fps".to_owned()))
        }
        "lower_quality" => {
            // Переключить FSR на более агрессивный режим
            previous_values.insert(
                "display.fsr_quality".to_owned(),
                serde_json::to_value(cfg.display.fsr_quality).unwrap_or(Value::Null),
            );
            cfg.display.fsr_quality = crate::settings::FsrQualitySetting::Performance;
            cfg.save();
            Ok(Some("display.fsr_quality".to_owned()))
        }
        "enable_safe_mode" => {
            // Переключить энкодер на software
            previous_values.insert(
                "display.encoder".to_owned(),
                serde_json::to_value(cfg.display.encoder).unwrap_or(Value::Null),
            );
            cfg.display.encoder = crate::settings::EncoderPreference::Software;
            cfg.save();
            Ok(Some("display.encoder".to_owned()))
        }
        "collect_diagnostics" => {
            // Диагностика запускается асинхронно и не изменяет конфиг
            eprintln!("[hotfix] collect_diagnostics requested (target: {:?})", action.target);
            Ok(None)
        }
        "restart_component_soft" => {
            // Перезапуск компонента — реализовать через существующие каналы
            eprintln!("[hotfix] restart_component_soft: {:?}", action.component);
            Ok(None)
        }
        "disable_feature" => {
            // Отключить фичу по ключу
            let key = action.key.as_deref().ok_or("missing key")?;
            eprintln!("[hotfix] disable_feature: {key}");
            Ok(Some(key.to_owned()))
        }
        _ => {
            // Остальные допустимые ops — заглушки для будущего расширения
            eprintln!("[hotfix] op not yet implemented: {}", action.op);
            Ok(None)
        }
    }
}

/// Прочитать значение конфига по dot-path ключу.
fn get_config_value(cfg: &AppConfig, key: &str) -> Value {
    match key {
        "display.target_fps"  => json!(cfg.display.target_fps),
        "display.encoder"     => serde_json::to_value(cfg.display.encoder).unwrap_or(Value::Null),
        "display.fsr_quality" => serde_json::to_value(cfg.display.fsr_quality).unwrap_or(Value::Null),
        "display.codec"       => serde_json::to_value(cfg.display.codec).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// Установить значение конфига по dot-path ключу.
fn set_config_value(cfg: &mut AppConfig, key: &str, value: Value) -> Result<(), String> {
    match key {
        "display.target_fps" => {
            cfg.display.target_fps = value.as_u64().ok_or("bad fps")? as u32;
            Ok(())
        }
        "display.encoder" => {
            cfg.display.encoder = serde_json::from_value(value).map_err(|e| e.to_string())?;
            Ok(())
        }
        "display.fsr_quality" => {
            cfg.display.fsr_quality = serde_json::from_value(value).map_err(|e| e.to_string())?;
            Ok(())
        }
        "display.codec" => {
            cfg.display.codec = serde_json::from_value(value).map_err(|e| e.to_string())?;
            Ok(())
        }
        _ => Err(format!("unknown config key: {key}")),
    }
}

// ── Rollback ──────────────────────────────────────────────────────────────────

fn rollback_plan(active: &ActivePlan, api_url: &str, cfg: &HotfixConfig) {
    let mut config = AppConfig::load_or_create();
    for action in &active.rollback_actions {
        if action.op == "restore_config" {
            if let Some(key) = &action.key {
                if let Some(old_val) = active.previous_values.get(key) {
                    let _ = set_config_value(&mut config, key, old_val.clone());
                }
            }
        }
    }
    config.save();

    let url = format!("{api_url}/api/v1/remediation-plans/{}/rollback", active.plan_id);
    let body = json!({
        "device_id":    config.ui.agent_machine_id,
        "reason":       "ttl_expired",
        "rolled_back_at": iso_now(),
    });
    let _ = post_json_hotfix(&url, body, cfg);
}

// ── Reports ───────────────────────────────────────────────────────────────────

fn report_decision(plan_id: &str, decision: &str, decided_by: &str, api_url: &str, cfg: &HotfixConfig) {
    let url = format!("{api_url}/api/v1/remediation-plans/{plan_id}/decision");
    let body = json!({
        "decision":   decision,
        "decided_by": decided_by,
        "decided_at": iso_now(),
    });
    let _ = post_json_hotfix(&url, body, cfg);
}

fn report_result_auto(plan_id: &str, api_url: &str, device_id: &str, cfg: &HotfixConfig) {
    // TODO: собрать реальные метрики из активной сессии
    let url = format!("{api_url}/api/v1/remediation-plans/{plan_id}/result");
    let body = json!({
        "device_id":            device_id,
        "status":               "unknown",
        "observed_for_seconds": 600,
        "new_crashes":          0,
    });
    let _ = post_json_hotfix(&url, body, cfg);
}

// ── Утилиты ───────────────────────────────────────────────────────────────────

fn iso_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Простой ISO-8601 без внешних зависимостей
    let s = now;
    let (y, m, d, h, mi, sec) = unix_to_ymd(s);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{sec:02}Z")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn chrono_parse_or_unix(s: &str) -> Result<u64, ()> {
    // Парсим RFC3339: "2026-06-22T12:00:00Z"
    let s = s.trim_end_matches('Z');
    let parts: Vec<&str> = s.split('T').collect();
    if parts.len() != 2 { return Err(()); }
    let date: Vec<u64> = parts[0].split('-').filter_map(|x| x.parse().ok()).collect();
    let time: Vec<u64> = parts[1].split(':').filter_map(|x| x.parse().ok()).collect();
    if date.len() != 3 || time.len() != 3 { return Err(()); }
    // Упрощённый расчёт unix timestamp (без учёта секунд координации)
    let days = days_since_epoch(date[0], date[1], date[2]);
    Ok(days * 86400 + time[0] * 3600 + time[1] * 60 + time[2])
}

fn days_since_epoch(y: u64, m: u64, d: u64) -> u64 {
    // Алгоритм Julian Day → Unix days (Julian Day epoch = 4713 BC Jan 1)
    let a = (14 - m) / 12;
    let yr = y + 4800 - a;
    let mo = m + 12 * a - 3;
    let jdn = d + (153 * mo + 2) / 5 + 365 * yr + yr / 4 - yr / 100 + yr / 400 - 32045;
    jdn - 2440588 // JDN of 1970-01-01
}

fn unix_to_ymd(t: u64) -> (u64, u64, u64, u64, u64, u64) {
    let sec = t % 60;
    let t = t / 60;
    let min = t % 60;
    let t = t / 60;
    let hour = t % 24;
    let days = t / 24;
    // Zeller-like: не идеален но достаточен для логов
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hour, min, sec)
}
```

---

## 3. Подключить модуль в lib.rs / main.rs

```rust
// src/lib.rs (или src/main.rs)
pub mod hotfix;
```

---

## 4. Вызовы из компонентов

### Рендер — eframe / wgpu backend упал

```rust
// В обработчике ошибки инициализации рендера
hotfix::report(
    hotfix::Incident {
        incident_type:   "crash",
        severity:        "error",
        component:       "renderer",
        error_code:      "RENDERER_INIT_FAILED",
        message:         format!("eframe/wgpu init error: {err}"),
        crash_signature: "renderer.wgpu.init.failed",
    },
    &config,
);
```

### EVRT — высокий packet loss

```rust
// В EvrtClient::run_feedback_loop, когда pressure > порога
if pressure.loss_rate > 0.15 {
    hotfix::report(
        hotfix::Incident {
            incident_type:   "degradation",
            severity:        "warning",
            component:       "evrt",
            error_code:      "EVRT_PACKET_LOSS_HIGH",
            message:         format!("packet loss {:.1}% on direct UDP", pressure.loss_rate * 100.0),
            crash_signature: "evrt.direct_udp.packet_loss.high",
        },
        &config,
    );
}
```

### Hyper-V — сбой инициализации VM-консоли

```rust
// В hyperv.rs
hotfix::report(
    hotfix::Incident {
        incident_type:   "crash",
        severity:        "error",
        component:       "vm_console",
        error_code:      "HYPERV_PROBE_FAILED",
        message:         format!("WMI probe error: {err}"),
        crash_signature: "vm_console.hyper_v.probe.failed",
    },
    &config,
);
```

---

## 5. Consent UI в egui

Вставить в основной `egui::CentralPanel` или в `App::update`:

```rust
// В App::update, в самом начале перед основным UI
if self.hotfix_pending_plan.is_none() {
    self.hotfix_pending_plan = hotfix::take_pending_consent();
}

if let Some(plan) = &self.hotfix_pending_plan.clone() {
    let title   = plan.summary["title"].as_str().unwrap_or("EvertyDesk: обнаружена проблема");
    let message = plan.summary["user_message"].as_str().unwrap_or("");
    let risk    = &plan.risk_level;
    let ttl_days = plan.ttl_seconds / 86400;

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(message);
            ui.add_space(8.0);

            if let Some(effects) = plan.summary["possible_side_effects"].as_array() {
                if !effects.is_empty() {
                    ui.label("Возможные побочные эффекты:");
                    for e in effects {
                        ui.label(format!("• {}", e.as_str().unwrap_or("")));
                    }
                    ui.add_space(4.0);
                }
            }

            ui.label(format!("Уровень риска: {risk}"));
            if ttl_days > 0 {
                ui.label(format!("Срок действия: {ttl_days} дней"));
            }
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                if ui.button("Применить").clicked() {
                    let p = self.hotfix_pending_plan.take().unwrap();
                    hotfix::user_accepted(p, &self.config);
                }
                if ui.button("Не сейчас").clicked() {
                    let p = self.hotfix_pending_plan.take().unwrap();
                    hotfix::user_declined(&p, &self.config);
                }
            });
        });
}

// TTL-rollback тик (раз в N итераций чтобы не спамить)
self.hotfix_tick += 1;
if self.hotfix_tick % 3600 == 0 {   // ~1 раз в минуту при 60fps
    hotfix::tick_ttl_rollbacks(&self.config);
}
```

Добавить в `App` struct:

```rust
pub struct App {
    // ...
    hotfix_pending_plan: Option<hotfix::RemediationPlan>,
    hotfix_tick: u64,
}
```

---

## 6. Экран "Активные исправления" (Settings UI)

```rust
// В секции Настройки → Диагностика
ui.heading("Активные исправления AI Hotfix");
let plans = hotfix::active_plans();
if plans.is_empty() {
    ui.label("Нет активных исправлений.");
} else {
    for plan in plans {
        ui.horizontal(|ui| {
            ui.label(&plan.summary_title);
            if plan.expires_at > 0 {
                let left = plan.expires_at.saturating_sub(hotfix::unix_now());
                ui.label(format!("(истекает через {} ч)", left / 3600));
            }
            if ui.small_button("Откатить").clicked() {
                hotfix::rollback_by_id(&plan.plan_id, &self.config);
            }
        });
    }
}
```

---

## 7. Настройка сервера — получить публичный ключ

На сервере, где задан `HOTFIX_SIGNING_KEY_PRIVATE`:

```bash
# Декодировать private key, взять последние 32 байта (публичный ключ), закодировать обратно
python3 -c "
import base64, sys
priv = base64.b64decode('${HOTFIX_SIGNING_KEY_PRIVATE}')
pub = priv[32:]
print(base64.b64encode(pub).decode())
"
```

Полученную строку вставить в `HotfixConfig.signing_public_key_b64` в конфиге клиента (или зашить как константу в `hotfix.rs` для production-сборки).

---

## 8. Что дописать (TODO)

| Место | Что сделать |
|-------|-------------|
| `collect_fingerprint()` | Заполнить `gpu_vendor/model/driver` через WMI (Windows) / glxinfo (Linux) |
| `collect_fingerprint()` | Передавать transport из активной EVRT-сессии через канал |
| `apply_action` → `restart_component_soft` | Отправить команду в transport channel для перезапуска |
| `apply_action` → `collect_diagnostics` | Запустить `crate::diagnostics::run_headless()` |
| `report_result_auto` | Собирать реальные метрики FPS / latency из `SessionEvent::Stats` |
| `tick_ttl_rollbacks` | Персистировать `active_plans` в JSON-файл чтобы пережить рестарт |
| `hotfix.rs` | Добавить `pub fn unix_now() -> u64` в pub scope (для UI) |
