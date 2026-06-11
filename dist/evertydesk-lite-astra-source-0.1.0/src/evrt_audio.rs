// =============================================================================
// EVRT Audio — разработан Артуром Валиевым (Artur Valiev)
// Порт аудио-пайплайна EvertyGame в Rust.
// WASAPI loopback capture (хост) + WASAPI playback (клиент)
// Транспорт: EVRT TypeAudioFrame UDP пакеты
// =============================================================================

//! EVRT аудио — захват системного звука и воспроизведение.
//!
//! # Архитектура
//! ```text
//! Хост (Windows):
//!   WASAPI loopback → PCM f32 → конвертация i16 → EVRT TypeAudioFrame UDP
//!
//! Клиент (Windows):
//!   EVRT TypeAudioFrame UDP → AudioReassembler → PCM i16 → WASAPI playback
//! ```
//!
//! Формат: PCM 16-bit stereo 48000 Hz (совместим с любым устройством).
//! Фрейм: 480 сэмплов (10 мс при 48kHz) = 1920 байт → всегда влезает в один UDP пакет.

// Полная API-поверхность EVRT-протокола: часть методов — публичный
// интерфейс для будущего использования (enhancement layer, audio config, jitter API).
#![allow(dead_code)]

use std::{
    collections::VecDeque,
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use crate::evrt;

const AUDIO_PREBUFFER_MS: u32 = 40;
const AUDIO_MAX_BUFFER_MS: u32 = 180;

// ─── AudioConfig ─────────────────────────────────────────────────────────────

/// Конфигурация аудио-потока.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        }
    }
}

impl AudioConfig {
    /// Сериализовать в JSON для TYPE_AUDIO_CONFIG пакета.
    pub fn to_json(&self) -> Vec<u8> {
        format!(
            r#"{{"sampleRate":{},"channels":{},"bitsPerSample":{}}}"#,
            self.sample_rate, self.channels, self.bits_per_sample
        )
        .into_bytes()
    }

    /// Разобрать из JSON.
    pub fn from_json(payload: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(payload).ok()?;
        let sr = json_u32(s, "sampleRate").unwrap_or(48000);
        let ch = json_u32(s, "channels").unwrap_or(2) as u16;
        let bps = json_u32(s, "bitsPerSample").unwrap_or(16) as u16;
        Some(Self {
            sample_rate: sr,
            channels: ch,
            bits_per_sample: bps,
        })
    }

    /// Байт на сэмпл (для одного канала).
    pub fn bytes_per_sample(&self) -> usize {
        self.bits_per_sample as usize / 8
    }

    /// Байт на фрейм (все каналы).
    pub fn bytes_per_frame(&self) -> usize {
        self.channels as usize * self.bytes_per_sample()
    }
}

// ─── WASAPI Host (захват) ────────────────────────────────────────────────────

