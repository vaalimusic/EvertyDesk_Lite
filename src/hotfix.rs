//! AI Hotfix Pipeline — клиентская интеграция для EvertyDesk Lite.
//!
//! Точки входа:
//!   • [`report`] — отправить краш в фоновом потоке (вызывай из panic hook / catch_unwind)
//!   • [`tick`]   — вызывай каждый кадр egui; обрабатывает TTL-откаты и возвращает
//!                  `Some(ConsentRequest)` если нужен диалог согласия пользователя
//!   • [`confirm_consent`] / [`deny_consent`] — ответ пользователя на диалог

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use dryoc::classic::crypto_sign::crypto_sign_open;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ureq::Error as UreqError;

use crate::settings::{
    AppConfig, DisplayConfig, EncoderPreference, FsrQualitySetting, HotfixConfig,
};

// ── Публичные типы ────────────────────────────────────────────────────────────

/// Возвращается из [`tick`] когда план требует согласия пользователя.
#[derive(Clone, Debug)]
pub struct ConsentRequest {
    pub plan_id: String,
    pub risk_level: String,
    /// Краткое описание что будет изменено (из plan.summary.text).
    pub summary: String,
    /// Человекочитаемый список действий.
    pub actions_human: Vec<String>,
}

/// Глобальное разделяемое состояние. Создай один раз в `main`, передавай всюду.
#[derive(Default)]
pub struct HotfixState {
    // Последнее время отправки для каждой crash_signature → unix timestamp.
    rate_map: HashMap<String, u64>,
    // Планы ожидающие TTL-отката: (deadline_unix, plan_id, rollback_actions, config_snapshot).
    pending_rollbacks: Vec<PendingRollback>,
    // Ожидает ли диалог согласия.
    pending_consent: Option<PendingConsent>,
    // Очередь готовых к применению планов (прошли верификацию, не требуют согласия).
    ready_plans: Vec<ReadyPlan>,
}

#[derive(Clone, Debug)]
struct PendingRollback {
    deadline_unix: u64,
    plan_id: String,
    rollback_actions: Vec<PlanAction>,
    original: DisplaySnapshot,
}

#[derive(Clone, Debug)]
struct PendingConsent {
    plan_id: String,
    risk_level: String,
    summary: String,
    actions_human: Vec<String>,
    ready: ReadyPlan,
}

#[derive(Clone, Debug)]
struct ReadyPlan {
    plan_id: String,
    actions: Vec<PlanAction>,
    rollback_actions: Vec<PlanAction>,
    ttl_seconds: u64,
}

/// Снимок настроек отображения перед применением (для отката).
#[derive(Clone, Debug)]
struct DisplaySnapshot {
    target_fps: u32,
    encoder: EncoderPreference,
    fsr_quality: FsrQualitySetting,
}

// ── Wire-форматы API ──────────────────────────────────────────────────────────

// Запрос POST /api/v1/incidents — вложенная структура как ожидает сервер.
#[derive(Serialize)]
struct IncidentPayload {
    schema_version: u8,
    incident_id: String,    // client-generated UUID (idempotency key)
    device_id: String,
    app: IncidentApp,
    incident: IncidentInfo,
    environment: IncidentEnv,
}

#[derive(Serialize)]
struct IncidentApp {
    name: String,
    version: String,
}

#[derive(Serialize)]
struct IncidentInfo {
    #[serde(rename = "type")]
    incident_type: String,
    severity: String,
    component: String,
    error_code: String,
    crash_signature: String,
    message: String,
    occurred_at: String,
}

#[derive(Serialize)]
struct IncidentEnv {
    os: EnvOs,
    hardware: EnvHardware,
    graphics: EnvGraphics,
    network: EnvNetwork,
    detail: EnvDetail,
}

#[derive(Serialize)]
struct EnvOs { family: String, name: String }

#[derive(Serialize)]
struct EnvGpu { vendor: String, driver_version: String }

