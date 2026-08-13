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
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};

// ─── Android PCM очередь ─────────────────────────────────────────────────────
// Максимум 6 фреймов × 10 мс = 60 мс. При переполнении — дроп старых.
const ANDROID_QUEUE_MAX_FRAMES: usize = 256; // 256 × 10мс = ~2.5с буфер против джиттера

fn android_queue() -> &'static Mutex<VecDeque<Vec<u8>>> {
    static Q: OnceLock<Mutex<VecDeque<Vec<u8>>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(VecDeque::with_capacity(ANDROID_QUEUE_MAX_FRAMES + 1)))
}

/// Сохранённый sample rate из AudioConfig (для JNI запросов).
/// 0 = AudioConfig ещё не получен, fallback = 48000.
static AUDIO_SAMPLE_RATE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Пушить PCM фрейм в Android очередь (вызывается из evrt_client.rs на Android).
pub fn push_android_audio(pcm: Vec<u8>) {
    if let Ok(mut q) = android_queue().lock() {
        while q.len() >= ANDROID_QUEUE_MAX_FRAMES {
            q.pop_front();
        }
        q.push_back(pcm);
    }
}

/// Достать один PCM фрейм из Android очереди (вызывается из JNI).
pub fn pop_android_audio() -> Option<Vec<u8>> {
    android_queue().lock().ok()?.pop_front()
}

/// Текущая глубина очереди в фреймах (без изъятия). Используется jitter buffer.
pub fn android_queue_depth() -> usize {
    android_queue().lock().ok().map(|q| q.len()).unwrap_or(0)
}

/// Сохранить sample_rate из AudioConfig (вызывается из evrt_client при получении AudioConfig).
pub fn set_audio_sample_rate(rate: u32) {
    AUDIO_SAMPLE_RATE.store(rate, std::sync::atomic::Ordering::Relaxed);
}

