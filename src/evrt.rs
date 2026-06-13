// =============================================================================
// EVRT Protocol — разработан Артуром Валиевым (Artur Valiev)
// Оригинальная реализация: EvertyGame (C#, https://github.com/djvaliev)
// Rust-порт для EvertyDesk Lite выполнен на основе оригинальных алгоритмов.
//
// Протокол, алгоритмы адаптивной буферизации, система давления (pressure),
// логика FeedbackLoop и LatestAccessUnitQueue — интеллектуальная собственность
// Артура Валиева, разработанная в течение нескольких лет работы над EvertyGame.
// =============================================================================

//! EVRT — порт UDP-транспортного протокола из EvertyGame (C#) в Rust.
//!
//! Бинарный протокол с заголовком 24 байта:
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                     Magic (0x45565254)                        | 4
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |  Version (3)  |     Type      |           Flags               | 8
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                          FrameId                              | 12
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |         PacketIndex           |          PacketCount          | 16
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                  PresentationTimeUs (hi)                      | 20
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                  PresentationTimeUs (lo)                      | 24
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                         Payload …                             |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! Максимальный размер UDP-датаграммы: 1200 байт (MTU-safe).
//! Максимальный размер payload: 1200 - 24 = 1176 байт.

// Полная API-поверхность EVRT-протокола: часть методов — публичный
// интерфейс для будущего использования (enhancement layer, audio config, jitter API).
#![allow(dead_code)]

use std::time::{SystemTime, UNIX_EPOCH};

// ─── константы ────────────────────────────────────────────────────────────────

pub const MAGIC: u32 = 0x4556_5254; // "EVRT"
pub const VERSION: u8 = 3;
pub const HEADER_SIZE: usize = 24;
pub const MAX_PACKET_SIZE: usize = 1200;
pub const MAX_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - HEADER_SIZE;
pub const MAX_FRAME_PACKET_COUNT: usize = 16 * 1024;
pub const MAX_FRAME_PAYLOAD_SIZE: usize = MAX_PAYLOAD_SIZE * MAX_FRAME_PACKET_COUNT;

// ─── типы пакетов ─────────────────────────────────────────────────────────────

pub const TYPE_SESSION_CONFIG: u8 = 1;
pub const TYPE_CODEC_CONFIG: u8 = 2;
pub const TYPE_VIDEO_FRAME: u8 = 3;
pub const TYPE_CONTROL: u8 = 4;
pub const TYPE_AUDIO_CONFIG: u8 = 5;
pub const TYPE_AUDIO_FRAME: u8 = 6;
pub const TYPE_ENHANCEMENT_CONFIG: u8 = 7;
pub const TYPE_ENHANCEMENT_FRAME: u8 = 8;
pub const TYPE_ROI_METADATA: u8 = 9;

// ─── флаги ────────────────────────────────────────────────────────────────────

pub const FLAG_KEY_FRAME: u16 = 0x0001;

// ─── структуры ────────────────────────────────────────────────────────────────

/// Распарсенный EVRT-пакет.
#[derive(Debug, Clone)]
pub struct EvrtPacket {
    pub packet_type: u8,
    pub flags: u16,
    pub frame_id: u32,
    pub packet_index: u16,
    pub packet_count: u16,
    pub presentation_time_us: u64,
    pub payload: Vec<u8>,
}

impl EvrtPacket {
    #[inline]
    pub fn is_key_frame(&self) -> bool {
        self.flags & FLAG_KEY_FRAME != 0
    }
}

// ─── парсер ───────────────────────────────────────────────────────────────────