#[derive(Serialize)]
struct EnvHardware { gpu: EnvGpu }

#[derive(Serialize)]
struct EnvGraphics { renderer_backend: String }

#[derive(Serialize)]
struct EnvNetwork { transport: String }

// Extra field for stack trace — sервер хранит в raw_payload.
#[derive(Serialize)]
struct EnvDetail { stack_trace: String }

// POST /api/v1/incidents → {"accepted":true,"server_incident_id":"..."}
#[derive(Deserialize, Debug)]
struct SubmitResponse {
    server_incident_id: String,
}

// GET /api/v1/incidents/{id}/analysis → {"status":"...","plan_id":"..."}
#[derive(Deserialize, Debug)]
struct AnalysisResponse {
    status: String, // queued | analyzing | ready | failed
    #[serde(default)]
    plan_id: Option<String>,
}

// GET /api/v1/remediation-plans/{id} → {"plan_id":"...","decision":"...",...}
#[derive(Deserialize, Debug)]
struct PlanResponse {
    plan_id: String,
    decision: String,
    risk_level: String,
    requires_user_consent: bool,
    ttl_seconds: u64,
    actions: Vec<PlanAction>,
    rollback_actions: Vec<PlanAction>,
    summary: Option<PlanSummary>,
    signature: PlanSignature,
    payload: String, // JSON строка подписанного тела (для верификации Ed25519)
}

#[derive(Deserialize, Debug, Clone)]
struct PlanAction {
    op: String,
    #[serde(default)]
    param: String,
    #[serde(default)]
    value: Value,
}

#[derive(Deserialize, Debug)]
struct PlanSummary {
    text: Option<String>,
}

#[derive(Deserialize, Debug)]
struct PlanSignature {
    sig: String,     // base64 Ed25519 64-byte signature
    key_id: String,
}

// ── Публичный API ─────────────────────────────────────────────────────────────

/// Собери и отправь краш-репорт в фоновом потоке.
///
/// Безопасно вызывать из panic hook — не паникует сам, все ошибки логируются stderr.
/// `state` должен быть `Arc<Mutex<HotfixState>>`.
/// Синхронная отправка краш-репорта — используй в panic hook.
///
/// Делает только POST /incidents с таймаутом 5 сек, не ждёт AI-анализа.
/// Безопасно вызывать из panic hook — не паникует сам.
pub fn submit_crash_sync(
    crash_signature: String,
    component: String,
    error_code: String,
    message: String,
    stack_trace: String,
    config: &HotfixConfig,
    app_config: &AppConfig,
) {
    if !config.enabled || config.api_key.is_empty() {
        eprintln!("[hotfix] submit_crash_sync skipped: enabled={} key_empty={}", config.enabled, config.api_key.is_empty());
        return;
    }
    let fp = collect_fingerprint();
    let payload = build_incident_payload(
        uuid::Uuid::new_v4().to_string(),
        app_config.ui.agent_machine_id.clone(),
        crash_signature,
        component,
        error_code,
        message,
        stack_trace,
        &fp,
    );
    let url = format!("{}/api/v1/incidents", app_config.server.api_url.trim_end_matches('/'));
    eprintln!("[hotfix] submitting to {url}");
    // 5 секунд максимум — не блокируем процесс надолго при завершении.
    match ureq::post(&url)
        .timeout(Duration::from_secs(5))
        .set("Authorization", &format!("Bearer {}", config.api_key))
        .set("Content-Type", "application/json")
        .send_json(serde_json::to_value(&payload).unwrap_or_default())
    {
        Ok(_) => eprintln!("[hotfix] crash submitted ok"),
        Err(e) => eprintln!("[hotfix] crash submit failed: {e}"),
    }
}

