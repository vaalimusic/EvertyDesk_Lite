// =============================================================================
// EVRT2 — Wire Packet Format
// Spec: evrt2/spec/EVRT2_PACKET.md
// Spec: evrt2/tasks/01_ABSOLUTE_NO_DELAY_VISIBLE_REGION.md § Wire signal
//       (VISIBLE_REGION, bit 8 — was "Reserved" in the base spec)
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! The 32-byte EVRT2 packet header, byte-exact per the wire spec. This is
//! the foundation every other EVRT2 module builds on (scheduler, FEC,
//! jitter buffer all operate on packets built with this header).
//!
//! Field layout (network byte order / big-endian throughout, matching the
//! spec's bit diagram which is drawn MSB-first):
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                     Magic  0x45565232 ("EVR2")                | 4
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |  Ver=2  | Type  |    Mode   |          Flags (16-bit)         | 8
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                          FrameId (32-bit)                     | 12
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |        PacketIndex (16-bit)   |      PacketCount (16-bit)     | 16
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                  PresentationTimeUs (64-bit)                  | 20
//! |                                                               | 24
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |   FEC Group (8-bit) | FEC Idx | FEC Total  |   Reserved      | 28
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                     AuthTag (32-bit truncated)                | 32
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                          Payload …                            |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! Note on the `Ver=4|Type=4` byte: the diagram packs a 4-bit version and a
//! 4-bit type into one byte (high nibble = Ver, low nibble = Type) — the
//! spec's field table lists them as separate 4-bit fields immediately
//! adjacent with no byte boundary drawn between them and Mode, so this is
//! the only layout consistent with "Ver(4) Type(4) Mode(8) Flags(16)" = 4
//! bytes total for that row.

use std::fmt;

/// `0x45565232` — "EVR2". Distinguishes EVRT2 packets from EVRT1
/// (`0x45565254` = "EVRT") at the very first four bytes, per
/// EVRT2_OVERVIEW.md § Compatibility with EVRT (2025).
pub const MAGIC: u32 = 0x4556_5232;

/// Protocol version carried in the Ver nibble.
pub const VERSION: u8 = 2;

/// Fixed header size in bytes (32, vs EVRT1's 24 — see EVRT2_PACKET.md).
pub const HEADER_LEN: usize = 32;

/// MTU-safe maximum UDP datagram size (IPv6 + GRE headroom).
pub const MAX_DATAGRAM: usize = 1400;

/// Maximum payload per packet: `MAX_DATAGRAM - HEADER_LEN`.
pub const MAX_PAYLOAD: usize = MAX_DATAGRAM - HEADER_LEN;

// ── Packet Types (EVRT2_PACKET.md § Packet Types) ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PacketType {
    SessionHello = 0x01,
    SessionAck = 0x02,
    CodecConfig = 0x03,
    ModeSwitch = 0x04,
    VideoFrame = 0x05,
    AudioFrame = 0x06,
    Feedback = 0x07,
    Keepalive = 0x08,
    FecRepair = 0x09,
    IdrRequest = 0x0A,
    Goodbye = 0x0B,
    RelayWrap = 0x0C,
    /// EVRT2CKMAX-TASK-01 § Breach Handling — `DEGRADE_SIGNAL { region,
    /// measured_age }`. Not in the base EVRT2_PACKET.md table (drafted
    /// before Task 01); assigned the next free type value after RELAY_WRAP.
    DegradeSignal = 0x0D,
    /// EVRT2CKMAX.md § Attention Priority Field — the wire representation
    /// of the Attention Map (ROADMAP.md Phase 3). Not in the base
    /// EVRT2_PACKET.md table (APF's own doc never assigned it a
    /// PacketType); assigned the next free value after DEGRADE_SIGNAL.
    ApfUpdate = 0x0E,
}

impl PacketType {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0x01 => Self::SessionHello,
            0x02 => Self::SessionAck,
            0x03 => Self::CodecConfig,
            0x04 => Self::ModeSwitch,
            0x05 => Self::VideoFrame,
            0x06 => Self::AudioFrame,
            0x07 => Self::Feedback,
            0x08 => Self::Keepalive,
            0x09 => Self::FecRepair,
            0x0A => Self::IdrRequest,
            0x0B => Self::Goodbye,
            0x0C => Self::RelayWrap,
            0x0D => Self::DegradeSignal,
            0x0E => Self::ApfUpdate,
            _ => return None,
        })
    }
}

