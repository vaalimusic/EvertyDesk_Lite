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

use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

// ─── константы ────────────────────────────────────────────────────────────────

pub const MAGIC: u32 = 0x4556_5254; // "EVRT"
pub const VERSION: u8 = 3;
pub const HEADER_SIZE: usize = 24;
pub const MAX_PACKET_SIZE: usize = 1200;
pub const MAX_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - HEADER_SIZE;
pub const AUTH_TAG_SIZE: usize = 16;
pub const MAX_AUTH_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - HEADER_SIZE - AUTH_TAG_SIZE;
pub const MAX_FRAME_PACKET_COUNT: usize = 16 * 1024;
pub const MAX_FRAME_PAYLOAD_SIZE: usize = MAX_PAYLOAD_SIZE * MAX_FRAME_PACKET_COUNT;
pub const MAX_AUTH_FRAME_PAYLOAD_SIZE: usize = MAX_AUTH_PAYLOAD_SIZE * MAX_FRAME_PACKET_COUNT;

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
pub const TYPE_FEC: u8 = 10;

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

pub fn parse_authenticated(buf: &[u8], len: usize, session_token: Option<&str>) -> Option<EvrtPacket> {
    let Some(token) = session_token.filter(|token| valid_session_token(token)) else {
        return parse(buf, len);
    };
    if len < HEADER_SIZE + AUTH_TAG_SIZE || len > MAX_PACKET_SIZE || len > buf.len() {
        return None;
    }
    let body_len = len.checked_sub(AUTH_TAG_SIZE)?;
    let body = &buf[..body_len];
    let tag = &buf[body_len..len];
    if !auth_tag_matches(token, body, tag) {
        return None;
    }
    parse(body, body_len)
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

pub fn build_packet_authenticated(
    packet_type: u8,
    flags: u16,
    frame_id: u32,
    packet_index: u16,
    packet_count: u16,
    presentation_time_us: u64,
    payload: &[u8],
    session_token: Option<&str>,
) -> Vec<u8> {
    let Some(token) = session_token.filter(|token| valid_session_token(token)) else {
        return build_packet(
            packet_type,
            flags,
            frame_id,
            packet_index,
            packet_count,
            presentation_time_us,
            payload,
        );
    };
    if payload.len() > MAX_AUTH_PAYLOAD_SIZE {
        return Vec::new();
    }
    let mut pkt = build_packet(
        packet_type,
        flags,
        frame_id,
        packet_index,
        packet_count,
        presentation_time_us,
        payload,
    );
    let tag = auth_tag(token, &pkt);
    pkt.extend_from_slice(&tag);
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

pub fn packetize_video_frame_authenticated(
    frame_id: u32,
    presentation_time_us: u64,
    is_key_frame: bool,
    payload: &[u8],
    session_token: Option<&str>,
) -> Vec<Vec<u8>> {
    let flags = if is_key_frame { FLAG_KEY_FRAME } else { 0 };
    packetize_authenticated(
        TYPE_VIDEO_FRAME,
        flags,
        frame_id,
        presentation_time_us,
        payload,
        session_token,
    )
}

/// Пакетизировать видеокадр СОГЛАСОВАННО с FEC: возвращает (data_пакеты,
/// fec_пакеты). Чанки бьются с запасом под FEC-заголовок, чтобы parity-пакет
/// (header + XOR(чанк)) гарантированно влезал в одну MTU-датаграмму.
///
/// FEC строится только для многопакетных кадров (одиночный XOR-ом не защитить).
/// Для key-frame'ов FEC особенно ценен: их потеря вызывает долгий фриз до IDR.
pub fn packetize_video_with_fec(
    frame_id: u32,
    presentation_time_us: u64,
    is_key_frame: bool,
    payload: &[u8],
    session_token: Option<&str>,
) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let base_max = if session_token.is_some_and(valid_session_token) {
        MAX_AUTH_PAYLOAD_SIZE
    } else {
        MAX_PAYLOAD_SIZE
    };
    // Резервируем место под FEC-заголовок, чтобы parity влез в датаграмму.
    let chunk_size = base_max.saturating_sub(FEC_HEADER_LEN).max(1);
    let flags = if is_key_frame { FLAG_KEY_FRAME } else { 0 };

    if payload.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let packet_count = payload.len().div_ceil(chunk_size);
    if packet_count == 0 || packet_count > MAX_FRAME_PACKET_COUNT {
        return (Vec::new(), Vec::new());
    }

    let chunks: Vec<&[u8]> = payload.chunks(chunk_size).collect();
    let mut data = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        data.push(build_packet_authenticated(
            TYPE_VIDEO_FRAME,
            flags,
            frame_id,
            i as u16,
            packet_count as u16,
            presentation_time_us,
            chunk,
            session_token,
        ));
    }

    // FEC только при ≥2 пакетах.
    let fec = if chunks.len() >= 2 {
        build_fec_packets(frame_id, presentation_time_us, flags, &chunks, session_token)
    } else {
        Vec::new()
    };

    (data, fec)
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

pub fn build_roi_metadata_authenticated(roi: RoiRect, session_token: Option<&str>) -> Vec<u8> {
    let json = roi.to_json();
    let max_payload = if session_token.is_some_and(valid_session_token) {
        MAX_AUTH_PAYLOAD_SIZE
    } else {
        MAX_PAYLOAD_SIZE
    };
    if json.len() <= max_payload {
        build_single_authenticated(TYPE_ROI_METADATA, &json, session_token)
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

pub fn packetize_audio_frame_authenticated(
    frame_id: u32,
    presentation_time_us: u64,
    payload: &[u8],
    session_token: Option<&str>,
) -> Vec<Vec<u8>> {
    packetize_authenticated(
        TYPE_AUDIO_FRAME,
        0,
        frame_id,
        presentation_time_us,
        payload,
        session_token,
    )
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

pub fn build_single_authenticated(
    packet_type: u8,
    payload: &[u8],
    session_token: Option<&str>,
) -> Vec<u8> {
    let max_payload = if session_token.is_some_and(valid_session_token) {
        MAX_AUTH_PAYLOAD_SIZE
    } else {
        MAX_PAYLOAD_SIZE
    };
    assert!(
        payload.len() <= max_payload,
        "single-packet payload too large: {} > {}",
        payload.len(),
        max_payload
    );
    build_packet_authenticated(packet_type, 0, 0, 0, 1, 0, payload, session_token)
}

pub fn build_session_config(payload: &[u8]) -> Vec<u8> {
    build_single(TYPE_SESSION_CONFIG, payload)
}

pub fn build_session_config_authenticated(payload: &[u8], session_token: Option<&str>) -> Vec<u8> {
    build_single_authenticated(TYPE_SESSION_CONFIG, payload, session_token)
}

pub fn build_codec_config(payload: &[u8]) -> Vec<u8> {
    build_single(TYPE_CODEC_CONFIG, payload)
}

pub fn build_codec_config_authenticated(payload: &[u8], session_token: Option<&str>) -> Vec<u8> {
    build_single_authenticated(TYPE_CODEC_CONFIG, payload, session_token)
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
    packetize_with_limit(packet_type, flags, frame_id, presentation_time_us, payload, MAX_PAYLOAD_SIZE, None)
}

fn packetize_authenticated(
    packet_type: u8,
    flags: u16,
    frame_id: u32,
    presentation_time_us: u64,
    payload: &[u8],
    session_token: Option<&str>,
) -> Vec<Vec<u8>> {
    if session_token.is_some_and(valid_session_token) {
        packetize_with_limit(
            packet_type,
            flags,
            frame_id,
            presentation_time_us,
            payload,
            MAX_AUTH_PAYLOAD_SIZE,
            session_token,
        )
    } else {
        packetize(packet_type, flags, frame_id, presentation_time_us, payload)
    }
}

fn packetize_with_limit(
    packet_type: u8,
    flags: u16,
    frame_id: u32,
    presentation_time_us: u64,
    payload: &[u8],
    max_payload_size: usize,
    session_token: Option<&str>,
) -> Vec<Vec<u8>> {
    if payload.is_empty() {
        return Vec::new();
    }
    if max_payload_size == 0 {
        return Vec::new();
    }
    let packet_count = payload.len().div_ceil(max_payload_size);
    if packet_count > MAX_FRAME_PACKET_COUNT {
        return Vec::new();
    }
    let mut packets = Vec::with_capacity(packet_count);

    for (i, chunk) in payload.chunks(max_payload_size).enumerate() {
        packets.push(build_packet_authenticated(
            packet_type,
            flags,
            frame_id,
            i as u16,
            packet_count as u16,
            presentation_time_us,
            chunk,
            session_token,
        ));
    }
    packets
}

// ═══════════════════════════════════════════════════════════════════════════
// FEC — Forward Error Correction (XOR-parity, в духе WebRTC ULPFEC)
//
// На каждые до FEC_GROUP_SIZE data-пакетов кадра строится 1 FEC-пакет: XOR их
// payload + XOR их длин. Если в группе потерян РОВНО ОДИН data-пакет, приёмник
// восстанавливает его без ретрансмиссии:
//     lost_payload = fec.xor_payload XOR (остальные payload группы)
//     lost_len     = fec.xor_len     XOR (длины остальных)
//
// Это убирает «фриз до следующего IDR» при единичных потерях — самый частый
// паттерн потерь на Wi-Fi/мобильной сети. Накладные расходы ~1/8 трафика.
//
// FEC payload layout (после 24-байтного EVRT-заголовка типа TYPE_FEC):
//   [0..2]  base_index : u16  — packet_index первого покрытого data-пакета
//   [2]     group_size : u8   — сколько data-пакетов покрыто (1..=GROUP_SIZE)
//   [3..5]  xor_len    : u16  — XOR длин payload покрытых пакетов
//   [5..]   xor_payload       — XOR payload (каждый дополнен нулями до max в группе)
// В заголовке FEC-пакета: frame_id = кадр; packet_index = индекс FEC-группы;
// packet_count = всего FEC-групп кадра.
// ═══════════════════════════════════════════════════════════════════════════

/// Сколько data-пакетов покрывает один FEC-пакет.
pub const FEC_GROUP_SIZE: usize = 8;
const FEC_HEADER_LEN: usize = 5; // base_index(2) + group_size(1) + xor_len(2)

/// Распарсенные метаданные FEC-пакета.
#[derive(Debug, Clone)]
pub struct FecMeta {
    pub frame_id: u32,
    pub base_index: u16,
    pub group_size: u8,
    pub xor_len: u16,
    pub xor_payload: Vec<u8>,
}

/// Построить FEC-пакеты для набора data-чанков (исходные payload до заголовка).
/// `flags` копируется из кадра (для совместимости — например key-frame бит).
pub fn build_fec_packets(
    frame_id: u32,
    presentation_time_us: u64,
    flags: u16,
    chunks: &[&[u8]],
    session_token: Option<&str>,
) -> Vec<Vec<u8>> {
    if chunks.is_empty() {
        return Vec::new();
    }
    let total_groups = chunks.len().div_ceil(FEC_GROUP_SIZE);
    if total_groups > MAX_FRAME_PACKET_COUNT {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(total_groups);

    for (gi, group) in chunks.chunks(FEC_GROUP_SIZE).enumerate() {
        let base_index = (gi * FEC_GROUP_SIZE) as u16;
        let max_len = group.iter().map(|c| c.len()).max().unwrap_or(0);
        // FEC payload не должен превышать лимит датаграммы.
        let max_payload = if session_token.is_some_and(valid_session_token) {
            MAX_AUTH_PAYLOAD_SIZE
        } else {
            MAX_PAYLOAD_SIZE
        };
        if FEC_HEADER_LEN + max_len > max_payload {
            // Группа слишком большая для одной FEC-датаграммы — пропускаем
            // (data-пакеты всё равно уйдут; FEC просто не покроет эту группу).
            continue;
        }

        let mut xor_payload = vec![0u8; max_len];
        let mut xor_len: u16 = 0;
        for chunk in group {
            for (i, &b) in chunk.iter().enumerate() {
                xor_payload[i] ^= b;
            }
            xor_len ^= chunk.len() as u16;
        }

        let mut fec_payload = Vec::with_capacity(FEC_HEADER_LEN + max_len);
        fec_payload.extend_from_slice(&base_index.to_be_bytes());
        fec_payload.push(group.len() as u8);
        fec_payload.extend_from_slice(&xor_len.to_be_bytes());
        fec_payload.extend_from_slice(&xor_payload);

        out.push(build_packet_authenticated(
            TYPE_FEC,
            flags,
            frame_id,
            gi as u16,
            total_groups as u16,
            presentation_time_us,
            &fec_payload,
            session_token,
        ));
    }
    out
}

/// Разобрать payload FEC-пакета в метаданные.
pub fn parse_fec_payload(frame_id: u32, payload: &[u8]) -> Option<FecMeta> {
    if payload.len() < FEC_HEADER_LEN {
        return None;
    }
    let base_index = u16::from_be_bytes([payload[0], payload[1]]);
    let group_size = payload[2];
    if group_size == 0 || group_size as usize > FEC_GROUP_SIZE {
        return None;
    }
    let xor_len = u16::from_be_bytes([payload[3], payload[4]]);
    let xor_payload = payload[FEC_HEADER_LEN..].to_vec();
    Some(FecMeta {
        frame_id,
        base_index,
        group_size,
        xor_len,
        xor_payload,
    })
}

/// Восстановить единственный потерянный пакет группы.
///
/// `present` — присутствующие пакеты группы как (packet_index, payload).
/// Если в группе [base_index .. base_index+group_size) ровно один отсутствует,
/// возвращает (packet_index, recovered_payload). Иначе None (восстановить XOR
/// можно только при ровно одной потере).
pub fn fec_recover_one(fec: &FecMeta, present: &[(u16, &[u8])]) -> Option<(u16, Vec<u8>)> {
    let base = fec.base_index;
    let end = base.saturating_add(fec.group_size as u16);

    // Какие индексы группы присутствуют.
    let mut seen = [false; FEC_GROUP_SIZE];
    let mut present_in_group = 0usize;
    for &(idx, _) in present {
        if idx >= base && idx < end {
            let off = (idx - base) as usize;
            if off < FEC_GROUP_SIZE && !seen[off] {
                seen[off] = true;
                present_in_group += 1;
            }
        }
    }
    // Восстановление возможно только при ровно одной недостаче.
    if present_in_group + 1 != fec.group_size as usize {
        return None;
    }
    // Находим отсутствующий offset.
    let missing_off = (0..fec.group_size as usize).find(|&o| !seen[o])?;
    let missing_index = base + missing_off as u16;

    // XOR: recovered = fec.xor ^ (все присутствующие payload группы).
    let mut recovered = fec.xor_payload.clone();
    let mut len_acc = fec.xor_len;
    for &(idx, payload) in present {
        if idx >= base && idx < end {
            for (i, &b) in payload.iter().enumerate() {
                if i < recovered.len() {
                    recovered[i] ^= b;
                }
            }
            len_acc ^= payload.len() as u16;
        }
    }
    let recovered_len = len_acc as usize;
    if recovered_len > recovered.len() {
        return None; // повреждённый FEC
    }
    recovered.truncate(recovered_len);
    Some((missing_index, recovered))
}

fn auth_tag(session_token: &str, packet_body: &[u8]) -> [u8; AUTH_TAG_SIZE] {
    let mac = hmac_sha256(session_token.as_bytes(), packet_body);
    let mut tag = [0u8; AUTH_TAG_SIZE];
    tag.copy_from_slice(&mac[..AUTH_TAG_SIZE]);
    tag
}

fn auth_tag_matches(session_token: &str, packet_body: &[u8], tag: &[u8]) -> bool {
    if tag.len() != AUTH_TAG_SIZE {
        return false;
    }
    let expected = auth_tag(session_token, packet_body);
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(tag.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        key_block[..32].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    outer.finalize().into()
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
pub const SESSION_TOKEN_HEX_LEN: usize = 32;

pub fn valid_session_token(token: &str) -> bool {
    token.len() == SESSION_TOKEN_HEX_LEN && token.bytes().all(|b| b.is_ascii_hexdigit())
}

fn session_token_json(token: Option<&str>) -> String {
    match token {
        Some(token) if valid_session_token(token) => {
            format!(r#","sessionToken":"{token}""#)
        }
        _ => String::new(),
    }
}

pub fn build_request_key_frame() -> Vec<u8> {
    build_request_key_frame_with_token(None)
}

pub fn build_request_key_frame_with_token(session_token: Option<&str>) -> Vec<u8> {
    let json = format!(
        r#"{{"kind":"request_key_frame"{}}}"#,
        session_token_json(session_token)
    );
    build_control(json.as_bytes())
}

pub fn build_request_key_frame_authenticated(session_token: Option<&str>) -> Vec<u8> {
    let json = format!(
        r#"{{"kind":"request_key_frame"{}}}"#,
        session_token_json(session_token)
    );
    build_single_authenticated(TYPE_CONTROL, json.as_bytes(), session_token)
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
    build_receiver_feedback_with_token(fb, None)
}

pub fn build_receiver_feedback_with_token(
    fb: &ReceiverFeedback,
    session_token: Option<&str>,
) -> Vec<u8> {
    let json = format!(
        r#"{{"kind":"receiver_feedback","pressure":"{}","backlogFrames":{},"queueDrops":{},"decodeFps":{},"assemblyDelayMs":{},"arrivalDeltaMs":{},"decodeDeltaMs":{},"presentDeltaMs":{},"pulseEstimateMs":{},"inputEstimateMs":{}{}}}"#,
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
        session_token_json(session_token),
    );
    // Может не войти в MAX_PAYLOAD_SIZE при очень больших числах — обрезать безопасно не нужно,
    // JSON всегда < 300 байт.
    build_control(json.as_bytes())
}

pub fn build_receiver_feedback_authenticated(
    fb: &ReceiverFeedback,
    session_token: Option<&str>,
) -> Vec<u8> {
    let json = format!(
        r#"{{"kind":"receiver_feedback","pressure":"{}","backlogFrames":{},"queueDrops":{},"decodeFps":{},"assemblyDelayMs":{},"arrivalDeltaMs":{},"decodeDeltaMs":{},"presentDeltaMs":{},"pulseEstimateMs":{},"inputEstimateMs":{}{}}}"#,
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
        session_token_json(session_token),
    );
    build_single_authenticated(TYPE_CONTROL, json.as_bytes(), session_token)
}

/// Разобрать входящий control-пакет.
pub fn control_session_token(payload: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(payload).ok()?;
    let token = json_str_field(s, "sessionToken").or_else(|| json_str_field(s, "token"))?;
    valid_session_token(&token).then_some(token)
}

pub fn control_token_matches(payload: &[u8], expected: Option<&str>) -> bool {
    match expected {
        Some(expected) if valid_session_token(expected) => {
            control_session_token(payload).as_deref() == Some(expected)
        }
        Some(_) => false,
        None => true,
    }
}

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
    fn fec_recovers_single_loss_in_group() {
        // Три data-чанка разной длины.
        let c0: Vec<u8> = vec![1, 2, 3, 4];
        let c1: Vec<u8> = vec![10, 20, 30];
        let c2: Vec<u8> = vec![100, 99, 98, 97, 96];
        let chunks: Vec<&[u8]> = vec![&c0, &c1, &c2];

        let fec_pkts = build_fec_packets(7, 1000, FLAG_KEY_FRAME, &chunks, None);
        assert_eq!(fec_pkts.len(), 1); // одна группа

        let parsed = parse(&fec_pkts[0], fec_pkts[0].len()).unwrap();
        assert_eq!(parsed.packet_type, TYPE_FEC);
        let meta = parse_fec_payload(parsed.frame_id, &parsed.payload).unwrap();

        // Потерян c1 (index 1). Присутствуют c0, c2.
        let present: Vec<(u16, &[u8])> = vec![(0, &c0), (2, &c2)];
        let (idx, recovered) = fec_recover_one(&meta, &present).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(recovered, c1);
    }

    #[test]
    fn fec_no_recovery_with_two_losses() {
        let c0: Vec<u8> = vec![1, 2, 3];
        let c1: Vec<u8> = vec![4, 5, 6];
        let c2: Vec<u8> = vec![7, 8, 9];
        let chunks: Vec<&[u8]> = vec![&c0, &c1, &c2];
        let fec_pkts = build_fec_packets(1, 0, 0, &chunks, None);
        let parsed = parse(&fec_pkts[0], fec_pkts[0].len()).unwrap();
        let meta = parse_fec_payload(parsed.frame_id, &parsed.payload).unwrap();
        // Потеряны двое — восстановление невозможно.
        let present: Vec<(u16, &[u8])> = vec![(0, &c0)];
        assert!(fec_recover_one(&meta, &present).is_none());
    }

    #[test]
    fn fec_recovers_first_packet() {
        let c0: Vec<u8> = vec![9, 9, 9, 9, 9, 9];
        let c1: Vec<u8> = vec![1, 1];
        let chunks: Vec<&[u8]> = vec![&c0, &c1];
        let fec_pkts = build_fec_packets(2, 0, 0, &chunks, None);
        let parsed = parse(&fec_pkts[0], fec_pkts[0].len()).unwrap();
        let meta = parse_fec_payload(parsed.frame_id, &parsed.payload).unwrap();
        // Потерян c0 (первый).
        let present: Vec<(u16, &[u8])> = vec![(1, &c1)];
        let (idx, recovered) = fec_recover_one(&meta, &present).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(recovered, c0);
    }

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
    fn authenticated_packet_roundtrip_strips_tag() {
        let token = "0123456789abcdef0123456789abcdef";
        let pkt = build_packet_authenticated(TYPE_VIDEO_FRAME, FLAG_KEY_FRAME, 7, 0, 1, 42, b"abc", Some(token));
        assert_eq!(pkt.len(), HEADER_SIZE + 3 + AUTH_TAG_SIZE);
        let parsed = parse_authenticated(&pkt, pkt.len(), Some(token)).unwrap();
        assert_eq!(parsed.packet_type, TYPE_VIDEO_FRAME);
        assert_eq!(parsed.frame_id, 7);
        assert_eq!(parsed.payload, b"abc");
        assert!(parsed.is_key_frame());
    }

    #[test]
    fn authenticated_packet_rejects_tamper() {
        let token = "0123456789abcdef0123456789abcdef";
        let mut pkt = build_packet_authenticated(TYPE_VIDEO_FRAME, 0, 7, 0, 1, 42, b"abc", Some(token));
        pkt[HEADER_SIZE] ^= 0x01;
        assert!(parse_authenticated(&pkt, pkt.len(), Some(token)).is_none());
        assert!(parse_authenticated(&pkt, pkt.len(), Some("ffffffffffffffffffffffffffffffff")).is_none());
    }

    #[test]
    fn authenticated_packetizer_keeps_mtu_safe_datagrams() {
        let token = "0123456789abcdef0123456789abcdef";
        let payload = vec![1u8; MAX_AUTH_PAYLOAD_SIZE + 1];
        let packets = packetize_video_frame_authenticated(1, 0, true, &payload, Some(token));
        assert_eq!(packets.len(), 2);
        assert!(packets.iter().all(|pkt| pkt.len() <= MAX_PACKET_SIZE));
        let first = parse_authenticated(&packets[0], packets[0].len(), Some(token)).unwrap();
        assert_eq!(first.payload.len(), MAX_AUTH_PAYLOAD_SIZE);
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
    fn request_key_frame_carries_session_token() {
        let token = "0123456789abcdef0123456789abcdef";
        let pkt = build_request_key_frame_with_token(Some(token));
        let parsed = parse(&pkt, pkt.len()).unwrap();
        assert_eq!(control_session_token(&parsed.payload).as_deref(), Some(token));
        assert!(control_token_matches(&parsed.payload, Some(token)));
        assert!(!control_token_matches(
            &parsed.payload,
            Some("ffffffffffffffffffffffffffffffff")
        ));
    }

    #[test]
    fn authenticated_control_roundtrip() {
        let token = "0123456789abcdef0123456789abcdef";
        let pkt = build_request_key_frame_authenticated(Some(token));
        let parsed = parse_authenticated(&pkt, pkt.len(), Some(token)).unwrap();
        assert_eq!(parsed.packet_type, TYPE_CONTROL);
        assert_eq!(control_session_token(&parsed.payload).as_deref(), Some(token));
        assert!(matches!(
            parse_control(&parsed.payload),
            Some(ControlMessage::RequestKeyFrame)
        ));
    }

    #[test]
    fn receiver_feedback_carries_session_token() {
        let token = "abcdef0123456789abcdef0123456789";
        let fb = ReceiverFeedback {
            pressure: Pressure::High,
            backlog_frames: 1,
            ..ReceiverFeedback::default()
        };
        let pkt = build_receiver_feedback_with_token(&fb, Some(token));
        let parsed = parse(&pkt, pkt.len()).unwrap();
        assert_eq!(control_session_token(&parsed.payload).as_deref(), Some(token));
        let ctrl = parse_control(&parsed.payload).unwrap();
        match ctrl {
            ControlMessage::ReceiverFeedback(parsed_fb) => {
                assert_eq!(parsed_fb.pressure, Pressure::High);
                assert_eq!(parsed_fb.backlog_frames, 1);
            }
            _ => panic!("wrong kind"),
        }
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
