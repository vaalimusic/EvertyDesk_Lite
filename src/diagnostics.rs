// =============================================================================
// Автоматическая диагностика EvertyDesk Lite.
//
// Headless-режим: подключается к хосту, гоняет полную сессию N секунд,
// собирает ВСЮ телеметрию (fps, latency, codec, encode backend, EVRT статус,
// битрейт, потери), агрегирует в структурированный отчёт (JSON + Markdown).
//
// Заменяет ручной цикл «покажи лог» — запустил, получил вердикт.
//
// Запуск:
//   evertydesk-lite --diagnose <remote-id> [password] [--secs N] [--out DIR]
// =============================================================================

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::json;

use crate::settings::AppConfig;
use crate::transport::{
    ConnectionRequest, RemoteDisplay, SessionCommand, SessionEvent, TransportClient,
};

const DIAGNOSTIC_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const SESSION_LOG_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const SUPPORT_REPORT_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MAX_DIAGNOSTIC_RUNS: usize = 10;
const MAX_SESSION_LOGS: usize = 20;
const MAX_SUPPORT_REPORTS: usize = 10;

#[derive(Default, Debug, Clone, Copy)]
pub struct CleanupSummary {
    pub removed_files: usize,
    pub removed_dirs: usize,
    pub errors: usize,
}

impl CleanupSummary {
    fn add(&mut self, other: Self) {
        self.removed_files += other.removed_files;
        self.removed_dirs += other.removed_dirs;
        self.errors += other.errors;
    }

    pub fn removed_total(self) -> usize {
        self.removed_files + self.removed_dirs
    }
}

#[derive(Debug)]
struct ArtifactGroup {
    key: String,
    paths: Vec<PathBuf>,
    modified: SystemTime,
}

pub fn cleanup_default_artifacts() -> CleanupSummary {
    let mut summary = cleanup_diagnostic_runs(Path::new("diagnostics"));
    summary.add(cleanup_session_logs());
    summary.add(cleanup_support_reports());
    summary
}

pub fn cleanup_session_logs() -> CleanupSummary {
    cleanup_files(
        Path::new("logs"),
        SESSION_LOG_RETENTION,
        MAX_SESSION_LOGS,
        |path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("log")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("evertydesk-"))
        },
    )
}

pub fn cleanup_support_reports() -> CleanupSummary {
    cleanup_directories(
        Path::new("reports"),
        SUPPORT_REPORT_RETENTION,
        MAX_SUPPORT_REPORTS,
        |path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("evertydesk-"))
        },
    )
}

pub fn cleanup_diagnostic_runs(dir: &Path) -> CleanupSummary {
    cleanup_diagnostic_runs_with_limits(dir, DIAGNOSTIC_RETENTION, MAX_DIAGNOSTIC_RUNS)
}

fn cleanup_diagnostic_runs_with_limits(
    dir: &Path,
    max_age: Duration,
    max_runs: usize,
) -> CleanupSummary {
    let mut summary = CleanupSummary::default();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return summary,
        Err(_) => {
            summary.errors += 1;
            return summary;
        }
    };

    let mut groups: HashMap<String, ArtifactGroup> = HashMap::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.errors += 1;
                continue;
            }
        };
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !stem.starts_with("diag_")
            || !matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("md" | "json")
            )
        {
            continue;
        }
        let modified = modified_time(&path, &mut summary);
        let group = groups
            .entry(stem.to_owned())
            .or_insert_with(|| ArtifactGroup {
                key: stem.to_owned(),
                paths: Vec::new(),
                modified,
            });
        group.modified = group.modified.max(modified);
        group.paths.push(path);
    }

    let mut groups = groups.into_values().collect::<Vec<_>>();
    groups.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.key.cmp(&a.key)));
    let now = SystemTime::now();
    for (index, group) in groups.into_iter().enumerate() {
        let expired = now
            .duration_since(group.modified)
            .map(|age| age > max_age)
            .unwrap_or(false);
        if !expired && index < max_runs {
            continue;
        }
        for path in group.paths {
            match fs::remove_file(path) {
                Ok(()) => summary.removed_files += 1,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => summary.errors += 1,
            }
        }
    }
    summary
}