// ── Mode byte (AR2R47_MODES.md) ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Mode {
    /// Static/support — lossless tile-delta, named for the first two
    /// letters of ARtur.
    Ar = 0x01,
    /// Dynamic/video — hybrid silicon+delta, the two R's in aRtuR.
    R2 = 0x02,
    /// Gaming, no compromises — silicon-only. `0x47` = ASCII 'G', and the
    /// last two characters of ARTUR **47**.
    Mode47 = 0x47,
}

impl Mode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Ar),
            0x02 => Some(Self::R2),
            0x47 => Some(Self::Mode47),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ar => "AR",
            Self::R2 => "2R",
            Self::Mode47 => "47",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

// ── Flags (16-bit bitmask, EVRT2_PACKET.md § Flags) ────────────────────────────

pub mod flags {
    pub const IS_KEYFRAME: u16 = 1 << 0;
    pub const IS_SILICON: u16 = 1 << 1;
    pub const HAS_AUDIO: u16 = 1 << 2;
    pub const ENCRYPTED: u16 = 1 << 3;
    pub const COMPRESSED: u16 = 1 << 4;
    pub const ROI_HINT: u16 = 1 << 5;
    pub const FEC_ENABLED: u16 = 1 << 6;
    pub const RELAY_MODE: u16 = 1 << 7;
    /// EVRT2CKMAX-TASK-01 § Wire signal — "A new flag in the EVRT2 packet
    /// header (Flags field, bit 8, currently reserved): this packet is
    /// part of the current Visible Region." The client's jitter buffer
    /// treats these with `buffer_depth = 0` — see `evrt2_jitter.rs`.
    pub const VISIBLE_REGION: u16 = 1 << 8;
    /// ROADMAP.md Phase 6.3′ H265 A/B test: only meaningful alongside
    /// `IS_SILICON` — distinguishes an NVENC H265 bitstream from the
    /// default NVENC H264 one, since `IS_SILICON` alone doesn't say which
    /// codec produced the bytes and the client needs to know before it can
    /// pick a decoder (`openh264` is H264-only; H265 routes to Android's
    /// MediaCodec Surface decode instead — see `run_client_experiment`).
    pub const IS_H265: u16 = 1 << 9;
    /// Bits 10-15 remain reserved (must be 0) per the base spec.
    pub const RESERVED_MASK: u16 = 0b1111_1100_0000_0000;
}

// ── Header ──────────────────────────────────────────────────────────────────

/// The full 32-byte EVRT2 packet header, decoded into native fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub packet_type: PacketType,
    pub mode: Mode,
    pub flags: u16,
    pub frame_id: u32,
    pub packet_index: u16,
    pub packet_count: u16,
    pub presentation_time_us: u64,
    pub fec_group: u8,
    pub fec_idx: u8,
    pub fec_total: u8,
    pub auth_tag: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderDecodeError {
    TooShort { got: usize, need: usize },
    BadMagic { got: u32 },
    UnsupportedVersion { got: u8 },
    UnknownPacketType { got: u8 },
    UnknownMode { got: u8 },
}

impl fmt::Display for HeaderDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { got, need } => {
                write!(f, "packet too short: {got} bytes, need at least {need}")
            }
            Self::BadMagic { got } => write!(f, "bad magic: 0x{got:08X}, expected 0x{MAGIC:08X}"),
            Self::UnsupportedVersion { got } => write!(f, "unsupported version: {got}"),
            Self::UnknownPacketType { got } => write!(f, "unknown packet type: 0x{got:02X}"),
            Self::UnknownMode { got } => write!(f, "unknown mode byte: 0x{got:02X}"),
        }
    }
}
impl std::error::Error for HeaderDecodeError {}