/// Получить последний известный sample_rate. 0 если AudioConfig ещё не получен.
pub fn get_audio_sample_rate() -> u32 {
    AUDIO_SAMPLE_RATE.load(std::sync::atomic::Ordering::Relaxed)
}

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
/// `on_tcp_frame`: optional mirror callback, invoked with each raw PCM chunk
/// (i16 stereo 48kHz, 1920 bytes) alongside the existing EVRT UDP send —
/// lets a TCP-relay-only session (no EVRT UDP available) receive audio too.
/// `None` preserves the exact original UDP-only behavior. The callback
/// receives the same bytes the UDP path packetizes, before EVRT framing.
pub fn run_audio_capture(
    socket: Arc<UdpSocket>,
    peer_addr: SocketAddr,
    stop: Arc<AtomicBool>,
    session_token: Option<String>,
    events: std::sync::mpsc::Sender<crate::host::HostEvent>,
    on_tcp_frame: Option<Box<dyn Fn(&[u8]) + Send>>,
) {
    #[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
    {
        if let Err(e) = run_audio_capture_windows(
            socket,
            peer_addr,
            stop,
            session_token,
            &events,
            on_tcp_frame,
        ) {
            let msg = format!("EVRT Audio: захват завершился с ошибкой: {e}");
            eprintln!("[evrt-audio] {msg}");
            let _ = events.send(crate::host::HostEvent::Log(msg));
        }
    }
    #[cfg(not(all(target_os = "windows", feature = "evrt-wasapi")))]
    {
        let _ = (socket, peer_addr, session_token, events, on_tcp_frame);
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
fn run_audio_capture_windows(
    socket: Arc<UdpSocket>,
    peer_addr: SocketAddr,
    stop: Arc<AtomicBool>,
    session_token: Option<String>,
    events: &std::sync::mpsc::Sender<crate::host::HostEvent>,
    on_tcp_frame: Option<Box<dyn Fn(&[u8]) + Send>>,
) -> Result<(), String> {
    use windows::Win32::{
        Media::Audio::{
            eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
            MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
        },
    };

    // WAVE_FORMAT_* constants (winmm)
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

    fn audio_log(ev: &std::sync::mpsc::Sender<crate::host::HostEvent>, msg: String) {
        eprintln!("[evrt-audio] {msg}");
        let _ = ev.send(crate::host::HostEvent::Log(msg));
    }

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        // ── Получить устройство воспроизведения (loopback захватывает его вывод) ─
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("MMDeviceEnumerator: {:#010X}", e.code().0))?;

        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("GetDefaultAudioEndpoint: {:#010X}", e.code().0))?;

        let client: IAudioClient = device
            .Activate::<IAudioClient>(CLSCTX_ALL, None)
            .map_err(|e| format!("Activate IAudioClient: {:#010X}", e.code().0))?;

        // ── GetMixFormat: WASAPI shared mode ТРЕБУЕТ нативный формат устройства ─
        // Хардкоженный PCM 16-bit вызывает AUDCLNT_E_UNSUPPORTED_FORMAT.
        let mix_fmt_ptr = client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat: {:#010X}", e.code().0))?;
        let mix_fmt = &*mix_fmt_ptr;

        let channels = mix_fmt.nChannels;
        let sample_rate = mix_fmt.nSamplesPerSec;
        let bits = mix_fmt.wBitsPerSample;
        let tag = mix_fmt.wFormatTag;

        // Detect float32: tag==3 (IEEE_FLOAT) или EXTENSIBLE с SubFormat.Data1==3
        // WAVEFORMATEXTENSIBLE layout: WAVEFORMATEX (18 bytes) + Samples (2) + ChannelMask (4) + SubFormat GUID (16)
        // SubFormat.Data1 (u32) находится на байтовом смещении 24.
        let is_float32 = if tag == WAVE_FORMAT_IEEE_FLOAT {
            true
        } else if tag == WAVE_FORMAT_EXTENSIBLE {
            let sub_data1 = u32::from_le_bytes([
                *(mix_fmt_ptr as *const u8).add(24),
                *(mix_fmt_ptr as *const u8).add(25),
                *(mix_fmt_ptr as *const u8).add(26),
                *(mix_fmt_ptr as *const u8).add(27),
            ]);
            sub_data1 == 3 // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            false
        };

        audio_log(
            events,
            format!(
                "EVRT Audio: устройство {}Hz {}ch {}bit {} (tag=0x{:04X})",
                sample_rate,
                channels,
                bits,
                if is_float32 { "float32" } else { "PCM_i16" },
                tag,
            ),
        );

        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                100_000, // 10 мс буфер (единицы 100-нс)
                0,
                mix_fmt_ptr,
                None,
            )
            .map_err(|e| {
                CoTaskMemFree(Some(mix_fmt_ptr as *mut _));
                format!("IAudioClient::Initialize: {:#010X}", e.code().0)
            })?;

        CoTaskMemFree(Some(mix_fmt_ptr as *mut _));

        // Всегда отдаём клиенту 48000Hz — Android LOW_LATENCY поддерживает только 48кГц.
        // Если устройство на 44100Hz — ресемплируем на хосте.
        const TARGET_RATE: u32 = 48000;
        let out_channels: u16 = channels.min(2);
        let cfg = AudioConfig {
            sample_rate: TARGET_RATE,
            channels: out_channels,
            bits_per_sample: 16,
        };
        let cfg_pkt = evrt::build_single_authenticated(
            evrt::TYPE_AUDIO_CONFIG,
            &cfg.to_json(),
            session_token.as_deref(),
        );
        let _ = socket.send_to(&cfg_pkt, peer_addr);

        let capture_client: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("GetService IAudioCaptureClient: {:#010X}", e.code().0))?;

        client
            .Start()
            .map_err(|e| format!("IAudioClient::Start: {:#010X}", e.code().0))?;

        audio_log(
            events,
            format!(
                "EVRT Audio: WASAPI loopback запущен → {}Hz {}ch → клиент i16 stereo",
                sample_rate, channels,
            ),
        );

        const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x0000_0002;
        // Фиксированный выходной фрейм: 480 сэмплов × TARGET_RATE / 48000
        // = ровно 10 мс при TARGET_RATE=48000. Всегда 960 байт i16 stereo.
        let out_frame_samples: usize = TARGET_RATE as usize * 10 / 1000; // 480

        let ch = channels as usize;
        let out_ch = out_channels as usize;
        let need_resample = sample_rate != TARGET_RATE;

        // Буфер накапливает i16 stereo сэмплы после конвертации из нативного формата.
        // Защитный cap: 48000 сэмплов × 2 канала = 1 секунда при 48kHz.
        // Если за 1 сек не дренируется — обрезаем хвост, предотвращая неограниченный рост.
        const RAW_BUF_CAP_FRAMES: usize = 48000;
        let mut raw_i16_buf: Vec<i16> = Vec::new(); // i16 стерео сэмплы (пары L,R)
        let out_frame_bytes = out_frame_samples * out_ch * 2; // 480 * 2 * 2 = 1920 байт

        let mut frame_id: u32 = 0;
        // Дробная позиция для линейной интерполяции ресемплинга
        let mut resample_pos: f64 = 0.0;
        let resample_step: f64 = sample_rate as f64 / TARGET_RATE as f64;

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

            let n = num_frames_available as usize;

            // Конвертируем нативный формат → i16 stereo (без ресемплинга пока)
            if flags & AUDCLNT_BUFFERFLAGS_SILENT != 0 {
                // WASAPI пометил буфер как тишина — добавляем нули
                for _ in 0..n {
                    for _ in 0..out_ch {
                        raw_i16_buf.push(0);
                    }
                }
            } else if is_float32 {
                let floats = std::slice::from_raw_parts(data_ptr as *const f32, n * ch);
                for i in 0..n {
                    for c in 0..out_ch {
                        let s = floats[i * ch + c.min(ch - 1)];
                        raw_i16_buf.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
                    }
                }
            } else if ch == out_ch {
                let samples = std::slice::from_raw_parts(data_ptr as *const i16, n * ch);
                raw_i16_buf.extend_from_slice(samples);
            } else {
                let samples = std::slice::from_raw_parts(data_ptr as *const i16, n * ch);
                for i in 0..n {
                    for c in 0..out_ch {
                        raw_i16_buf.push(samples[i * ch + c.min(ch - 1)]);
                    }
                }
            }

            capture_client.ReleaseBuffer(num_frames_available).ok();

            // Защита от накопления: если буфер превысил 1 сек — обрезаем,
            // иначе после длительного простоя (Sleep/Suspend) отправим burst.
            let cap = RAW_BUF_CAP_FRAMES * out_ch;
            if raw_i16_buf.len() > cap {
                let excess = raw_i16_buf.len() - cap;
                raw_i16_buf.drain(..excess);
                if need_resample {
                    resample_pos = resample_pos.max(0.0) - excess as f64 / out_ch as f64;
                    resample_pos = resample_pos.max(0.0);
                }
            }

            // Ресемплинг + нарезка на фиксированные фреймы 480 × out_ch
            // raw_i16_buf хранит стерео-пары [(L0,R0),(L1,R1),...] из input-rate
            let in_frames = raw_i16_buf.len() / out_ch;

            let mut pcm_out: Vec<u8> = Vec::new();
            while need_resample {
                let idx = resample_pos as usize;
                if idx + 1 >= in_frames {
                    break;
                }
                let frac = resample_pos - idx as f64;
                for c in 0..out_ch {
                    let s0 = raw_i16_buf[idx * out_ch + c] as f64;
                    let s1 = raw_i16_buf[(idx + 1) * out_ch + c] as f64;
                    let s = (s0 + (s1 - s0) * frac) as i16;
                    pcm_out.extend_from_slice(&s.to_le_bytes());
                }
                resample_pos += resample_step;
            }
            if !need_resample {
                for s in &raw_i16_buf {
                    pcm_out.extend_from_slice(&s.to_le_bytes());
                }
            }

            // Сдвигаем позицию ресемплинга чтобы не выходить за пределы использованных сэмплов
            let consumed_in_frames = if need_resample {
                resample_pos as usize
            } else {
                in_frames
            };
            let consumed = consumed_in_frames.min(in_frames);
            raw_i16_buf.drain(..consumed * out_ch);
            if need_resample {
                resample_pos -= consumed as f64;
            }

            // Нарезаем pcm_out на фиксированные фреймы и отправляем
            let mut pos = 0;
            while pos + out_frame_bytes <= pcm_out.len() {
                let chunk = &pcm_out[pos..pos + out_frame_bytes];
                frame_id = frame_id.wrapping_add(1);
                let pts_us = pts_hns / 10;
                let pkts = evrt::packetize_audio_frame_authenticated(
                    frame_id,
                    pts_us,
                    chunk,
                    session_token.as_deref(),
                );
                for pkt in &pkts {
                    let _ = socket.send_to(pkt, peer_addr);
                }
                if let Some(cb) = &on_tcp_frame {
                    cb(chunk);
                }
                pos += out_frame_bytes;
            }
        }

        client.Stop().ok();
        audio_log(events, "EVRT Audio: WASAPI loopback остановлен".into());
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
    #[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
    inner: Option<WasapiPlayer>,
    #[cfg(not(all(target_os = "windows", feature = "evrt-wasapi")))]
    _unused: (),
}

