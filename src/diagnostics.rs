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
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::transport::{
    ConnectionRequest, RemoteDisplay, SessionCommand, SessionEvent, TransportClient,
};
use crate::settings::AppConfig;

// ─── собранные метрики ────────────────────────────────────────────────────────

#[derive(Default)]
struct Collected {
    // Подключение
    connected:       bool,
    connect_ms:      u64,
    peer_info:       String,
    fail_reason:     Option<String>,

    // Видео
    frames_received: u64,
    first_frame_ms:  Option<u64>,
    codec:           String,
    last_width:      usize,
    last_height:     usize,

    // FPS/битрейт (из VideoPacketMetrics / FrameMetrics)
    input_fps_samples:   Vec<f32>,
    input_kbps_samples:  Vec<u64>,
    decode_ms_samples:   Vec<u64>,
    queue_ms_samples:    Vec<u64>,
    dropped_total:       usize,

    // Latency
    latency_samples: Vec<u32>,

    // EVRT
    evrt_active:      bool,
    evrt_host_addr:   String,
    evrt_pressure:    Vec<String>,
    evrt_arrival_ms:  Vec<i32>,
    evrt_jitter_ms:   Vec<u32>,

    // Дисплеи
    displays:        Vec<RemoteDisplay>,

    // Сырой журнал Info/Progress (для ★ строк бэкенда и EVRT)
    info_log:        Vec<String>,
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
            SessionEvent::Frame { codec, width, height, .. } => {
                self.frames_received += 1;
                if self.first_frame_ms.is_none() {
                    self.first_frame_ms = Some(started.elapsed().as_millis() as u64);
                }
                self.codec = codec.clone();
                self.last_width = *width;
                self.last_height = *height;
            }
            SessionEvent::FrameMetrics { queue_ms, decode_ms, dropped, .. } => {
                self.queue_ms_samples.push(*queue_ms);
                self.decode_ms_samples.push(*decode_ms);
                self.dropped_total += *dropped;
            }
            SessionEvent::VideoPacketMetrics { input_fps, input_kbps } => {
                self.input_fps_samples.push(*input_fps);
                self.input_kbps_samples.push(*input_kbps);
            }
            SessionEvent::Latency(ms) => {
                self.latency_samples.push(*ms);
            }
            SessionEvent::Displays(d) => {
                self.displays = d.clone();
            }
            SessionEvent::EvrtStatus { active, host_addr, port } => {
                self.evrt_active = *active;
                if *active {
                    self.evrt_host_addr = format!("{host_addr}:{port}");
                }
            }
            SessionEvent::EvrtMetrics { pressure, arrival_delta_ms, jitter_ms, .. } => {
                self.evrt_pressure.push(pressure.clone());
                self.evrt_arrival_ms.push(*arrival_delta_ms);
                self.evrt_jitter_ms.push(*jitter_ms);
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
    if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
}
fn avg_u64(v: &[u64]) -> u64 {
    if v.is_empty() { 0 } else { v.iter().sum::<u64>() / v.len() as u64 }
}
fn avg_u32(v: &[u32]) -> u32 {
    if v.is_empty() { 0 } else { (v.iter().map(|x| *x as u64).sum::<u64>() / v.len() as u64) as u32 }
}
fn avg_i32(v: &[i32]) -> i32 {
    if v.is_empty() { 0 } else { (v.iter().map(|x| *x as i64).sum::<i64>() / v.len() as i64) as i32 }
}
fn max_f32(v: &[f32]) -> f32 {
    v.iter().cloned().fold(0.0, f32::max)
}
fn min_f32(v: &[f32]) -> f32 {
    v.iter().cloned().fold(f32::INFINITY, f32::min)
}

// ─── точка входа CLI ──────────────────────────────────────────────────────────

/// Запустить автоматическую диагностику. Возвращает exit-code.
pub fn run_diagnose(remote_id: &str, password: &str, secs: u64, out_dir: &str) -> i32 {
    let config = AppConfig::load_or_create();
    let request = ConnectionRequest {
        remote_id: remote_id.to_owned(),
        password: password.to_owned(),
        server: config.server.clone(),
        display: config.display.clone(),
    };

    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║  EvertyDesk Lite — автоматическая диагностика             ║");
    eprintln!("╚══════════════════════════════════════════════════════════╝");
    eprintln!("Хост:        {remote_id}");
    eprintln!("Длительность: {secs}s");
    eprintln!();

    let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCommand>();
    let (ev_tx, ev_rx)   = mpsc::channel::<SessionEvent>();

    let started = Instant::now();
    let session = std::thread::spawn(move || {
        TransportClient::run_session(request, cmd_rx, ev_tx);
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
                    SessionEvent::Connected(i)   => eprintln!("  ✓ Подключено: {i}"),
                    SessionEvent::Failed(e)      => eprintln!("  ✗ Ошибка: {e}"),
                    SessionEvent::Info(m) if m.contains('★') => eprintln!("  {m}"),
                    SessionEvent::EvrtStatus { active: true, host_addr, port } =>
                        eprintln!("  ⚡ EVRT активен → {host_addr}:{port}"),
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
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _ = std::fs::create_dir_all(out_dir);
    let md_path   = format!("{out_dir}/diag_{ts}.md");
    let json_path = format!("{out_dir}/diag_{ts}.json");
    if std::fs::write(&md_path, &report).is_ok() {
        eprintln!("📄 Markdown отчёт: {md_path}");
    }
    if std::fs::write(&json_path, build_json(&collected)).is_ok() {
        eprintln!("📊 JSON данные:    {json_path}");
    }

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
    } else if avg_f32(&c.input_fps_samples) >= 20.0 {
        "✅ РАБОТАЕТ ХОРОШО"
    } else if avg_f32(&c.input_fps_samples) >= 8.0 {
        "🟡 РАБОТАЕТ, НО МЕДЛЕННО"
    } else {
        "🔴 ОЧЕНЬ НИЗКИЙ FPS"
    };
    s.push_str(&format!("## Вердикт: {verdict}\n\n"));

    // Подключение
    s.push_str("## Подключение\n");
    s.push_str(&format!("- Статус: {}\n", if c.connected { "✓ подключено" } else { "✗ не подключено" }));
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
    if c.evrt_active {
        s.push_str(&format!("- ⚡ **EVRT прямой UDP** активен → {}\n", c.evrt_host_addr));
        if !c.evrt_arrival_ms.is_empty() {
            s.push_str(&format!("- EVRT arrival delta: avg {} мс\n", avg_i32(&c.evrt_arrival_ms)));
            s.push_str(&format!("- EVRT jitter: avg {} мс\n", avg_u32(&c.evrt_jitter_ms)));
            let crit = c.evrt_pressure.iter().filter(|p| p.as_str() == "critical").count();
            s.push_str(&format!("- EVRT pressure critical: {}/{} тиков\n", crit, c.evrt_pressure.len()));
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
    s.push_str(&format!("- Кодек: {}\n", if c.codec.is_empty() { "—" } else { &c.codec }));
    s.push_str(&format!("- Разрешение: {}x{}\n", c.last_width, c.last_height));
    s.push_str(&format!("- Кадров получено: {} за {}s\n", c.frames_received, secs));
    if let Some(ff) = c.first_frame_ms {
        s.push_str(&format!("- Первый кадр через: {} мс\n", ff));
    }
    if !c.input_fps_samples.is_empty() {
        s.push_str(&format!(
            "- **FPS**: avg {:.1}, min {:.1}, max {:.1}\n",
            avg_f32(&c.input_fps_samples),
            min_f32(&c.input_fps_samples),
            max_f32(&c.input_fps_samples),
        ));
    }
    if !c.input_kbps_samples.is_empty() {
        s.push_str(&format!("- **Битрейт**: avg {} kbps\n", avg_u64(&c.input_kbps_samples)));
    }
    if !c.decode_ms_samples.is_empty() {
        s.push_str(&format!("- Декод: avg {} мс\n", avg_u64(&c.decode_ms_samples)));
    }
    s.push_str(&format!("- Дропнуто кадров: {}\n", c.dropped_total));
    s.push('\n');

    // ── Хост-энкодер (из HostTelemetry) ───────────────────────────────────────
    let host_enc: Vec<_> = c.info_log.iter().filter(|m| m.contains("Хост-энкодер")).collect();
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
    if avg_f32(&c.input_fps_samples) < 20.0 && c.frames_received > 0 {
        s.push_str("> ⚠️ Низкий FPS может быть из-за статичного экрана хоста во время теста\n");
        s.push_str("> (детектор изменений пропускает неизменные кадры — это норма).\n");
        s.push_str("> Для реального замера двигай окна/видео на хосте во время диагностики,\n");
        s.push_str("> и смотри `encode_ms` хост-энкодера выше — он от активности не зависит.\n\n");
    }

    // Latency
    if !c.latency_samples.is_empty() {
        s.push_str("## Задержка\n");
        s.push_str(&format!("- RTT: avg {} мс\n\n", avg_u32(&c.latency_samples)));
    }

    // Дисплеи
    s.push_str("## Дисплеи\n");
    if c.displays.is_empty() {
        s.push_str("- (не получены)\n");
    } else {
        for d in &c.displays {
            s.push_str(&format!("- #{}: {}x{} @ ({},{}) {}\n",
                d.index, d.width, d.height, d.x, d.y, d.name));
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
    format!(
        r#"{{
  "connected": {},
  "connect_ms": {},
  "peer_info": "{}",
  "fail_reason": {},
  "frames_received": {},
  "first_frame_ms": {},
  "codec": "{}",
  "resolution": "{}x{}",
  "fps_avg": {:.2},
  "fps_min": {:.2},
  "fps_max": {:.2},
  "bitrate_kbps_avg": {},
  "decode_ms_avg": {},
  "dropped_total": {},
  "latency_ms_avg": {},
  "evrt_active": {},
  "evrt_host_addr": "{}",
  "evrt_arrival_ms_avg": {},
  "evrt_jitter_ms_avg": {},
  "displays": {}
}}
"#,
        c.connected,
        c.connect_ms,
        c.peer_info.replace('"', "'"),
        c.fail_reason.as_ref().map(|e| format!("\"{}\"", e.replace('"', "'"))).unwrap_or_else(|| "null".into()),
        c.frames_received,
        c.first_frame_ms.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
        c.codec,
        c.last_width, c.last_height,
        avg_f32(&c.input_fps_samples),
        min_f32(&c.input_fps_samples).max(0.0),
        max_f32(&c.input_fps_samples),
        avg_u64(&c.input_kbps_samples),
        avg_u64(&c.decode_ms_samples),
        c.dropped_total,
        avg_u32(&c.latency_samples),
        c.evrt_active,
        c.evrt_host_addr,
        avg_i32(&c.evrt_arrival_ms),
        avg_u32(&c.evrt_jitter_ms),
        c.displays.len(),
    )
}