/// Запустить захват системного аудио (WASAPI loopback) и отправку по EVRT UDP.
///
/// Блокирует до установки `stop=true`.
/// На не-Windows платформах — no-op.
pub fn run_audio_capture(socket: Arc<UdpSocket>, peer_addr: SocketAddr, stop: Arc<AtomicBool>) {
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = run_audio_capture_windows(socket, peer_addr, stop) {
            eprintln!("[evrt-audio] захват завершился: {e}");
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (socket, peer_addr);
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(target_os = "windows")]
fn run_audio_capture_windows(
    socket: Arc<UdpSocket>,
    peer_addr: SocketAddr,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    use windows::Win32::{
        Media::Audio::{
            eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
            MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
            WAVEFORMATEX, WAVE_FORMAT_PCM,
        },
        System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        // ── Получить устройство воспроизведения (loopback захватывает его вывод) ─
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("MMDeviceEnumerator: {e}"))?;

        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("GetDefaultAudioEndpoint: {e}"))?;

        let client: IAudioClient = device
            .Activate::<IAudioClient>(CLSCTX_ALL, None)
            .map_err(|e| format!("Activate IAudioClient: {e}"))?;

        // ── Формат: PCM 16-bit stereo 48kHz ──────────────────────────────────
        let cfg = AudioConfig::default();
        let block_align = (cfg.channels * cfg.bits_per_sample / 8) as u16;
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: cfg.channels,
            nSamplesPerSec: cfg.sample_rate,
            nAvgBytesPerSec: cfg.sample_rate * block_align as u32,
            nBlockAlign: block_align,
            wBitsPerSample: cfg.bits_per_sample,
            cbSize: 0,
        };

        // Период буфера: 10 мс в единицах 100-нс
        let buffer_dur: i64 = 100_000; // 10 мс

        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                buffer_dur,
                0,
                &fmt as *const _,
                None,
            )
            .map_err(|e| format!("IAudioClient::Initialize: {e}"))?;

        // ── Отправить AudioConfig клиенту ─────────────────────────────────────
        let cfg_pkt = evrt::build_single(evrt::TYPE_AUDIO_CONFIG, &cfg.to_json());
        let _ = socket.send_to(&cfg_pkt, peer_addr);

        let capture_client: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("GetService IAudioCaptureClient: {e}"))?;

        client
            .Start()
            .map_err(|e| format!("IAudioClient::Start: {e}"))?;

        eprintln!(
            "[evrt-audio] WASAPI loopback старт: {}Hz {}ch {}bit",
            cfg.sample_rate, cfg.channels, cfg.bits_per_sample
        );

        let bytes_per_frame = cfg.bytes_per_frame();
        let mut frame_id: u32 = 0;

        while !stop.load(Ordering::Relaxed) {
            let mut num_frames_available: u32 = 0;
            let mut data_ptr = std::ptr::null_mut::<u8>();
            let mut flags: u32 = 0;
            let mut pts_hns: u64 = 0;

            let hr = capture_client.GetBuffer(
                &mut data_ptr,
                &mut num_frames_available,
                &mut flags,
                None,
                Some(&mut pts_hns),
            );

            if hr.is_err() || num_frames_available == 0 {
                thread::sleep(Duration::from_millis(3));
                continue;
            }

            let byte_len = num_frames_available as usize * bytes_per_frame;
            let pcm_slice = std::slice::from_raw_parts(data_ptr, byte_len);

            // Пакетизируем и отправляем
            frame_id = frame_id.wrapping_add(1);
            let pts_us = pts_hns / 10;
            let pkts = evrt::packetize_audio_frame(frame_id, pts_us, pcm_slice);
            for pkt in &pkts {
                let _ = socket.send_to(pkt, peer_addr);
            }

            capture_client.ReleaseBuffer(num_frames_available).ok();
        }

        client.Stop().ok();
        eprintln!("[evrt-audio] WASAPI loopback стоп");
        Ok(())
    }
}

// ─── WASAPI Client (воспроизведение) ─────────────────────────────────────────

/// Принять аудио-фрейм от хоста и поставить в очередь воспроизведения.
/// Создаёт WASAPI playback при первом вызове.
pub struct AudioPlayer {
    cfg: Option<AudioConfig>,
    queue: VecDeque<Vec<u8>>,
    front_offset: usize,
    buffering: bool,
    #[cfg(target_os = "windows")]
    inner: Option<WasapiPlayer>,
    #[cfg(not(target_os = "windows"))]
    _unused: (),
}

impl AudioPlayer {
    pub fn new() -> Self {
        Self {
            cfg: None,
            queue: VecDeque::new(),
            front_offset: 0,
            buffering: true,
            #[cfg(target_os = "windows")]
            inner: None,
            #[cfg(not(target_os = "windows"))]
            _unused: (),
        }
    }