impl PacketHeader {
    /// Encode this header (32 bytes) followed by `payload`, into one buffer.
    /// `auth_tag` here is the value already computed by the caller (session
    /// crypto layer) — this module doesn't compute HMAC itself, it only
    /// carries the field; see the note on `auth_tag` below.
    pub fn encode(&self, payload: &[u8], out: &mut Vec<u8>) {
        out.reserve(HEADER_LEN + payload.len());
        out.extend_from_slice(&MAGIC.to_be_bytes());

        let ver_type = (VERSION << 4) | (self.packet_type as u8 & 0x0F);
        out.push(ver_type);
        out.push(self.mode as u8);
        out.extend_from_slice(&self.flags.to_be_bytes());

        out.extend_from_slice(&self.frame_id.to_be_bytes());
        out.extend_from_slice(&self.packet_index.to_be_bytes());
        out.extend_from_slice(&self.packet_count.to_be_bytes());
        out.extend_from_slice(&self.presentation_time_us.to_be_bytes());

        out.push(self.fec_group);
        out.push(self.fec_idx);
        out.push(self.fec_total);
        out.push(0); // Reserved — must be 0

        out.extend_from_slice(&self.auth_tag.to_be_bytes());

        out.extend_from_slice(payload);
    }

    /// Decode the 32-byte header from the front of `data`. Returns the
    /// header and the payload slice (everything after byte 32).
    pub fn decode(data: &[u8]) -> Result<(Self, &[u8]), HeaderDecodeError> {
        if data.len() < HEADER_LEN {
            return Err(HeaderDecodeError::TooShort {
                got: data.len(),
                need: HEADER_LEN,
            });
        }

        let magic = u32::from_be_bytes(data[0..4].try_into().unwrap());
        if magic != MAGIC {
            return Err(HeaderDecodeError::BadMagic { got: magic });
        }

        let ver_type = data[4];
        let ver = ver_type >> 4;
        if ver != VERSION {
            return Err(HeaderDecodeError::UnsupportedVersion { got: ver });
        }
        let type_nibble = ver_type & 0x0F;
        let packet_type = PacketType::from_u8(type_nibble)
            .ok_or(HeaderDecodeError::UnknownPacketType { got: type_nibble })?;

        let mode_byte = data[5];
        let mode =
            Mode::from_u8(mode_byte).ok_or(HeaderDecodeError::UnknownMode { got: mode_byte })?;

        let flags = u16::from_be_bytes(data[6..8].try_into().unwrap());
        let frame_id = u32::from_be_bytes(data[8..12].try_into().unwrap());
        let packet_index = u16::from_be_bytes(data[12..14].try_into().unwrap());
        let packet_count = u16::from_be_bytes(data[14..16].try_into().unwrap());
        let presentation_time_us = u64::from_be_bytes(data[16..24].try_into().unwrap());
        let fec_group = data[24];
        let fec_idx = data[25];
        let fec_total = data[26];
        // data[27] = Reserved, ignored on decode (must-be-0 is a sender
        // obligation, not something we reject on — matches how EVRT1's
        // reserved fields are treated elsewhere in this codebase).
        let auth_tag = u32::from_be_bytes(data[28..32].try_into().unwrap());

        let header = PacketHeader {
            packet_type,
            mode,
            flags,
            frame_id,
            packet_index,
            packet_count,
            presentation_time_us,
            fec_group,
            fec_idx,
            fec_total,
            auth_tag,
        };
        Ok((header, &data[HEADER_LEN..]))
    }