fn cleanup_files(
    dir: &Path,
    max_age: Duration,
    max_files: usize,
    include: impl Fn(&Path) -> bool,
) -> CleanupSummary {
    let mut summary = CleanupSummary::default();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return summary,
        Err(_) => {
            summary.errors += 1;
            return summary;
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.errors += 1;
                continue;
            }
        };
        let path = entry.path();
        if path.is_file() && include(&path) {
            files.push((modified_time(&path, &mut summary), path));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let now = SystemTime::now();
    for (index, (modified, path)) in files.into_iter().enumerate() {
        let expired = now
            .duration_since(modified)
            .map(|age| age > max_age)
            .unwrap_or(false);
        if !expired && index < max_files {
            continue;
        }
        match fs::remove_file(path) {
            Ok(()) => summary.removed_files += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => summary.errors += 1,
        }
    }
    summary
}

fn cleanup_directories(
    dir: &Path,
    max_age: Duration,
    max_dirs: usize,
    include: impl Fn(&Path) -> bool,
) -> CleanupSummary {
    let mut summary = CleanupSummary::default();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return summary,
        Err(_) => {
            summary.errors += 1;
            return summary;
        }
    };
    let mut directories = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.errors += 1;
                continue;
            }
        };
        let path = entry.path();
        if path.is_dir() && include(&path) {
            directories.push((modified_time(&path, &mut summary), path));
        }
    }
    directories.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let now = SystemTime::now();
    for (index, (modified, path)) in directories.into_iter().enumerate() {
        let expired = now
            .duration_since(modified)
            .map(|age| age > max_age)
            .unwrap_or(false);
        if !expired && index < max_dirs {
            continue;
        }
        match fs::remove_dir_all(path) {
            Ok(()) => summary.removed_dirs += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => summary.errors += 1,
        }
    }
    summary
}

fn modified_time(path: &Path, summary: &mut CleanupSummary) -> SystemTime {
    match fs::metadata(path).and_then(|metadata| metadata.modified()) {
        Ok(modified) => modified,
        Err(_) => {
            summary.errors += 1;
            UNIX_EPOCH
        }
    }
}

// ─── собранные метрики ────────────────────────────────────────────────────────

#[derive(Default)]
struct Collected {
    // Подключение
    connected: bool,
    connect_ms: u64,
    peer_info: String,
    fail_reason: Option<String>,

    // Видео
    frames_received: u64,
    first_frame_ms: Option<u64>,
    codec: String,
    last_width: usize,
    last_height: usize,

    // FPS/битрейт (из VideoPacketMetrics / FrameMetrics)
    input_fps_samples: Vec<f32>,
    input_kbps_samples: Vec<u64>,
    decode_ms_samples: Vec<u64>,
    queue_ms_samples: Vec<u64>,
    dropped_total: usize,

    // Latency
    latency_samples: Vec<u32>,

    // EVRT
    evrt_active: bool,
    evrt_connected: bool,
    evrt_host_addr: String,
    evrt_pressure: Vec<String>,
    evrt_arrival_ms: Vec<i32>,
    evrt_assembly_ms: Vec<i32>,
    evrt_decode_ms: Vec<i32>,
    evrt_jitter_ms: Vec<u32>,
    evrt_fps: Vec<u32>,
    evrt_bitrate_mbps: Vec<f32>,
    evrt_packets_received: u64,
    evrt_frames_assembled: u64,
    evrt_reassembly_drops: u64,
    evrt_queue_drops: u64,

    // Дисплеи
    displays: Vec<RemoteDisplay>,

    // Сырой журнал Info/Progress (для ★ строк бэкенда и EVRT)
    info_log: Vec<String>,
}