    /// Инициализировать с заданным форматом.
    pub fn init(&mut self, cfg: &AudioConfig) {
        #[cfg(target_os = "windows")]
        {
            if self.cfg.as_ref() == Some(cfg) && self.inner.is_some() {
                return;
            }
            match WasapiPlayer::new(cfg) {
                Ok(p) => {
                    eprintln!(
                        "[evrt-audio] WASAPI playback инициализирован: {}Hz {}ch",
                        cfg.sample_rate, cfg.channels
                    );
                    self.cfg = Some(cfg.clone());
                    self.queue.clear();
                    self.front_offset = 0;
                    self.buffering = true;
                    self.inner = Some(p);
                }
                Err(e) => eprintln!("[evrt-audio] WASAPI playback init failed: {e}"),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = cfg;
            eprintln!("[evrt-audio] аудио воспроизведение не поддерживается на этой платформе");
        }
    }

    /// Воспроизвести PCM-фрейм.
    pub fn play(&mut self, pcm: &[u8]) {
        #[cfg(target_os = "windows")]
        {
            if pcm.is_empty() {
                return;
            }
            if self.cfg.is_none() || self.inner.is_none() {
                self.init(&AudioConfig::default());
            }
            self.enqueue_pcm(pcm);
            self.pump();
        }
        #[cfg(not(target_os = "windows"))]
        let _ = pcm;
    }

    pub fn tick(&mut self) {
        #[cfg(target_os = "windows")]
        self.pump();
    }

    #[cfg(target_os = "windows")]
    fn enqueue_pcm(&mut self, pcm: &[u8]) {
        self.queue.push_back(pcm.to_vec());
        while self.queued_ms() > AUDIO_MAX_BUFFER_MS && self.queue.len() > 1 {
            self.queue.pop_front();
            self.front_offset = 0;
            self.buffering = false;
        }
    }

    #[cfg(target_os = "windows")]
    fn pump(&mut self) {
        if self.buffering && self.queued_ms() < AUDIO_PREBUFFER_MS {
            return;
        }
        self.buffering = false;

        let Some(inner) = self.inner.as_mut() else {
            return;
        };

        let mut writes = 0;
        while writes < 8 {
            let Some(front_len) = self.queue.front().map(Vec::len) else {
                self.buffering = true;
                self.front_offset = 0;
                break;
            };

            if self.front_offset >= front_len {
                self.queue.pop_front();
                self.front_offset = 0;
                continue;
            }

            let consumed = {
                let front = self.queue.front().expect("front checked above");
                match inner.write_some(&front[self.front_offset..]) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        eprintln!("[evrt-audio] play error: {e}");
                        break;
                    }
                }
            };

            if consumed == 0 {
                break;
            }

            self.front_offset += consumed;
            writes += 1;
        }
    }

    fn queued_ms(&self) -> u32 {
        let Some(cfg) = &self.cfg else {
            return 0;
        };
        pcm_duration_ms(cfg, self.queued_bytes())
    }

    fn queued_bytes(&self) -> usize {
        self.queue
            .iter()
            .enumerate()
            .map(|(idx, chunk)| {
                if idx == 0 {
                    chunk.len().saturating_sub(self.front_offset)
                } else {
                    chunk.len()
                }
            })
            .sum()
    }
}

fn pcm_duration_ms(cfg: &AudioConfig, bytes: usize) -> u32 {
    let bytes_per_frame = cfg.bytes_per_frame().max(1);
    let frames = bytes / bytes_per_frame;
    ((frames as u64).saturating_mul(1000) / u64::from(cfg.sample_rate.max(1))) as u32
}

#[cfg(target_os = "windows")]
struct WasapiPlayer {
    client: windows::Win32::Media::Audio::IAudioClient,
    render_client: windows::Win32::Media::Audio::IAudioRenderClient,
    block_align: usize,
    buffer_frames: u32,
}