fn build_incident_payload(
    incident_id: String,
    device_id: String,
    crash_signature: String,
    component: String,
    error_code: String,
    message: String,
    stack_trace: String,
    fp: &Fingerprint,
) -> IncidentPayload {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // RFC3339 approximation from unix timestamp
    let secs = now.as_secs();
    let occurred_at = format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        1970 + secs / 31_557_600,
        ((secs % 31_557_600) / 2_629_800) + 1,
        ((secs % 2_629_800) / 86400) + 1,
        (secs % 86400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    );
    IncidentPayload {
        schema_version: 1,
        incident_id,
        device_id,
        app: IncidentApp {
            name: "EvertyDesk Lite".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        incident: IncidentInfo {
            incident_type: "crash".to_owned(),
            severity: "error".to_owned(),
            component,
            error_code,
            crash_signature,
            message,
            occurred_at,
        },
        environment: IncidentEnv {
            os: EnvOs {
                family: fp.os_family.clone(),
                name: fp.distro.clone(),
            },
            hardware: EnvHardware {
                gpu: EnvGpu {
                    vendor: fp.gpu_vendor.clone(),
                    driver_version: fp.driver_version.clone(),
                },
            },
            graphics: EnvGraphics {
                renderer_backend: fp.renderer_backend.clone(),
            },
            network: EnvNetwork {
                transport: fp.evrt_transport.clone(),
            },
            detail: EnvDetail { stack_trace },
        },
    }
}

pub fn report(
    crash_signature: String,
    component: String,
    error_code: String,
    message: String,
    stack_trace: String,
    config: HotfixConfig,
    app_config: AppConfig,
    state: Arc<Mutex<HotfixState>>,
) {
    if !config.enabled || config.api_key.is_empty() {
        return;
    }

    // Rate-limit check
    let now_unix = unix_now();
    {
        let mut st = state.lock().unwrap();
        if let Some(&last) = st.rate_map.get(&crash_signature) {
            if now_unix.saturating_sub(last) < config.rate_limit_secs {
                return;
            }
        }
        st.rate_map.insert(crash_signature.clone(), now_unix);
    }

    let state2 = Arc::clone(&state);
    thread::spawn(move || {
        if let Err(e) = run_report(
            crash_signature,
            component,
            error_code,
            message,
            stack_trace,
            &config,
            &app_config,
            state2,
        ) {
            eprintln!("[hotfix] report error: {e}");
        }
    });
}

/// Вызывай каждый кадр egui-петли.
///
/// Возвращает `Some(ConsentRequest)` ровно один раз — когда план требует
/// подтверждения пользователя. Показывай диалог, затем вызови
/// [`confirm_consent`] или [`deny_consent`].
pub fn tick(state: &Arc<Mutex<HotfixState>>, config: &mut AppConfig) -> Option<ConsentRequest> {
    let mut st = state.lock().unwrap();

    // TTL-откаты
    let now = unix_now();
    let mut expired = vec![];
    let mut remaining = vec![];
    for rb in st.pending_rollbacks.drain(..) {
        if now >= rb.deadline_unix {
            expired.push(rb);
        } else {
            remaining.push(rb);
        }
    }
    st.pending_rollbacks = remaining;
    drop(st);

    for rb in expired {
        eprintln!("[hotfix] TTL expired for plan {}, rolling back", rb.plan_id);
        apply_snapshot(&rb.original, config);
        config.save();
    }

    // Применить готовые планы без согласия
    let mut st = state.lock().unwrap();
    let ready: Vec<ReadyPlan> = st.ready_plans.drain(..).collect();
    drop(st);

    for plan in ready {
        let snapshot = snapshot_display(&config.display);
        apply_actions(&plan.actions, config);
        config.save();

        if plan.ttl_seconds > 0 {
            let deadline = unix_now() + plan.ttl_seconds;
            state.lock().unwrap().pending_rollbacks.push(PendingRollback {
                deadline_unix: deadline,
                plan_id: plan.plan_id,
                rollback_actions: plan.rollback_actions,
                original: snapshot,
            });
        }
    }

    // Возвращаем запрос на consent если есть
    let st = state.lock().unwrap();
    if let Some(ref pc) = st.pending_consent {
        return Some(ConsentRequest {
            plan_id: pc.plan_id.clone(),
            risk_level: pc.risk_level.clone(),
            summary: pc.summary.clone(),
            actions_human: pc.actions_human.clone(),
        });
    }
    None
}

/// Пользователь разрешил применить план.
pub fn confirm_consent(state: &Arc<Mutex<HotfixState>>, config: &mut AppConfig) {
    let pending = state.lock().unwrap().pending_consent.take();
    if let Some(pc) = pending {
        let snapshot = snapshot_display(&config.display);
        apply_actions(&pc.ready.actions, config);
        config.save();

        if pc.ready.ttl_seconds > 0 {
            let deadline = unix_now() + pc.ready.ttl_seconds;
            state.lock().unwrap().pending_rollbacks.push(PendingRollback {
                deadline_unix: deadline,
                plan_id: pc.ready.plan_id,
                rollback_actions: pc.ready.rollback_actions,
                original: snapshot,
            });
        }
    }
}

/// Пользователь отказался.
pub fn deny_consent(state: &Arc<Mutex<HotfixState>>) {
    state.lock().unwrap().pending_consent = None;
}

// ── Основной фоновый поток ────────────────────────────────────────────────────

fn run_report(
    crash_signature: String,
    component: String,
    error_code: String,
    message: String,
    stack_trace: String,
    config: &HotfixConfig,
    app_config: &AppConfig,
    state: Arc<Mutex<HotfixState>>,
) -> Result<(), String> {
    let api_base = app_config.server.api_url.trim_end_matches('/').to_owned();
    let device_id = app_config.ui.agent_machine_id.clone();

    let fp = collect_fingerprint();
    let incident_id = uuid::Uuid::new_v4().to_string();

    let payload = build_incident_payload(
        incident_id.clone(),
        device_id.clone(),
        crash_signature,
        component,
        error_code,
        message,
        stack_trace,
        &fp,
    );

    let submit_url = format!("{api_base}/api/v1/incidents");
    let resp: SubmitResponse = post_json_auth(&submit_url, &config.api_key, &payload)
        .map_err(|e| format!("submit: {e}"))?;
    let server_incident_id = resp.server_incident_id;

    // Поллинг анализа (до 2 минут, каждые 5 секунд)
    let analysis_url = format!("{api_base}/api/v1/incidents/{server_incident_id}/analysis");
    let mut plan_id = None;
    for _ in 0..24 {
        thread::sleep(Duration::from_secs(5));
        let ar: AnalysisResponse = get_json_auth(&analysis_url, &config.api_key)
            .map_err(|e| format!("poll analysis: {e}"))?;
        match ar.status.as_str() {
            "ready" => {
                plan_id = ar.plan_id;
                break;
            }
            "failed" => return Err("AI analysis failed".to_owned()),
            _ => continue,
        }
    }

    let plan_id = match plan_id {
        Some(id) if !id.is_empty() => id,
        _ => return Ok(()), // timeout или нет плана — нормально
    };

    // Загрузить план
    let plan_url = format!("{api_base}/api/v1/remediation-plans/{plan_id}");
    let plan: PlanResponse = get_json_auth(&plan_url, &config.api_key)
        .map_err(|e| format!("get plan: {e}"))?;

    // Если решение no_action — ничего не делаем
    if plan.decision == "no_action" || plan.decision == "advice_only" {
        return Ok(());
    }

    // Верифицируем подпись
    if !config.signing_public_key.is_empty() {
        verify_plan_signature(&plan.payload, &plan.signature.sig, &config.signing_public_key)
            .map_err(|e| format!("signature verify: {e}"))?;
    }

    // Отправляем decision=accepted
    let decision_url = format!("{api_base}/api/v1/remediation-plans/{plan_id}/decision");
    let _ = post_json_auth::<_, Value>(&decision_url, &config.api_key, &json!({
        "device_id": device_id,
        "decision": "accepted",
        "incident_id": server_incident_id
    }));

    let actions_human: Vec<String> = plan.actions.iter()
        .map(|a| action_human_label(a))
        .collect();

    let ready = ReadyPlan {
        plan_id: plan_id.clone(),
        actions: plan.actions,
        rollback_actions: plan.rollback_actions,
        ttl_seconds: plan.ttl_seconds,
    };

    if plan.requires_user_consent {
        let summary = plan.summary.as_ref()
            .and_then(|s| s.text.clone())
            .unwrap_or_else(|| format!("Изменить настройки ({} действий)", ready.actions.len()));

        let mut st = state.lock().unwrap();
        st.pending_consent = Some(PendingConsent {
            plan_id,
            risk_level: plan.risk_level,
            summary,
            actions_human,
            ready,
        });
    } else {
        state.lock().unwrap().ready_plans.push(ready);
    }

    Ok(())
}

// ── Ed25519 верификация ───────────────────────────────────────────────────────

fn verify_plan_signature(payload: &str, sig_b64: &str, pubkey_b64: &str) -> Result<(), String> {
    let sig_bytes = B64.decode(sig_b64)
        .map_err(|e| format!("sig decode: {e}"))?;
    if sig_bytes.len() != 64 {
        return Err(format!("sig length: expected 64, got {}", sig_bytes.len()));
    }

    let pk_bytes = B64.decode(pubkey_b64)
        .map_err(|e| format!("pubkey decode: {e}"))?;
    let pk: [u8; 32] = pk_bytes.try_into()
        .map_err(|_| "pubkey must be 32 bytes".to_owned())?;

    // libsodium combined mode: signed_message = sig(64) || payload_bytes
    let payload_bytes = payload.as_bytes();
    let mut combined = Vec::with_capacity(64 + payload_bytes.len());
    combined.extend_from_slice(&sig_bytes);
    combined.extend_from_slice(payload_bytes);

    let mut out = Vec::new();
    crypto_sign_open(&mut out, &combined, &pk)
        .map_err(|e| format!("signature invalid: {e:?}"))?;

    Ok(())
}

// ── Применение действий ───────────────────────────────────────────────────────

fn apply_actions(actions: &[PlanAction], config: &mut AppConfig) {
    for action in actions {
        apply_action(action, config);
    }
}

fn apply_action(action: &PlanAction, config: &mut AppConfig) {
    match action.op.as_str() {
        "set_target_fps" => {
            if let Some(v) = action.value.as_u64() {
                config.display.target_fps = v.clamp(5, 120) as u32;
            }
        }
        "set_encoder" => {
            if let Some(s) = action.value.as_str() {
                config.display.encoder = match s {
                    "software" => EncoderPreference::Software,
                    "nvenc" | "videotoolbox" => EncoderPreference::Nvenc,
                    _ => EncoderPreference::Auto,
                };
            }
        }
        "set_fsr_quality" => {
            if let Some(s) = action.value.as_str() {
                config.display.fsr_quality = match s {
                    "off" => FsrQualitySetting::Off,
                    "native" => FsrQualitySetting::Native,
                    "ultra_quality" => FsrQualitySetting::UltraQuality,
                    "quality" => FsrQualitySetting::Quality,
                    "balanced" => FsrQualitySetting::Balanced,
                    "performance" => FsrQualitySetting::Performance,
                    _ => FsrQualitySetting::Off,
                };
            }
        }
        "set_adaptive_quality" => {
            if let Some(b) = action.value.as_bool() {
                config.display.adaptive_quality = b;
            }
        }
        other => {
            eprintln!("[hotfix] unknown op: {other}");
        }
    }
}

fn apply_snapshot(snap: &DisplaySnapshot, config: &mut AppConfig) {
    config.display.target_fps = snap.target_fps;
    config.display.encoder = snap.encoder;
    config.display.fsr_quality = snap.fsr_quality;
}

fn snapshot_display(display: &DisplayConfig) -> DisplaySnapshot {
    DisplaySnapshot {
        target_fps: display.target_fps,
        encoder: display.encoder,
        fsr_quality: display.fsr_quality,
    }
}

fn action_human_label(a: &PlanAction) -> String {
    match a.op.as_str() {
        "set_target_fps" => format!("Установить FPS = {}", a.value),
        "set_encoder" => format!("Сменить энкодер на {}", a.value),
        "set_fsr_quality" => format!("FSR качество = {}", a.value),
        "set_adaptive_quality" => format!("Адаптивное качество = {}", a.value),
        op => format!("{op} = {}", a.value),
    }
}

// ── Сбор отпечатка окружения ─────────────────────────────────────────────────

struct Fingerprint {
    os_family: String,
    distro: String,
    gpu_vendor: String,
    driver_version: String,
    renderer_backend: String,
    evrt_transport: String,
}

fn collect_fingerprint() -> Fingerprint {
    let os_family = std::env::consts::OS.to_owned();

    let distro = {
        #[cfg(target_os = "linux")]
        {
            read_os_release_field("PRETTY_NAME")
                .or_else(|| read_os_release_field("NAME"))
                .unwrap_or_else(|| "Linux".to_owned())
        }
        #[cfg(not(target_os = "linux"))]
        {
            os_family.clone()
        }
    };

    let renderer_backend = std::env::var("WGPU_BACKEND").unwrap_or_default();
    let evrt_transport = std::env::var("EVRT_TRANSPORT").unwrap_or_default();

    Fingerprint {
        os_family,
        distro,
        gpu_vendor: String::new(),
        driver_version: String::new(),
        renderer_backend,
        evrt_transport,
    }
}

#[cfg(target_os = "linux")]
fn read_os_release_field(key: &str) -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(&format!("{key}=")) {
            return Some(rest.trim_matches('"').to_owned());
        }
    }
    None
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