impl Collected {
    fn ingest(&mut self, ev: &SessionEvent, started: Instant) {
        match ev {
            SessionEvent::Connected(info) => {
                self.connected = true;
                self.connect_ms = started.elapsed().as_millis() as u64;
                self.peer_info = info.clone();
            }
            SessionEvent::Failed(err) => {
                self.fail_reason = Some(err.clone());
            }
            SessionEvent::Frame {
                codec,
                width,
                height,
                ..
            } => {
                self.frames_received += 1;
                if self.first_frame_ms.is_none() {
                    self.first_frame_ms = Some(started.elapsed().as_millis() as u64);
                }
                self.codec = codec.clone();
                self.last_width = *width;
                self.last_height = *height;
            }
            SessionEvent::FrameMetrics {
                queue_ms,
                decode_ms,
                dropped,
                ..
            } => {
                self.queue_ms_samples.push(*queue_ms);
                self.decode_ms_samples.push(*decode_ms);
                self.dropped_total += *dropped;
            }
            SessionEvent::VideoPacketMetrics {
                input_fps,
                input_kbps,
            } => {
                self.input_fps_samples.push(*input_fps);
                self.input_kbps_samples.push(*input_kbps);
            }
            SessionEvent::Latency(ms) => {
                self.latency_samples.push(*ms);
            }
            SessionEvent::Displays(d) => {
                self.displays = d.clone();
            }
            SessionEvent::EvrtStatus {
                active,
                host_addr,
                port,
            } => {
                self.evrt_active = *active;
                if *active {
                    self.evrt_connected = true;
                    self.evrt_host_addr = format!("{host_addr}:{port}");
                }
            }
            SessionEvent::EvrtMetrics {
                pressure,
                arrival_delta_ms,
                assembly_delay_ms,
                decode_delta_ms,
                jitter_ms,
                bitrate_mbps,
                fps,
                packets_received,
                frames_assembled,
                reassembly_drops,
                queue_drops,
            } => {
                self.evrt_pressure.push(pressure.clone());
                self.evrt_arrival_ms.push(*arrival_delta_ms);
                self.evrt_assembly_ms.push(*assembly_delay_ms);
                self.evrt_decode_ms.push(*decode_delta_ms);
                self.evrt_jitter_ms.push(*jitter_ms);
                self.evrt_fps.push(*fps);
                self.evrt_bitrate_mbps.push(*bitrate_mbps);
                self.evrt_packets_received = self.evrt_packets_received.max(*packets_received);
                self.evrt_frames_assembled = self.evrt_frames_assembled.max(*frames_assembled);
                self.evrt_reassembly_drops = self.evrt_reassembly_drops.max(*reassembly_drops);
                self.evrt_queue_drops = self.evrt_queue_drops.max(*queue_drops);
            }
            SessionEvent::Info(msg) => {
                self.info_log.push(msg.clone());
            }
            _ => {}
        }
    }
}

// ─── статистика ───────────────────────────────────────────────────────────────

fn avg_f32(v: &[f32]) -> f32 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    }
}
fn avg_u64(v: &[u64]) -> u64 {
    if v.is_empty() {
        0
    } else {
        v.iter().sum::<u64>() / v.len() as u64
    }
}
fn avg_u32(v: &[u32]) -> u32 {
    if v.is_empty() {
        0
    } else {
        (v.iter().map(|x| *x as u64).sum::<u64>() / v.len() as u64) as u32
    }
}

fn avg_nonzero_u32(v: &[u32]) -> u32 {
    let mut count = 0_u64;
    let sum = v
        .iter()
        .filter(|value| **value > 0)
        .map(|value| {
            count += 1;
            u64::from(*value)
        })
        .sum::<u64>();
    if count == 0 {
        0
    } else {
        (sum / count) as u32
    }
}
fn avg_i32(v: &[i32]) -> i32 {
    if v.is_empty() {
        0
    } else {
        (v.iter().map(|x| *x as i64).sum::<i64>() / v.len() as i64) as i32
    }
}
fn avg_nonnegative_i32(v: &[i32]) -> i32 {
    let samples = v.iter().copied().filter(|value| *value >= 0);
    let (sum, count) = samples.fold((0_i64, 0_i64), |(sum, count), value| {
        (sum + i64::from(value), count + 1)
    });
    if count == 0 {
        0
    } else {
        (sum / count) as i32
    }
}
fn max_f32(v: &[f32]) -> f32 {
    v.iter().cloned().fold(0.0, f32::max)
}
fn min_f32(v: &[f32]) -> f32 {
    v.iter().cloned().fold(f32::INFINITY, f32::min)
}

fn effective_fps(c: &Collected) -> f32 {
    if c.evrt_connected {
        avg_nonzero_u32(&c.evrt_fps) as f32
    } else {
        avg_f32(&c.input_fps_samples)
    }
}

// ─── точка входа CLI ──────────────────────────────────────────────────────────