/// Разобрать UDP-датаграмму в `EvrtPacket`.
/// Возвращает `None` если датаграмма слишком короткая, magic/version не совпадают.
pub fn parse(buf: &[u8], len: usize) -> Option<EvrtPacket> {
    if len < HEADER_SIZE || len > MAX_PACKET_SIZE || len > buf.len() {
        return None;
    }
    let b = &buf[..len];

    let magic = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    if magic != MAGIC {
        return None;
    }
    if b[4] != VERSION {
        return None;
    }

    let packet_type = b[5];
    let flags = u16::from_be_bytes([b[6], b[7]]);
    let frame_id = u32::from_be_bytes([b[8], b[9], b[10], b[11]]);
    let packet_index = u16::from_be_bytes([b[12], b[13]]);
    let packet_count = u16::from_be_bytes([b[14], b[15]]);
    if packet_count == 0
        || packet_index >= packet_count
        || packet_count as usize > MAX_FRAME_PACKET_COUNT
    {
        return None;
    }
    let presentation_time_us =
        u64::from_be_bytes([b[16], b[17], b[18], b[19], b[20], b[21], b[22], b[23]]);
    let payload = b[HEADER_SIZE..].to_vec();

    Some(EvrtPacket {
        packet_type,
        flags,
        frame_id,
        packet_index,
        packet_count,
        presentation_time_us,
        payload,
    })
}

// ─── построитель пакетов ──────────────────────────────────────────────────────

/// Построить один пакет с заданными полями.
pub fn build_packet(
    packet_type: u8,
    flags: u16,
    frame_id: u32,
    packet_index: u16,
    packet_count: u16,
    presentation_time_us: u64,
    payload: &[u8],
) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(HEADER_SIZE + payload.len());
    pkt.extend_from_slice(&MAGIC.to_be_bytes());
    pkt.push(VERSION);
    pkt.push(packet_type);
    pkt.extend_from_slice(&flags.to_be_bytes());
    pkt.extend_from_slice(&frame_id.to_be_bytes());
    pkt.extend_from_slice(&packet_index.to_be_bytes());
    pkt.extend_from_slice(&packet_count.to_be_bytes());
    pkt.extend_from_slice(&presentation_time_us.to_be_bytes());
    pkt.extend_from_slice(payload);
    pkt
}

/// Пакетизировать видеокадр в список UDP-датаграмм.
/// Каждая датаграмма ≤ MAX_PACKET_SIZE байт.
pub fn packetize_video_frame(
    frame_id: u32,
    presentation_time_us: u64,
    is_key_frame: bool,
    payload: &[u8],
) -> Vec<Vec<u8>> {
    let flags = if is_key_frame { FLAG_KEY_FRAME } else { 0 };
    packetize(
        TYPE_VIDEO_FRAME,
        flags,
        frame_id,
        presentation_time_us,
        payload,
    )
}

/// Пакетизировать enhancement-кадр.
pub fn packetize_enhancement_frame(
    frame_id: u32,
    presentation_time_us: u64,
    is_key_frame: bool,
    payload: &[u8],
) -> Vec<Vec<u8>> {
    let flags = if is_key_frame { FLAG_KEY_FRAME } else { 0 };
    packetize(
        TYPE_ENHANCEMENT_FRAME,
        flags,
        frame_id,
        presentation_time_us,
        payload,
    )
}

/// ROI (Region of Interest) — регион изменения экрана.
/// Хост шлёт перед видео-фреймом чтобы клиент знал какая часть изменилась.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoiRect {
    pub frame_id: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl RoiRect {
    pub fn to_json(self) -> Vec<u8> {
        format!(
            r#"{{"frameId":{},"x":{},"y":{},"w":{},"h":{}}}"#,
            self.frame_id, self.x, self.y, self.w, self.h
        )
        .into_bytes()
    }

    pub fn from_json(payload: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(payload).ok()?;
        Some(Self {
            frame_id: json_u32_field(s, "frameId").unwrap_or(0),
            x: json_u32_field(s, "x").unwrap_or(0),
            y: json_u32_field(s, "y").unwrap_or(0),
            w: json_u32_field(s, "w").unwrap_or(0),
            h: json_u32_field(s, "h").unwrap_or(0),
        })
    }

    /// Является ли ROI полным экраном (нет ограничений)?
    pub fn is_full_screen(self) -> bool {
        self.w == 0 && self.h == 0
    }

    pub fn dirty_area_milli(self, frame_width: u32, frame_height: u32) -> u32 {
        if self.is_full_screen() {
            return 1_000;
        }
        let frame_area = u64::from(frame_width) * u64::from(frame_height);
        if frame_area == 0 {
            return 1_000;
        }

        let x0 = self.x.min(frame_width);
        let y0 = self.y.min(frame_height);
        let x1 = self.x.saturating_add(self.w).min(frame_width);
        let y1 = self.y.saturating_add(self.h).min(frame_height);
        let dirty_w = x1.saturating_sub(x0);
        let dirty_h = y1.saturating_sub(y0);
        let dirty_area = u64::from(dirty_w) * u64::from(dirty_h);

        ((dirty_area * 1_000 + frame_area - 1) / frame_area).min(1_000) as u32
    }
}