#[cfg(target_os = "windows")]
impl WasapiPlayer {
    fn new(cfg: &AudioConfig) -> Result<Self, String> {
        use windows::Win32::{
            Media::Audio::{
                eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
                MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, WAVEFORMATEX, WAVE_FORMAT_PCM,
            },
            System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
        };

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("MMDeviceEnumerator: {e}"))?;

            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("GetDefaultAudioEndpoint: {e}"))?;

            let client: IAudioClient = device
                .Activate::<IAudioClient>(CLSCTX_ALL, None)
                .map_err(|e| format!("Activate: {e}"))?;

            let block_align = (cfg.channels * cfg.bits_per_sample / 8) as u16;
            let fmt = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM as u16,
                nChannels: cfg.channels,
                nSamplesPerSec: cfg.sample_rate,
                nAvgBytesPerSec: cfg.sample_rate * block_align as u32,
                nBlockAlign: block_align,
                wBitsPerSample: cfg.bits_per_sample,
                cbSize: 0,
            };

            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    0,       // no flags for playback
                    200_000, // 20ms буфер
                    0,
                    &fmt as *const _,
                    None,
                )
                .map_err(|e| format!("Initialize: {e}"))?;

            let buffer_frames = client
                .GetBufferSize()
                .map_err(|e| format!("GetBufferSize: {e}"))?;

            let render_client: IAudioRenderClient = client
                .GetService()
                .map_err(|e| format!("GetService IAudioRenderClient: {e}"))?;

            client.Start().map_err(|e| format!("Start: {e}"))?;

            Ok(Self {
                client,
                render_client,
                block_align: block_align as usize,
                buffer_frames,
            })
        }
    }

    fn write_some(&mut self, pcm: &[u8]) -> Result<usize, String> {
        if pcm.is_empty() {
            return Ok(0);
        }

        unsafe {
            let padding = self
                .client
                .GetCurrentPadding()
                .map_err(|e| format!("GetCurrentPadding: {e}"))?;

            let available = self.buffer_frames.saturating_sub(padding);
            let frames_in_data = (pcm.len() / self.block_align) as u32;
            let frames_to_write = frames_in_data.min(available);

            if frames_to_write == 0 {
                return Ok(0);
            }

            let buf_ptr = self
                .render_client
                .GetBuffer(frames_to_write)
                .map_err(|e| format!("GetBuffer: {e}"))?;

            let write_bytes = frames_to_write as usize * self.block_align;
            std::ptr::copy_nonoverlapping(pcm.as_ptr(), buf_ptr, write_bytes.min(pcm.len()));

            self.render_client
                .ReleaseBuffer(frames_to_write, 0)
                .map_err(|e| format!("ReleaseBuffer: {e}"))?;

            Ok(write_bytes)
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for WasapiPlayer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

// ─── Пакетизатор аудио (в evrt.rs уже есть, но добавим хелпер) ───────────────

impl crate::evrt::EvrtPacket {
    /// Является ли пакет аудио-фреймом.
    pub fn is_audio(&self) -> bool {
        self.packet_type == evrt::TYPE_AUDIO_FRAME
    }
    /// Является ли пакет аудио-конфигом.
    pub fn is_audio_config(&self) -> bool {
        self.packet_type == evrt::TYPE_AUDIO_CONFIG
    }
}

// ─── Reassembler аудио ────────────────────────────────────────────────────────
//
// Аудио-фреймы маленькие (≤1920 байт), обычно умещаются в один пакет.
// Используем упрощённый reassembler без сложной логики keyframe.

/// Сборщик аудио-фреймов из UDP-пакетов.
pub struct AudioReassembler {
    frames: std::collections::HashMap<u32, AudioAssembly>,
    latest_id_seen: Option<u32>,
}

struct AudioAssembly {
    parts: Vec<Option<Vec<u8>>>,
    received: u16,
    count: u16,
}

impl AudioReassembler {
    pub fn new() -> Self {
        Self {
            frames: std::collections::HashMap::new(),
            latest_id_seen: None,
        }
    }

    /// Принять пакет. Возвращает `Some(pcm)` когда фрейм собран.
    pub fn on_packet(&mut self, pkt: &evrt::EvrtPacket) -> Option<Vec<u8>> {
        if pkt.packet_count == 0 || pkt.packet_index >= pkt.packet_count {
            return None;
        }
        // Дропаем старые
        if let Some(seen) = self.latest_id_seen {
            if pkt.frame_id < seen.saturating_sub(4) {
                return None;
            }
        }
        if self
            .latest_id_seen
            .map(|s| pkt.frame_id > s)
            .unwrap_or(true)
        {
            // Очищаем фреймы старше текущего на 8
            let cutoff = pkt.frame_id.saturating_sub(8);
            self.frames.retain(|&id, _| id >= cutoff);
            self.latest_id_seen = Some(pkt.frame_id);
        }

        let entry = self
            .frames
            .entry(pkt.frame_id)
            .or_insert_with(|| AudioAssembly {
                parts: vec![None; pkt.packet_count as usize],
                received: 0,
                count: pkt.packet_count,
            });

        if entry.count != pkt.packet_count {
            return None;
        }
        let idx = pkt.packet_index as usize;
        if idx >= entry.parts.len() || entry.parts[idx].is_some() {
            return None;
        }
        entry.parts[idx] = Some(pkt.payload.clone());
        entry.received += 1;

        if entry.received < entry.count {
            return None;
        }

        // Фрейм собран
        let assembly = self.frames.remove(&pkt.frame_id)?;
        let mut out = Vec::new();
        for part in assembly.parts {
            out.extend_from_slice(&part?);
        }
        Some(out)
    }
}

impl Default for AudioReassembler {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Хелперы ─────────────────────────────────────────────────────────────────

fn json_u32(s: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{}\"", key);
    let pos = s.find(&needle)?;
    let rest = s[pos + needle.len()..].trim_start_matches([' ', ':', '\t']);
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

// ─── Тесты ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_config_roundtrip() {
        let cfg = AudioConfig::default();
        let json = cfg.to_json();
        let parsed = AudioConfig::from_json(&json).unwrap();
        assert_eq!(parsed.sample_rate, 48000);
        assert_eq!(parsed.channels, 2);
        assert_eq!(parsed.bits_per_sample, 16);
    }

    #[test]
    fn audio_config_frame_size() {
        let cfg = AudioConfig::default();
        // 480 сэмплов × 2 канала × 2 байта = 1920 байт
        // Это > MAX_PAYLOAD_SIZE(1176) — пакетизируется в 2 UDP пакета, это нормально.
        // Минимальный фрейм 10мс при 48kHz всегда умещается в ≤2 пакета.
        let frame_480 = 480 * cfg.bytes_per_frame();
        assert_eq!(frame_480, 1920);
        let pkts = evrt::packetize_audio_frame(1, 0, &vec![0u8; frame_480]);
        assert!(
            pkts.len() <= 2,
            "аудио фрейм 10мс должен занимать ≤2 UDP пакета"
        );
        for pkt in &pkts {
            assert!(pkt.len() <= evrt::MAX_PACKET_SIZE);
        }
    }

    #[test]
    fn audio_duration_uses_pcm_frame_size() {
        let cfg = AudioConfig::default();
        assert_eq!(pcm_duration_ms(&cfg, 480 * cfg.bytes_per_frame()), 10);
        assert_eq!(pcm_duration_ms(&cfg, 1_920 * cfg.bytes_per_frame()), 40);
    }

    #[test]
    fn audio_player_queue_accounts_for_partial_front_chunk() {
        let cfg = AudioConfig::default();
        let frame_10ms = 480 * cfg.bytes_per_frame();
        let mut player = AudioPlayer::new();
        player.cfg = Some(cfg);
        player.queue.push_back(vec![0u8; frame_10ms * 3]);
        assert_eq!(player.queued_ms(), 30);
        player.front_offset = frame_10ms;
        assert_eq!(player.queued_ms(), 20);
    }

    #[test]
    fn audio_reassembler_single_packet() {
        let pcm = vec![0i16, 1, 2, 3]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect::<Vec<u8>>();
        let pkts = evrt::packetize_audio_frame(1, 0, &pcm);
        assert_eq!(pkts.len(), 1);
        let mut re = AudioReassembler::new();
        let parsed = evrt::parse(&pkts[0], pkts[0].len()).unwrap();
        let result = re.on_packet(&parsed).unwrap();
        assert_eq!(result, pcm);
    }

    #[test]
    fn audio_reassembler_drops_old() {
        let pcm = vec![0u8; 100];
        let pkts = evrt::packetize_audio_frame(1, 0, &pcm);
        let mut re = AudioReassembler::new();
        let parsed = evrt::parse(&pkts[0], pkts[0].len()).unwrap();
        re.on_packet(&parsed); // frame 1

        // Сильно новый фрейм — старый frame_id дропается
        let pkts2 = evrt::packetize_audio_frame(100, 0, &pcm);
        let parsed2 = evrt::parse(&pkts2[0], pkts2[0].len()).unwrap();
        re.on_packet(&parsed2);

        // frame 1 повторно — должен быть дропнут
        let result = re.on_packet(&parsed);
        assert!(result.is_none());
    }
}