impl AudioPlayer {
    pub fn new() -> Self {
        Self {
            cfg: None,
            queue: VecDeque::new(),
            front_offset: 0,
            buffering: true,
            #[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
            inner: None,
            #[cfg(not(all(target_os = "windows", feature = "evrt-wasapi")))]
            _unused: (),
        }
    }

    /// Инициализировать с заданным форматом.
    pub fn init(&mut self, cfg: &AudioConfig) {
        #[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
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
        #[cfg(not(all(target_os = "windows", feature = "evrt-wasapi")))]
        {
            let _ = cfg;
            eprintln!("[evrt-audio] аудио воспроизведение не поддерживается на этой платформе");
        }
    }

    /// Воспроизвести PCM-фрейм.
    pub fn play(&mut self, pcm: &[u8]) {
        #[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
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
        #[cfg(target_os = "android")]
        if !pcm.is_empty() {
            push_android_audio(pcm.to_vec());
        }
        #[cfg(not(any(
            all(target_os = "windows", feature = "evrt-wasapi"),
            target_os = "android"
        )))]
        let _ = pcm;
    }

    pub fn tick(&mut self) {
        #[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
        self.pump();
    }

    /// Drop queued PCM immediately when a viewer session is muted.
    pub fn clear_buffer(&mut self) {
        self.queue.clear();
        self.front_offset = 0;
        self.buffering = true;
    }

    #[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
    fn enqueue_pcm(&mut self, pcm: &[u8]) {
        self.queue.push_back(pcm.to_vec());
        while self.queued_ms() > AUDIO_MAX_BUFFER_MS && self.queue.len() > 1 {
            self.queue.pop_front();
            self.front_offset = 0;
            self.buffering = false;
        }
    }

    #[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
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