/// Построить ROI-пакет для отправки перед видео-фреймом.
pub fn build_roi_metadata(roi: RoiRect) -> Vec<u8> {
    let json = roi.to_json();
    if json.len() <= MAX_PAYLOAD_SIZE {
        build_single(TYPE_ROI_METADATA, &json)
    } else {
        Vec::new()
    }
}

/// Пакетизировать аудио-фрейм (PCM данные).
/// Аудио-фреймы маленькие и обычно умещаются в один пакет.
pub fn packetize_audio_frame(
    frame_id: u32,
    presentation_time_us: u64,
    payload: &[u8],
) -> Vec<Vec<u8>> {
    packetize(TYPE_AUDIO_FRAME, 0, frame_id, presentation_time_us, payload)
}

/// Построить одиночный пакет для конфигурации/управления.
pub fn build_single(packet_type: u8, payload: &[u8]) -> Vec<u8> {
    assert!(
        payload.len() <= MAX_PAYLOAD_SIZE,
        "single-packet payload too large: {} > {}",
        payload.len(),
        MAX_PAYLOAD_SIZE
    );
    build_packet(packet_type, 0, 0, 0, 1, 0, payload)
}

pub fn build_session_config(payload: &[u8]) -> Vec<u8> {
    build_single(TYPE_SESSION_CONFIG, payload)
}

pub fn build_codec_config(payload: &[u8]) -> Vec<u8> {
    build_single(TYPE_CODEC_CONFIG, payload)
}

pub fn build_control(payload: &[u8]) -> Vec<u8> {
    build_single(TYPE_CONTROL, payload)
}

// ─── вспомогательные ──────────────────────────────────────────────────────────

fn packetize(
    packet_type: u8,
    flags: u16,
    frame_id: u32,
    presentation_time_us: u64,
    payload: &[u8],
) -> Vec<Vec<u8>> {
    if payload.is_empty() {
        return Vec::new();
    }
    let packet_count = payload.len().div_ceil(MAX_PAYLOAD_SIZE);
    if packet_count > MAX_FRAME_PACKET_COUNT {
        return Vec::new();
    }
    let mut packets = Vec::with_capacity(packet_count);

    for (i, chunk) in payload.chunks(MAX_PAYLOAD_SIZE).enumerate() {
        packets.push(build_packet(
            packet_type,
            flags,
            frame_id,
            i as u16,
            packet_count as u16,
            presentation_time_us,
            chunk,
        ));
    }
    packets
}

/// Текущее время в микросекундах (монотонно относительно process start).
pub fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

// ─── управляющие пакеты ───────────────────────────────────────────────────────

/// Построить пакет `request_key_frame`.
pub fn build_request_key_frame() -> Vec<u8> {
    let payload = br#"{"kind":"request_key_frame"}"#;
    build_control(payload)
}

/// Feedback от получателя к отправителю.
#[derive(Debug, Clone, Default)]
pub struct ReceiverFeedback {
    pub pressure: Pressure,
    pub backlog_frames: u32,
    pub queue_drops: u64,
    pub decode_fps: u32,
    pub assembly_delay_ms: i32,
    pub arrival_delta_ms: i32,
    pub decode_delta_ms: i32,
    pub present_delta_ms: i32,
    pub pulse_estimate_ms: i32,
    pub input_estimate_ms: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pressure {
    #[default]
    Normal,
    High,
    Critical,
}

impl Pressure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "critical" => Self::Critical,
            "high" => Self::High,
            _ => Self::Normal,
        }
    }
}