    pub fn has_flag(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    pub fn is_visible_region(&self) -> bool {
        self.has_flag(flags::VISIBLE_REGION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header() -> PacketHeader {
        PacketHeader {
            packet_type: PacketType::VideoFrame,
            mode: Mode::Mode47,
            flags: flags::IS_KEYFRAME | flags::IS_SILICON | flags::VISIBLE_REGION,
            frame_id: 0xDEAD_BEEF,
            packet_index: 3,
            packet_count: 10,
            presentation_time_us: 1_700_000_000_123_456,
            fec_group: 7,
            fec_idx: 1,
            fec_total: 8,
            auth_tag: 0xCAFE_BABE,
        }
    }

    #[test]
    fn header_round_trips_exactly() {
        let header = sample_header();
        let payload = b"hello evrt2 payload";
        let mut wire = Vec::new();
        header.encode(payload, &mut wire);

        assert_eq!(wire.len(), HEADER_LEN + payload.len());

        let (decoded, decoded_payload) = PacketHeader::decode(&wire).expect("must decode");
        assert_eq!(decoded, header);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn header_is_exactly_32_bytes_with_empty_payload() {
        let header = sample_header();
        let mut wire = Vec::new();
        header.encode(&[], &mut wire);
        assert_eq!(wire.len(), HEADER_LEN);
    }

    #[test]
    fn magic_bytes_match_spec_and_differ_from_evrt1() {
        // EVRT2_OVERVIEW.md § Compatibility: EVRT1 magic = 0x45565254 ("EVRT"),
        // EVRT2 magic = 0x45565232 ("EVR2"). A host must be able to tell them
        // apart from the first 4 bytes alone, before any version parsing.
        const EVRT1_MAGIC: u32 = 0x4556_5254;
        assert_ne!(MAGIC, EVRT1_MAGIC);
        assert_eq!(&MAGIC.to_be_bytes(), b"EVR2");
        assert_eq!(&EVRT1_MAGIC.to_be_bytes(), b"EVRT");
    }

    #[test]
    fn mode_byte_47_is_ascii_g() {
        // AR2R47_MODES.md: "0x47 in the Mode byte is ASCII 'G' for Gaming."
        assert_eq!(Mode::Mode47 as u8, b'G');
    }

    #[test]
    fn decode_rejects_short_buffer() {
        let short = vec![0u8; HEADER_LEN - 1];
        assert_eq!(
            PacketHeader::decode(&short),
            Err(HeaderDecodeError::TooShort {
                got: HEADER_LEN - 1,
                need: HEADER_LEN
            })
        );
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut wire = vec![0u8; HEADER_LEN];
        wire[0..4].copy_from_slice(&0x1234_5678u32.to_be_bytes());
        assert_eq!(
            PacketHeader::decode(&wire),
            Err(HeaderDecodeError::BadMagic { got: 0x1234_5678 })
        );
    }

    #[test]
    fn decode_rejects_unknown_packet_type() {
        let header = sample_header();
        let mut wire = Vec::new();
        header.encode(&[], &mut wire);
        // Corrupt the low nibble of the Ver|Type byte to an unassigned type.
        wire[4] = (VERSION << 4) | 0x0F;
        assert_eq!(
            PacketHeader::decode(&wire),
            Err(HeaderDecodeError::UnknownPacketType { got: 0x0F })
        );
    }

    #[test]
    fn visible_region_flag_round_trips() {
        let mut header = sample_header();
        header.flags = flags::VISIBLE_REGION;
        let mut wire = Vec::new();
        header.encode(&[], &mut wire);
        let (decoded, _) = PacketHeader::decode(&wire).unwrap();
        assert!(decoded.is_visible_region());
        assert!(!decoded.has_flag(flags::IS_KEYFRAME));
    }

    #[test]
    fn empty_payload_packet_fits_max_datagram() {
        let mut wire = Vec::new();
        sample_header().encode(&vec![0u8; MAX_PAYLOAD], &mut wire);
        assert_eq!(wire.len(), MAX_DATAGRAM);
    }

    #[test]
    fn all_packet_types_round_trip() {
        for &(v, expected) in &[
            (0x01u8, PacketType::SessionHello),
            (0x02, PacketType::SessionAck),
            (0x03, PacketType::CodecConfig),
            (0x04, PacketType::ModeSwitch),
            (0x05, PacketType::VideoFrame),
            (0x06, PacketType::AudioFrame),
            (0x07, PacketType::Feedback),
            (0x08, PacketType::Keepalive),
            (0x09, PacketType::FecRepair),
            (0x0A, PacketType::IdrRequest),
            (0x0B, PacketType::Goodbye),
            (0x0C, PacketType::RelayWrap),
            (0x0D, PacketType::DegradeSignal),
            (0x0E, PacketType::ApfUpdate),
        ] {
            assert_eq!(PacketType::from_u8(v), Some(expected));
            assert_eq!(expected as u8, v);
        }
    }

    #[test]
    fn all_modes_round_trip() {
        assert_eq!(Mode::from_u8(0x01), Some(Mode::Ar));
        assert_eq!(Mode::from_u8(0x02), Some(Mode::R2));
        assert_eq!(Mode::from_u8(0x47), Some(Mode::Mode47));
        assert_eq!(Mode::from_u8(0x99), None);
    }
}