#[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
struct WasapiPlayer {
    client: windows::Win32::Media::Audio::IAudioClient,
    render_client: windows::Win32::Media::Audio::IAudioRenderClient,
    block_align: usize,
    buffer_frames: u32,
}

#[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
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
                    .map_err(|e| format!("MMDeviceEnumerator: {:#010X}", e.code().0))?;

            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("GetDefaultAudioEndpoint: {:#010X}", e.code().0))?;

            let client: IAudioClient = device
                .Activate::<IAudioClient>(CLSCTX_ALL, None)
                .map_err(|e| format!("Activate: {:#010X}", e.code().0))?;

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
                .map_err(|e| format!("Initialize: {:#010X}", e.code().0))?;

            let buffer_frames = client
                .GetBufferSize()
                .map_err(|e| format!("GetBufferSize: {:#010X}", e.code().0))?;

            let render_client: IAudioRenderClient = client
                .GetService()
                .map_err(|e| format!("GetService IAudioRenderClient: {:#010X}", e.code().0))?;

            client
                .Start()
                .map_err(|e| format!("Start: {:#010X}", e.code().0))?;

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
                .map_err(|e| format!("GetCurrentPadding: {:#010X}", e.code().0))?;

            let available = self.buffer_frames.saturating_sub(padding);
            let frames_in_data = (pcm.len() / self.block_align) as u32;
            let frames_to_write = frames_in_data.min(available);

            if frames_to_write == 0 {
                return Ok(0);
            }

            let buf_ptr = self
                .render_client
                .GetBuffer(frames_to_write)
                .map_err(|e| format!("GetBuffer: {:#010X}", e.code().0))?;

            let write_bytes = frames_to_write as usize * self.block_align;
            std::ptr::copy_nonoverlapping(pcm.as_ptr(), buf_ptr, write_bytes.min(pcm.len()));

            self.render_client
                .ReleaseBuffer(frames_to_write, 0)
                .map_err(|e| format!("ReleaseBuffer: {:#010X}", e.code().0))?;

            Ok(write_bytes)
        }
    }
}