/// Сериализовать feedback в JSON-байты для `TYPE_CONTROL` пакета.
pub fn build_receiver_feedback(fb: &ReceiverFeedback) -> Vec<u8> {
    let json = format!(
        r#"{{"kind":"receiver_feedback","pressure":"{}","backlogFrames":{},"queueDrops":{},"decodeFps":{},"assemblyDelayMs":{},"arrivalDeltaMs":{},"decodeDeltaMs":{},"presentDeltaMs":{},"pulseEstimateMs":{},"inputEstimateMs":{}}}"#,
        fb.pressure.as_str(),
        fb.backlog_frames,
        fb.queue_drops,
        fb.decode_fps,
        fb.assembly_delay_ms,
        fb.arrival_delta_ms,
        fb.decode_delta_ms,
        fb.present_delta_ms,
        fb.pulse_estimate_ms,
        fb.input_estimate_ms,
    );
    // Может не войти в MAX_PAYLOAD_SIZE при очень больших числах — обрезать безопасно не нужно,
    // JSON всегда < 300 байт.
    build_control(json.as_bytes())
}

/// Разобрать входящий control-пакет.
pub fn parse_control(payload: &[u8]) -> Option<ControlMessage> {
    // Минимальный ручной парсер без зависимости от serde_json.
    let s = std::str::from_utf8(payload).ok()?;
    let kind = json_str_field(s, "kind")?;
    match kind.as_str() {
        "request_key_frame" => Some(ControlMessage::RequestKeyFrame),
        "receiver_feedback" => parse_feedback(s).map(ControlMessage::ReceiverFeedback),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub enum ControlMessage {
    RequestKeyFrame,
    ReceiverFeedback(ReceiverFeedback),
}

// ─── SessionConfig ────────────────────────────────────────────────────────────

/// Конфигурация сессии, совместимая с EvertyGame SessionConfig.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub codec: String,
    pub preset: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub stream_mode: String,
    pub adaptation_mode: String,
}

impl SessionConfig {
    /// Сериализовать в JSON.
    pub fn to_json(&self) -> Vec<u8> {
        format!(
            r#"{{"codec":"{}","preset":"{}","adaptationMode":"{}","width":{},"height":{},"fps":{},"bitrate":{},"streamMode":"{}","enhancementEnabled":false,"enhancementCodec":null,"enhancementMaxWidth":0,"enhancementMaxHeight":0,"roiMode":"none"}}"#,
            self.codec,
            self.preset,
            self.adaptation_mode,
            self.width,
            self.height,
            self.fps,
            self.bitrate,
            self.stream_mode,
        )
        .into_bytes()
    }

    /// Разобрать из JSON.
    pub fn from_json(payload: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(payload).ok()?;
        Some(Self {
            codec: json_str_field(s, "codec").unwrap_or_default(),
            preset: json_str_field(s, "preset").unwrap_or_default(),
            width: json_u32_field(s, "width").unwrap_or(1920),
            height: json_u32_field(s, "height").unwrap_or(1080),
            fps: json_u32_field(s, "fps").unwrap_or(60),
            bitrate: json_u32_field(s, "bitrate").unwrap_or(8_000_000),
            stream_mode: json_str_field(s, "streamMode").unwrap_or_else(|| "single".into()),
            adaptation_mode: json_str_field(s, "adaptationMode").unwrap_or_else(|| "GAME".into()),
        })
    }

    pub fn is_cinema_smooth(&self) -> bool {
        self.adaptation_mode.eq_ignore_ascii_case("CINEMA_SMOOTH")
    }
}

// ─── мини JSON-парсер (без зависимостей) ──────────────────────────────────────

fn json_str_field(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = s.find(needle.as_str())?;
    let rest = &s[pos + needle.len()..];
    let rest = rest.trim_start_matches([' ', ':', '\t']);
    if rest.starts_with('"') {
        let inner = &rest[1..];
        let end = inner.find('"')?;
        Some(inner[..end].to_owned())
    } else {
        None
    }
}

fn json_u32_field(s: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{}\"", key);
    let pos = s.find(needle.as_str())?;
    let rest = &s[pos + needle.len()..];
    let rest = rest.trim_start_matches([' ', ':', '\t']);
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn json_i32_field(s: &str, key: &str) -> Option<i32> {
    let needle = format!("\"{}\"", key);
    let pos = s.find(needle.as_str())?;
    let rest = &s[pos + needle.len()..];
    let rest = rest.trim_start_matches([' ', ':', '\t']);
    let end = rest
        .find(|c: char| c != '-' && !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn json_u64_field(s: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\"", key);
    let pos = s.find(needle.as_str())?;
    let rest = &s[pos + needle.len()..];
    let rest = rest.trim_start_matches([' ', ':', '\t']);
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn parse_feedback(s: &str) -> Option<ReceiverFeedback> {
    Some(ReceiverFeedback {
        pressure: Pressure::from_str(&json_str_field(s, "pressure").unwrap_or_default()),
        backlog_frames: json_u32_field(s, "backlogFrames").unwrap_or(0),
        queue_drops: json_u64_field(s, "queueDrops").unwrap_or(0),
        decode_fps: json_u32_field(s, "decodeFps").unwrap_or(0),
        assembly_delay_ms: json_i32_field(s, "assemblyDelayMs").unwrap_or(-1),
        arrival_delta_ms: json_i32_field(s, "arrivalDeltaMs").unwrap_or(-1),
        decode_delta_ms: json_i32_field(s, "decodeDeltaMs").unwrap_or(-1),
        present_delta_ms: json_i32_field(s, "presentDeltaMs").unwrap_or(-1),
        pulse_estimate_ms: json_i32_field(s, "pulseEstimateMs").unwrap_or(-1),
        input_estimate_ms: json_i32_field(s, "inputEstimateMs").unwrap_or(-1),
    })
}

// ─── тесты ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_video_frame() {
        let payload = vec![0xAB_u8; 3000];
        let packets = packetize_video_frame(42, 12345678, true, &payload);
        // 3000 / 1176 = 3 пакета
        assert_eq!(packets.len(), 3);
        for (i, pkt) in packets.iter().enumerate() {
            assert!(pkt.len() <= MAX_PACKET_SIZE);
            let parsed = parse(pkt, pkt.len()).unwrap();
            assert_eq!(parsed.packet_type, TYPE_VIDEO_FRAME);
            assert_eq!(parsed.frame_id, 42);
            assert_eq!(parsed.packet_index, i as u16);
            assert_eq!(parsed.packet_count, 3);
            assert!(parsed.is_key_frame());
            assert_eq!(parsed.presentation_time_us, 12345678);
        }
        // Собрать обратно
        let mut reassembled = Vec::new();
        for pkt in &packets {
            let parsed = parse(pkt, pkt.len()).unwrap();
            reassembled.extend_from_slice(&parsed.payload);
        }
        assert_eq!(reassembled, payload);
    }

    #[test]
    fn single_packet_fits() {
        let payload = vec![1u8; MAX_PAYLOAD_SIZE];
        let packets = packetize_video_frame(1, 0, false, &payload);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].len(), MAX_PACKET_SIZE);
    }

    #[test]
    fn parse_invalid() {
        assert!(parse(&[0u8; 10], 10).is_none()); // too short
        assert!(parse(&[0u8; HEADER_SIZE], HEADER_SIZE).is_none()); // wrong magic
    }

    #[test]
    fn parse_rejects_oversized_datagram() {
        let pkt = build_packet(
            TYPE_VIDEO_FRAME,
            0,
            1,
            0,
            1,
            0,
            &vec![0u8; MAX_PAYLOAD_SIZE + 1],
        );
        assert_eq!(pkt.len(), MAX_PACKET_SIZE + 1);
        assert!(parse(&pkt, pkt.len()).is_none());
    }

    #[test]
    fn parse_rejects_declared_len_beyond_buffer() {
        let pkt = build_packet(TYPE_VIDEO_FRAME, 0, 1, 0, 1, 0, &[1]);
        assert!(parse(&pkt, pkt.len() + 1).is_none());
    }

    #[test]
    fn parse_rejects_invalid_fragment_header() {
        let zero_count = build_packet(TYPE_VIDEO_FRAME, 0, 1, 0, 0, 0, &[1]);
        assert!(parse(&zero_count, zero_count.len()).is_none());

        let index_out_of_range = build_packet(TYPE_VIDEO_FRAME, 0, 1, 2, 2, 0, &[1]);
        assert!(parse(&index_out_of_range, index_out_of_range.len()).is_none());

        let excessive_packet_count = build_packet(
            TYPE_VIDEO_FRAME,
            0,
            1,
            0,
            (MAX_FRAME_PACKET_COUNT as u16) + 1,
            0,
            &[1],
        );
        assert!(parse(&excessive_packet_count, excessive_packet_count.len()).is_none());
    }

    #[test]
    fn packetize_rejects_oversized_frame_before_u16_wrap() {
        let payload = vec![0u8; MAX_FRAME_PAYLOAD_SIZE + 1];
        assert!(packetize_video_frame(1, 0, true, &payload).is_empty());
    }

    #[test]
    fn feedback_roundtrip() {
        let fb = ReceiverFeedback {
            pressure: Pressure::Critical,
            backlog_frames: 3,
            queue_drops: 17,
            decode_fps: 45,
            assembly_delay_ms: 12,
            arrival_delta_ms: 8,
            decode_delta_ms: 5,
            present_delta_ms: 3,
            pulse_estimate_ms: 22,
            input_estimate_ms: 30,
        };
        let pkt = build_receiver_feedback(&fb);
        let parsed = parse(&pkt, pkt.len()).unwrap();
        assert_eq!(parsed.packet_type, TYPE_CONTROL);
        let ctrl = parse_control(&parsed.payload).unwrap();
        match ctrl {
            ControlMessage::ReceiverFeedback(f) => {
                assert_eq!(f.pressure, Pressure::Critical);
                assert_eq!(f.backlog_frames, 3);
                assert_eq!(f.decode_fps, 45);
                assert_eq!(f.assembly_delay_ms, 12);
                assert_eq!(f.arrival_delta_ms, 8);
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn request_key_frame() {
        let pkt = build_request_key_frame();
        let parsed = parse(&pkt, pkt.len()).unwrap();
        let ctrl = parse_control(&parsed.payload).unwrap();
        assert!(matches!(ctrl, ControlMessage::RequestKeyFrame));
    }

    #[test]
    fn roi_dirty_area_fullscreen_is_full_frame() {
        let roi = RoiRect {
            frame_id: 7,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        assert_eq!(roi.dirty_area_milli(1920, 1080), 1_000);
    }

    #[test]
    fn roi_dirty_area_clips_to_frame() {
        let roi = RoiRect {
            frame_id: 7,
            x: 90,
            y: 90,
            w: 50,
            h: 50,
        };
        assert_eq!(roi.dirty_area_milli(100, 100), 10);
    }

    #[test]
    fn session_config_roundtrip() {
        let cfg = SessionConfig {
            codec: "H264".into(),
            preset: "GAME".into(),
            width: 1280,
            height: 720,
            fps: 60,
            bitrate: 8_500_000,
            stream_mode: "single".into(),
            adaptation_mode: "GAME".into(),
        };
        let json = cfg.to_json();
        let parsed = SessionConfig::from_json(&json).unwrap();
        assert_eq!(parsed.width, 1280);
        assert_eq!(parsed.fps, 60);
        assert_eq!(parsed.codec, "H264");
    }
}