/// Запустить автоматическую диагностику. Возвращает exit-code.
pub fn run_diagnose(remote_id: &str, password: &str, secs: u64, out_dir: &str) -> i32 {
    let config = AppConfig::load_or_create();
    let request = ConnectionRequest {
        remote_id: remote_id.to_owned(),
        password: password.to_owned(),
        client_id: config.local_id.clone(),
        client_name: std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "EvertyDesk Diagnostics".to_owned()),
        server: config.server.clone(),
        display: config.display.clone(),
        control_only: false,
        audio_enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        evrt2_only: false,
        require_direct_transport: false,
        network_debug: config.network_debug.clone(),
    };

    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║  EvertyDesk Lite — автоматическая диагностика             ║");
    eprintln!("╚══════════════════════════════════════════════════════════╝");
    eprintln!("Хост:        {remote_id}");
    eprintln!("Длительность: {secs}s");
    eprintln!();

    let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCommand>();
    let (ev_tx, ev_rx) = mpsc::channel::<SessionEvent>();

    let started = Instant::now();
    let session = std::thread::spawn(move || {
        let no_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        TransportClient::run_session(request, cmd_rx, ev_tx, no_stop);
    });

    let mut collected = Collected::default();
    let deadline = started + Duration::from_secs(secs);

    // Собираем события до дедлайна
    while Instant::now() < deadline {
        match ev_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ev) => {
                // Печатаем важные строки в реальном времени
                match &ev {
                    SessionEvent::Progress(p, m) => eprintln!("  [{p}%] {m}"),
                    SessionEvent::Connected(i) => eprintln!("  ✓ Подключено: {i}"),
                    SessionEvent::Failed(e) => eprintln!("  ✗ Ошибка: {e}"),
                    SessionEvent::Info(m) if m.contains('★') => eprintln!("  {m}"),
                    SessionEvent::EvrtStatus {
                        active: true,
                        host_addr,
                        port,
                    } => eprintln!("  ⚡ EVRT активен → {host_addr}:{port}"),
                    _ => {}
                }
                collected.ingest(&ev, started);
                if collected.fail_reason.is_some() && !collected.connected {
                    break; // ранний выход при ошибке подключения
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Останавливаем сессию
    let _ = cmd_tx.send(SessionCommand::Close);
    drop(cmd_tx);
    // Сессия может не завершиться мгновенно — даём ей 2с, потом отпускаем
    let _ = session;

    // ── Генерируем отчёт ──────────────────────────────────────────────────────
    let report = build_report(&collected, secs);
    eprintln!("\n{report}\n");

    // Записываем файлы
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let out_dir = Path::new(out_dir);
    let _ = cleanup_diagnostic_runs(out_dir);
    let _ = fs::create_dir_all(out_dir);
    let md_path = out_dir.join(format!("diag_{ts}.md"));
    let json_path = out_dir.join(format!("diag_{ts}.json"));
    if fs::write(&md_path, &report).is_ok() {
        eprintln!("📄 Markdown отчёт: {}", md_path.display());
    }
    if fs::write(&json_path, build_json(&collected)).is_ok() {
        eprintln!("📊 JSON данные:    {}", json_path.display());
    }
    let _ = cleanup_diagnostic_runs(out_dir);

    // Exit code: 0 если подключились и получили кадры, иначе ошибка
    if collected.fail_reason.is_some() && !collected.connected {
        1
    } else if collected.frames_received == 0 {
        2 // подключились, но видео не пошло
    } else {
        0
    }
}

// ─── отчёт ────────────────────────────────────────────────────────────────────

fn build_report(c: &Collected, secs: u64) -> String {
    let mut s = String::new();
    s.push_str("# Диагностический отчёт EvertyDesk Lite\n\n");

    // Вердикт
    let verdict = if c.fail_reason.is_some() && !c.connected {
        "❌ ПОДКЛЮЧЕНИЕ НЕ УДАЛОСЬ"
    } else if c.frames_received == 0 {
        "⚠️ ПОДКЛЮЧИЛИСЬ, НО ВИДЕО НЕ ПОШЛО"
    } else if effective_fps(c) >= 20.0 {
        "✅ РАБОТАЕТ ХОРОШО"
    } else if effective_fps(c) >= 8.0 {
        "🟡 РАБОТАЕТ, НО МЕДЛЕННО"
    } else {
        "🔴 ОЧЕНЬ НИЗКИЙ FPS"
    };
    s.push_str(&format!("## Вердикт: {verdict}\n\n"));

    // Подключение
    s.push_str("## Подключение\n");
    s.push_str(&format!(
        "- Статус: {}\n",
        if c.connected {
            "✓ подключено"
        } else {
            "✗ не подключено"
        }
    ));
    if c.connected {
        s.push_str(&format!("- Время подключения: {} мс\n", c.connect_ms));
        s.push_str(&format!("- Хост: {}\n", c.peer_info));
    }
    if let Some(ref e) = c.fail_reason {
        s.push_str(&format!("- Ошибка: {e}\n"));
    }
    s.push('\n');

    // Транспорт
    s.push_str("## Транспорт\n");
    if c.evrt_connected {
        s.push_str(&format!(
            "- ⚡ **EVRT прямой UDP** активен → {}\n",
            c.evrt_host_addr
        ));
        if !c.evrt_arrival_ms.is_empty() {
            s.push_str(&format!(
                "- EVRT arrival delta: avg {} мс\n",
                avg_i32(&c.evrt_arrival_ms)
            ));
            s.push_str(&format!(
                "- EVRT assembly delay: avg {} ms\n",
                avg_nonnegative_i32(&c.evrt_assembly_ms)
            ));
            s.push_str(&format!(
                "- EVRT jitter: avg {} мс\n",
                avg_u32(&c.evrt_jitter_ms)
            ));
            s.push_str(&format!(
                "- EVRT decode FPS: avg {}\n",
                avg_nonzero_u32(&c.evrt_fps)
            ));
            s.push_str(&format!(
                "- EVRT decode delta: avg {} мс\n",
                avg_nonnegative_i32(&c.evrt_decode_ms)
            ));
            s.push_str(&format!(
                "- EVRT packets/assembled: {}/{}\n",
                c.evrt_packets_received, c.evrt_frames_assembled
            ));
            s.push_str(&format!(
                "- EVRT drops: reassembly={} queue={}\n",
                c.evrt_reassembly_drops, c.evrt_queue_drops
            ));
            let crit = c
                .evrt_pressure
                .iter()
                .filter(|p| p.as_str() == "critical")
                .count();
            s.push_str(&format!(
                "- EVRT pressure critical: {}/{} тиков\n",
                crit,
                c.evrt_pressure.len()
            ));
        }
    } else {
        s.push_str("- 📡 TCP relay (EVRT не активировался)\n");
        // Подсказка почему
        if c.info_log.iter().any(|m| m.contains("EvrtEndpoints")) {
            s.push_str("  - Хост прислал EVRT кандидаты, но punch не прошёл (VPN/firewall?)\n");
        }
    }
    s.push('\n');

    // Видео
    s.push_str("## Видео\n");
    s.push_str(&format!(
        "- Кодек: {}\n",
        if c.codec.is_empty() { "—" } else { &c.codec }
    ));
    s.push_str(&format!(
        "- Разрешение: {}x{}\n",
        c.last_width, c.last_height
    ));
    s.push_str(&format!(
        "- Кадров получено: {} за {}s\n",
        c.frames_received, secs
    ));
    if let Some(ff) = c.first_frame_ms {
        s.push_str(&format!("- Первый кадр через: {} мс\n", ff));
    }
    if c.evrt_connected {
        s.push_str(&format!(
            "- **EVRT FPS**: avg {}\n",
            avg_nonzero_u32(&c.evrt_fps)
        ));
        s.push_str("- TCP FPS ниже относится только к резервному relay-потоку\n");
    }
    if !c.input_fps_samples.is_empty() {
        s.push_str(&format!(
            "- **TCP FPS**: avg {:.1}, min {:.1}, max {:.1}\n",
            avg_f32(&c.input_fps_samples),
            min_f32(&c.input_fps_samples),
            max_f32(&c.input_fps_samples),
        ));
    }
    if !c.input_kbps_samples.is_empty() {
        s.push_str(&format!(
            "- **TCP битрейт**: avg {} kbps\n",
            avg_u64(&c.input_kbps_samples)
        ));
    }
    if !c.decode_ms_samples.is_empty() {
        s.push_str(&format!(
            "- Декод: avg {} мс\n",
            avg_u64(&c.decode_ms_samples)
        ));
    }
    if !c.queue_ms_samples.is_empty() {
        s.push_str(&format!(
            "- Очередь декодера: avg {} мс\n",
            avg_u64(&c.queue_ms_samples)
        ));
    }
    s.push_str(&format!("- Дропнуто кадров: {}\n", c.dropped_total));
    s.push('\n');

    // ── Хост-энкодер (из HostTelemetry) ───────────────────────────────────────
    let host_enc: Vec<_> = c
        .info_log
        .iter()
        .filter(|m| m.contains("Хост-энкодер"))
        .collect();
    if let Some(last) = host_enc.last() {
        s.push_str("## Хост-энкодер (реальный)\n");
        s.push_str(&format!("- {}\n", last.replace("★ Хост-энкодер: ", "")));
        if last.contains("OpenH264-SW") {
            s.push_str("- ⚠️ СОФТВЕРНЫЙ энкодер! Нет аппаратного (NVENC/MF). Соберите хост с NV Codec SDK.\n");
        } else if last.contains("NVENC") {
            s.push_str("- ✅ Аппаратный NVENC\n");
        } else if last.contains("MediaFoundation") {
            s.push_str("- ✅ Аппаратный Media Foundation\n");
        }
        s.push('\n');
    } else if c.frames_received > 0 {
        // Видео идёт, но телеметрии хоста нет → хост на старом билде.
        s.push_str("## Хост-энкодер (реальный)\n");
        s.push_str("- ❓ Телеметрия хоста НЕ получена, хотя видео идёт.\n");
        s.push_str("- 👉 **Хост запущен со СТАРЫМ билдом** (до telemetry/downscale).\n");
        s.push_str("- Останови процесс хоста полностью, пересобери (`cargo build --release`),\n");
        s.push_str("  запусти НОВЫЙ бинарь. В консоли хоста должна быть строка `🔧 ... ХОСТ запущен | build=HH:MM:SS`.\n\n");
    }

    // Предупреждение про статичный экран при headless-диагностике
    if effective_fps(c) < 20.0 && c.frames_received > 0 {
        s.push_str("> ⚠️ Низкий FPS может быть из-за статичного экрана хоста во время теста\n");
        s.push_str("> (детектор изменений пропускает неизменные кадры — это норма).\n");
        s.push_str("> Для реального замера двигай окна/видео на хосте во время диагностики,\n");
        s.push_str("> и смотри `encode_ms` хост-энкодера выше — он от активности не зависит.\n\n");
    }

    // Latency
    if !c.latency_samples.is_empty() {
        s.push_str("## Задержка\n");
        s.push_str(&format!(
            "- RTT: avg {} мс\n\n",
            avg_u32(&c.latency_samples)
        ));
    }

    // Дисплеи
    s.push_str("## Дисплеи\n");
    if c.displays.is_empty() {
        s.push_str("- (не получены)\n");
    } else {
        for d in &c.displays {
            s.push_str(&format!(
                "- #{}: {}x{} @ ({},{}) {}\n",
                d.index, d.width, d.height, d.x, d.y, d.name
            ));
        }
    }
    s.push('\n');

    // Диагностические ★ строки
    let stars: Vec<_> = c.info_log.iter().filter(|m| m.contains('★')).collect();
    if !stars.is_empty() {
        s.push_str("## Диагностика хоста (★)\n");
        for m in stars {
            s.push_str(&format!("- {m}\n"));
        }
        s.push('\n');
    }

    s
}

fn build_json(c: &Collected) -> String {
    serde_json::to_string_pretty(&json!({
        "connected": c.connected,
        "connect_ms": c.connect_ms,
        "peer_info": c.peer_info,
        "fail_reason": c.fail_reason,
        "frames_received": c.frames_received,
        "first_frame_ms": c.first_frame_ms,
        "codec": c.codec,
        "resolution": format!("{}x{}", c.last_width, c.last_height),
        "effective_fps_avg": effective_fps(c),
        "tcp_fps_avg": avg_f32(&c.input_fps_samples),
        "fps_avg": avg_f32(&c.input_fps_samples),
        "fps_min": min_f32(&c.input_fps_samples).max(0.0),
        "fps_max": max_f32(&c.input_fps_samples),
        "bitrate_kbps_avg": avg_u64(&c.input_kbps_samples),
        "decode_ms_avg": avg_u64(&c.decode_ms_samples),
        "queue_ms_avg": avg_u64(&c.queue_ms_samples),
        "dropped_total": c.dropped_total,
        "latency_ms_avg": avg_u32(&c.latency_samples),
        "evrt_active": c.evrt_active,
        "evrt_connected": c.evrt_connected,
        "evrt_host_addr": c.evrt_host_addr,
        "evrt_arrival_ms_avg": avg_i32(&c.evrt_arrival_ms),
        "evrt_assembly_ms_avg": avg_nonnegative_i32(&c.evrt_assembly_ms),
        "evrt_decode_ms_avg": avg_nonnegative_i32(&c.evrt_decode_ms),
        "evrt_jitter_ms_avg": avg_u32(&c.evrt_jitter_ms),
        "evrt_fps_avg": avg_nonzero_u32(&c.evrt_fps),
        "evrt_bitrate_mbps_avg": avg_f32(&c.evrt_bitrate_mbps),
        "evrt_packets_received": c.evrt_packets_received,
        "evrt_frames_assembled": c.evrt_frames_assembled,
        "evrt_reassembly_drops": c.evrt_reassembly_drops,
        "evrt_queue_drops": c.evrt_queue_drops,
        "displays": c.displays.iter().map(|display| json!({
            "index": display.index,
            "name": display.name,
            "width": display.width,
            "height": display.height,
            "x": display.x,
            "y": display.y,
            "cursor_embedded": display.cursor_embedded,
        })).collect::<Vec<_>>(),
    }))
    .unwrap_or_else(|err| format!(r#"{{"error":"JSON serialization failed: {err}"}}"#))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("evertydesk-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn diagnostic_cleanup_removes_oldest_run_as_a_pair() {
        let dir = temp_dir("diagnostic-cleanup");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("host_diag.md"), "keep").unwrap();
        for run in 1..=3 {
            fs::write(dir.join(format!("diag_{run}.md")), "report").unwrap();
            fs::write(dir.join(format!("diag_{run}.json")), "{}").unwrap();
        }

        let summary =
            cleanup_diagnostic_runs_with_limits(&dir, Duration::from_secs(365 * 24 * 60 * 60), 2);

        assert_eq!(summary.removed_files, 2);
        assert!(!dir.join("diag_1.md").exists());
        assert!(!dir.join("diag_1.json").exists());
        assert!(dir.join("diag_2.md").exists());
        assert!(dir.join("diag_3.json").exists());
        assert!(dir.join("host_diag.md").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn diagnostic_json_escapes_windows_display_names() {
        let collected = Collected {
            peer_info: r#"hostname=host displays=\\.\DISPLAY1"#.to_owned(),
            displays: vec![RemoteDisplay {
                index: 0,
                name: r#"\\.\DISPLAY1"#.to_owned(),
                width: 2560,
                height: 1440,
                x: 0,
                y: 0,
                cursor_embedded: false,
            }],
            ..Collected::default()
        };

        let body = build_json(&collected);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["displays"][0]["name"], r#"\\.\DISPLAY1"#);
    }

    #[test]
    fn file_and_directory_cleanup_respect_count_limits() {
        let root = temp_dir("artifact-cleanup");
        let logs = root.join("logs");
        let reports = root.join("reports");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&reports).unwrap();
        for index in 1..=3 {
            fs::write(logs.join(format!("evertydesk-{index}.log")), "log").unwrap();
            let report = reports.join(format!("evertydesk-{index}"));
            fs::create_dir_all(&report).unwrap();
            fs::write(report.join("summary.txt"), "report").unwrap();
        }

        let file_summary =
            cleanup_files(&logs, Duration::from_secs(365 * 24 * 60 * 60), 2, |_| true);
        let dir_summary =
            cleanup_directories(&reports, Duration::from_secs(365 * 24 * 60 * 60), 2, |_| {
                true
            });

        assert_eq!(file_summary.removed_files, 1);
        assert_eq!(dir_summary.removed_dirs, 1);
        assert_eq!(fs::read_dir(&logs).unwrap().count(), 2);
        assert_eq!(fs::read_dir(&reports).unwrap().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }
}