#[cfg(all(target_os = "windows", feature = "evrt-wasapi"))]
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

const MAX_AUDIO_PACKET_COUNT: u16 = 8;

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
        if pkt.packet_type != evrt::TYPE_AUDIO_FRAME
            || pkt.packet_count == 0
            || pkt.packet_count > MAX_AUDIO_PACKET_COUNT
            || pkt.packet_index >= pkt.packet_count
            || pkt.payload.len() > evrt::MAX_PAYLOAD_SIZE
        {
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
            self.frames.remove(&pkt.frame_id);
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
    fn audio_player_mute_clears_buffered_pcm() {
        let mut player = AudioPlayer::new();
        player.cfg = Some(AudioConfig::default());
        player.queue.push_back(vec![1; 1_920]);
        player.front_offset = 100;
        player.buffering = false;

        player.clear_buffer();

        assert_eq!(player.queued_bytes(), 0);
        assert_eq!(player.front_offset, 0);
        assert!(player.buffering);
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
    fn audio_reassembler_rejects_non_audio_packet() {
        let mut re = AudioReassembler::new();
        let pkt = evrt::EvrtPacket {
            packet_type: evrt::TYPE_VIDEO_FRAME,
            flags: 0,
            frame_id: 1,
            packet_index: 0,
            packet_count: 1,
            presentation_time_us: 0,
            payload: vec![1],
        };

        assert!(re.on_packet(&pkt).is_none());
    }

    #[test]
    fn audio_reassembler_rejects_excessive_packet_count() {
        let mut re = AudioReassembler::new();
        let pkt = evrt::EvrtPacket {
            packet_type: evrt::TYPE_AUDIO_FRAME,
            flags: 0,
            frame_id: 1,
            packet_index: 0,
            packet_count: MAX_AUDIO_PACKET_COUNT + 1,
            presentation_time_us: 0,
            payload: vec![1],
        };

        assert!(re.on_packet(&pkt).is_none());
    }

    #[test]
    fn audio_reassembler_drops_conflicting_packet_count() {
        let mut re = AudioReassembler::new();
        let first = evrt::EvrtPacket {
            packet_type: evrt::TYPE_AUDIO_FRAME,
            flags: 0,
            frame_id: 1,
            packet_index: 0,
            packet_count: 2,
            presentation_time_us: 0,
            payload: vec![1],
        };
        let conflict = evrt::EvrtPacket {
            packet_type: evrt::TYPE_AUDIO_FRAME,
            flags: 0,
            frame_id: 1,
            packet_index: 1,
            packet_count: 3,
            presentation_time_us: 0,
            payload: vec![2],
        };
        let valid = evrt::EvrtPacket {
            packet_type: evrt::TYPE_AUDIO_FRAME,
            flags: 0,
            frame_id: 1,
            packet_index: 0,
            packet_count: 1,
            presentation_time_us: 0,
            payload: vec![3],
        };

        assert!(re.on_packet(&first).is_none());
        assert!(re.on_packet(&conflict).is_none());
        assert_eq!(re.on_packet(&valid), Some(vec![3]));
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