fn post_json_auth<B: Serialize, R: for<'de> Deserialize<'de>>(
    url: &str,
    api_key: &str,
    body: &B,
) -> Result<R, String> {
    match ureq::post(url)
        .timeout(Duration::from_secs(30))
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Content-Type", "application/json")
        .send_json(serde_json::to_value(body).map_err(|e| e.to_string())?)
    {
        Ok(resp) => resp
            .into_json::<R>()
            .map_err(|e| format!("parse response: {e}")),
        Err(UreqError::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("HTTP {code}: {text}"))
        }
        Err(UreqError::Transport(e)) => Err(format!("network: {e}")),
    }
}

fn get_json_auth<R: for<'de> Deserialize<'de>>(url: &str, api_key: &str) -> Result<R, String> {
    match ureq::get(url)
        .timeout(Duration::from_secs(30))
        .set("Authorization", &format!("Bearer {api_key}"))
        .call()
    {
        Ok(resp) => resp
            .into_json::<R>()
            .map_err(|e| format!("parse response: {e}")),
        Err(UreqError::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("HTTP {code}: {text}"))
        }
        Err(UreqError::Transport(e)) => Err(format!("network: {e}")),
    }
}

// ── Utils ─────────────────────────────────────────────────────────────────────

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// SHA-256 дедупликационный хеш — совпадает с сервером.
#[allow(dead_code)]
pub fn dedup_hash(
    crash_signature: &str,
    component: &str,
    app_version: &str,
    os_family: &str,
    distro: &str,
    gpu_vendor: &str,
    driver_version: &str,
    renderer_backend: &str,
    evrt_transport: &str,
) -> String {
    let input = format!(
        "{crash_signature}|{component}|{app_version}|{os_family}|{distro}|{gpu_vendor}|{driver_version}|{renderer_backend}|{evrt_transport}"
    );
    let hash = Sha256::digest(input.as_bytes());
    format!("{hash:x}")
}
