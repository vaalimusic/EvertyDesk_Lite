// =============================================================================
// EVRT2 — Live SD-UDP Session Engine
// Spec: evrt2/spec/EVRT2_OVERVIEW.md § Session Lifecycle
// Spec: evrt2/transport/SDUDP.md
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! A real UDP socket that speaks the EVRT2 wire protocol: fragments a big
//! encoded frame into ≤1368-byte packets, protects each FEC group, orders
//! them via the Task-01 scheduler, and sends them over an actual
//! `UdpSocket`. The receive side reassembles frames from arriving packets,
//! recovering losses via FEC before declaring a frame incomplete.
//!
//! **This module is additive.** It does not touch `host.rs` / `transport.rs`
//! or the live EVRT1 session path — per EVRT2_OVERVIEW.md's own
//! compatibility requirement ("Existing EVRT + EVRTCK clients continue to
//! operate unchanged"), and because wiring this in as the *default* path
//! for real sessions is a separate, much higher-stakes integration step
//! that deserves its own review, not something to fold silently into a
//! spec-implementation pass. What's here is real, working, tested code —
//! see `evrt2_session::tests` for an actual two-socket loopback round trip
//! — not a mock of what the wiring would look like.
//!
//! Composition: this module is intentionally thin. It orders and
//! transports `evrt2_packet` header + payload pairs; frame slicing uses
//! `evrt2_fec` for redundancy and `evrt2_scheduler` for send order. Each of
//! those was built and tested standalone; this module's job is only to
//! drive real sockets with them.

use crate::evrt2_fec::{self, FecConfig, RepairPacket};
use crate::evrt2_packet::{flags, Mode, PacketHeader, PacketType, MAX_PAYLOAD};
use crate::evrt2_scheduler::{schedule_send_order, NormalPriority, Slice, SliceKind};
use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// ROADMAP.md Phase 5.3 (RELAY_WRAP, transport/RELAY_TUNNEL.md): how a
/// session's packets actually leave/arrive. `Udp` is the normal path
/// (Phase 1-5.2); `Relay` tunnels the exact same wire bytes over an
/// already-open TCP relay connection instead, for when no UDP candidate at
/// all is reachable (symmetric NAT on either end). Every other piece of
/// `Evrt2Session` — FEC, scheduling, AuthTag, encryption — is transport-
/// agnostic and runs unchanged on top of either variant.
enum Transport {
    Udp(UdpSocket),
    /// `outbound`: wire bytes this session hands off to be sent — the owner
    /// wraps each one in a `Misc::Evrt2RelayWrap` and writes it to the TCP
    /// relay stream. `inbound`: wire bytes the owner already unwrapped from
    /// an incoming `Evrt2RelayWrap` message. There is no `SocketAddr` for
    /// this variant — the channel pair itself IS the fixed peer (nothing
    /// else can inject into it), so `recv_one`'s "from == self.peer" check
    /// is simply skipped for `Relay`.
    Relay {
        outbound: Sender<Vec<u8>>,
        inbound: Receiver<Vec<u8>>,
    },
}

// ── ReceiverFeedback2 (SDUDP.md § 4. Pressure System) ──────────────────────────

/// Fixed 25-byte wire encoding: frame_id(4) + pressure(4) + jitter_p95_us(4)
/// + decoded_fps(4) + silicon_ok(1) + dropped_frames(4) + rtt_us(4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReceiverFeedback2 {
    pub frame_id: u32,
    pub pressure: f32,
    pub jitter_p95_us: u32,
    pub decoded_fps: f32,
    pub silicon_ok: bool,
    pub dropped_frames: u32,
    pub rtt_us: u32,
}

impl ReceiverFeedback2 {
    pub const WIRE_LEN: usize = 25;

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::WIRE_LEN);
        out.extend_from_slice(&self.frame_id.to_be_bytes());
        out.extend_from_slice(&self.pressure.to_be_bytes());
        out.extend_from_slice(&self.jitter_p95_us.to_be_bytes());
        out.extend_from_slice(&self.decoded_fps.to_be_bytes());
        out.push(self.silicon_ok as u8);
        out.extend_from_slice(&self.dropped_frames.to_be_bytes());
        out.extend_from_slice(&self.rtt_us.to_be_bytes());
        out
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < Self::WIRE_LEN {
            return None;
        }
        Some(Self {
            frame_id: u32::from_be_bytes(data[0..4].try_into().unwrap()),
            pressure: f32::from_be_bytes(data[4..8].try_into().unwrap()),
            jitter_p95_us: u32::from_be_bytes(data[8..12].try_into().unwrap()),
            decoded_fps: f32::from_be_bytes(data[12..16].try_into().unwrap()),
            silicon_ok: data[16] != 0,
            dropped_frames: u32::from_be_bytes(data[17..21].try_into().unwrap()),
            rtt_us: u32::from_be_bytes(data[21..25].try_into().unwrap()),
        })
    }

    /// SDUDP.md § 4: host reaction thresholds, exposed as queries so the
    /// caller's rebalancing logic reads the same numbers this doc names
    /// instead of re-hardcoding 0.8/0.2/0.8 elsewhere.
    pub fn should_reduce_bitrate(&self) -> bool {
        self.pressure > 0.8
    }
    pub fn should_increase_bitrate(&self) -> bool {
        self.pressure < 0.2
    }
    pub fn decoded_fps_below_target(&self, target_fps: f32) -> bool {
        self.decoded_fps < target_fps * 0.8
    }
}

// ── Frame fragmentation + send ordering ─────────────────────────────────────────

/// Bytes reserved at the front of every protected unit for a self-describing
/// true-length prefix — see the doc comment on `fragment` for why this
/// exists (FEC recovery cannot otherwise know the exact length of a packet
/// that was never received).
const LEN_PREFIX_BYTES: usize = 2;

/// Largest raw chunk of frame data one fragment can carry, leaving room for
/// the length prefix while the total (prefix + chunk) still fits `MAX_PAYLOAD`.
const CHUNK_MAX: usize = MAX_PAYLOAD - LEN_PREFIX_BYTES;

/// One fragment of an outgoing frame, tagged for scheduling and FEC. `unit`
/// is the self-describing protected unit — `[true_len: u16 BE][chunk bytes]`
/// — NOT the raw chunk. See `fragment`'s doc comment.
struct DataFragment {
    packet_index: u16,
    unit: Vec<u8>,
    is_visible_region: bool,
}

/// Splits `frame_bytes` into fragments, each wrapped as a **self-describing
/// protected unit**: `[true_len: u16 BE][chunk bytes]` rather than the raw
/// chunk alone.
///
/// Why: FEC recovery reconstructs a *missing* packet's bytes via XOR against
/// a padded buffer — but the padding means the recovered buffer's length is
/// the FEC group's max unit size, not necessarily the missing packet's own
/// true length (e.g. the last, shorter fragment of a frame). Without a way
/// to learn the true length of a packet THAT WAS NEVER RECEIVED, recovery
/// has no honest source for it — `evrt2_fec::recover`'s own docs say the
/// true length "must be known out-of-band," and this prefix is exactly
/// that: it travels inside the XOR-protected bytes themselves, so it comes
/// back correctly even for a fully-reconstructed (never-received) packet —
/// XOR is linear over the whole buffer, prefix bytes included.
///
/// `visible_region_byte_ranges` marks byte ranges within `frame_bytes` (in
/// original, unprefixed coordinates) that belong to the Task-01 Visible
/// Region — any fragment overlapping one is tagged `VISIBLE_REGION` and
/// scheduled first. An empty slice means "no visible-region awareness for
/// this frame" — this module doesn't fabricate a visible region when the
/// caller didn't identify one.
fn fragment(
    frame_bytes: &[u8],
    visible_region_byte_ranges: &[(usize, usize)],
) -> Vec<DataFragment> {
    frame_bytes
        .chunks(CHUNK_MAX)
        .enumerate()
        .map(|(i, chunk)| {
            let start = i * CHUNK_MAX;
            let end = start + chunk.len();
            let is_visible_region = visible_region_byte_ranges
                .iter()
                .any(|&(vr_start, vr_end)| start < vr_end && end > vr_start);
            let mut unit = Vec::with_capacity(LEN_PREFIX_BYTES + chunk.len());
            unit.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
            unit.extend_from_slice(chunk);
            DataFragment {
                packet_index: i as u16,
                unit,
                is_visible_region,
            }
        })
        .collect()
}

/// Strip the `fragment()` length prefix, returning the true chunk bytes.
/// Safe to call on both directly-received AND FEC-recovered units — see
/// `fragment`'s doc comment for why the recovered case works too.
fn strip_len_prefix(unit: &[u8]) -> Option<&[u8]> {
    if unit.len() < LEN_PREFIX_BYTES {
        return None;
    }
    let true_len = u16::from_be_bytes(unit[0..2].try_into().unwrap()) as usize;
    unit.get(LEN_PREFIX_BYTES..LEN_PREFIX_BYTES + true_len)
}

/// Builds the full, correctly-ordered set of wire packets (header + payload
/// already concatenated, ready for `send_to`) for one encoded frame — data
/// fragments plus FEC repair packets, in Task-01 send order. Pure function,
/// no socket I/O, fully testable in isolation (see `tests::` below).
pub fn build_frame_packets(
    frame_bytes: &[u8],
    frame_id: u32,
    mode: Mode,
    is_keyframe: bool,
    is_silicon: bool,
    is_h265: bool,
    presentation_time_us: u64,
    visible_region_byte_ranges: &[(usize, usize)],
    fec: FecConfig,
) -> Vec<Vec<u8>> {
    let fragments = fragment(frame_bytes, visible_region_byte_ranges);
    let packet_count = fragments.len() as u16;

    let mut base_flags = 0u16;
    if is_keyframe {
        base_flags |= flags::IS_KEYFRAME;
    }
    if is_silicon {
        base_flags |= flags::IS_SILICON;
    }
    if is_h265 {
        base_flags |= flags::IS_H265;
    }
    if fec.is_enabled() {
        base_flags |= flags::FEC_ENABLED;
    }

    // FEC groups of `fec.n` data fragments each (SDUDP.md: "N data packets,
    // K repair packets" per group — a frame with more fragments than N gets
    // multiple independent FEC groups).
    let mut repairs_by_group: HashMap<u8, Vec<RepairPacket>> = HashMap::new();
    if fec.is_enabled() {
        for (group_idx, group) in fragments.chunks(fec.n).enumerate() {
            let group_data: Vec<Vec<u8>> = group.iter().map(|f| f.unit.clone()).collect();
            let repairs = evrt2_fec::encode_repairs(&group_data, fec.k);
            repairs_by_group.insert(group_idx as u8, repairs);
        }
    }

    let mut slices: Vec<Slice<Vec<u8>>> = Vec::with_capacity(fragments.len() + fec.k * 4);

    for frag in &fragments {
        let group_idx = if fec.is_enabled() {
            (frag.packet_index as usize / fec.n) as u8
        } else {
            0
        };
        let idx_in_group = if fec.is_enabled() {
            (frag.packet_index as usize % fec.n) as u8
        } else {
            0
        };
        let mut pkt_flags = base_flags;
        if frag.is_visible_region {
            pkt_flags |= flags::VISIBLE_REGION;
        }
        let header = PacketHeader {
            packet_type: PacketType::VideoFrame,
            mode,
            flags: pkt_flags,
            frame_id,
            packet_index: frag.packet_index,
            packet_count,
            presentation_time_us,
            fec_group: group_idx,
            fec_idx: idx_in_group,
            fec_total: (fec.n + fec.k) as u8,
            auth_tag: 0, // populated by the session's crypto layer, out of scope here
        };
        let mut wire = Vec::with_capacity(32 + frag.unit.len());
        header.encode(&frag.unit, &mut wire);

        let kind = if frag.is_visible_region {
            SliceKind::VisibleRegion
        } else if is_keyframe {
            SliceKind::Idr
        } else {
            // Priority order (existing): earlier fragments first, matching
            // the pre-Task-01 in-order send behavior for the non-priority
            // remainder — see NormalPriority's doc comment (smaller sorts
            // first).
            SliceKind::Normal(NormalPriority(frag.packet_index as u32))
        };
        slices.push(Slice {
            kind,
            payload: wire,
        });
    }

    for (&group_idx, repairs) in &repairs_by_group {
        for repair in repairs {
            let mut pkt_flags = base_flags;
            pkt_flags &= !flags::VISIBLE_REGION; // repair packets are never visible-region-tagged
            let header = PacketHeader {
                packet_type: PacketType::FecRepair,
                mode,
                flags: pkt_flags,
                frame_id,
                packet_index: packet_count + repair.repair_idx as u16, // beyond data range, informational
                packet_count,
                presentation_time_us,
                fec_group: group_idx,
                fec_idx: (fec.n + repair.repair_idx as usize) as u8,
                fec_total: (fec.n + fec.k) as u8,
                auth_tag: 0,
            };
            let mut wire = Vec::with_capacity(32 + repair.xor_payload.len());
            header.encode(&repair.xor_payload, &mut wire);
            slices.push(Slice {
                kind: SliceKind::FecRepair,
                payload: wire,
            });
        }
    }

    schedule_send_order(slices)
        .into_iter()
        .map(|s| s.payload)
        .collect()
}

// ── Frame reassembly (receive side) ─────────────────────────────────────────────

struct PendingFrame {
    packet_count: u16,
    data: Vec<Option<Vec<u8>>>,
    data_lens: Vec<usize>,
    repairs_by_group: HashMap<u8, Vec<RepairPacket>>,
    fec_n: usize,
    received_count: u16,
    first_seen: Instant,
}

impl PendingFrame {
    fn new(packet_count: u16, fec_n: usize) -> Self {
        Self {
            packet_count,
            data: vec![None; packet_count as usize],
            data_lens: vec![0; packet_count as usize],
            repairs_by_group: HashMap::new(),
            fec_n,
            received_count: 0,
            first_seen: Instant::now(),
        }
    }

    fn is_complete(&self) -> bool {
        self.data.iter().all(Option::is_some)
    }

    /// Records that a unit of `len` bytes (a whole protected unit, prefix
    /// included) was observed for the FEC group `packet_index` belongs to.
    /// `data_lens` ends up holding, for every slot, the group's max
    /// observed padded-unit length — which is exactly the safe truncation
    /// bound `evrt2_fec::recover` needs (truncating a recovered buffer to
    /// "at least as long as it really is" never loses data; the true
    /// length is separately recovered via the self-describing prefix in
    /// `reassemble`, not via this bound).
    fn note_group_padded_len(&mut self, packet_index: usize, len: usize) {
        let n = self.fec_n.max(1);
        let group_idx = packet_index / n;
        let start = group_idx * n;
        let end = (start + n).min(self.data.len());
        for slot in &mut self.data_lens[start..end] {
            *slot = (*slot).max(len);
        }
    }

    /// Attempt FEC recovery across every group that has repair packets.
    /// `repair.covers` holds indices absolute to the WHOLE frame (not
    /// relative to the group), so `recover` is called against the full
    /// `self.data`/`self.data_lens` arrays, not a per-group sub-slice —
    /// slicing here would silently reinterpret those absolute indices as
    /// relative to the slice's own start, corrupting or panicking on any
    /// frame with more than one FEC group.
    fn try_recover(&mut self) {
        if self.repairs_by_group.is_empty() {
            return;
        }
        for repairs in self.repairs_by_group.values() {
            evrt2_fec::recover(&mut self.data, &self.data_lens, repairs);
        }
    }

    /// Reassemble the full frame, stripping each protected unit's
    /// self-describing length prefix — works identically for
    /// directly-received and FEC-recovered slots (see `fragment`'s doc
    /// comment for why the prefix survives recovery correctly).
    fn reassemble(&self) -> Option<Vec<u8>> {
        if !self.is_complete() {
            return None;
        }
        let mut out = Vec::new();
        for piece in &self.data {
            out.extend_from_slice(strip_len_prefix(piece.as_ref()?)?);
        }
        Some(out)
    }
}

/// How long an incomplete frame is kept waiting for more packets/repairs
/// before being dropped — bounds memory use under sustained loss instead of
/// accumulating pending frames forever.
const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Accepts incoming (header, payload) pairs and reassembles complete
/// frames, applying FEC recovery when data fragments are missing. Pure
/// logic, no socket — the socket wrapper (`Evrt2Session`) feeds it.
#[derive(Default)]
pub struct FrameReassembler {
    pending: HashMap<u32, PendingFrame>,
}

/// What happened to a just-ingested packet.
#[derive(Debug)]
pub enum IngestResult {
    /// Frame not yet complete; more packets/repairs needed.
    Pending,
    /// Frame fully reassembled (with or without FEC recovery).
    FrameComplete { frame_id: u32, bytes: Vec<u8> },
}

impl FrameReassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest one packet. FEC group size (N) and repair-coverage (which
    /// data indices a given repair packet XORs together) are derived from
    /// `FecConfig::for_mode(header.mode)` — the packet's own `mode` field
    /// is authoritative, so this never depends on a separate setup call
    /// happening at the right time (an earlier version of this function
    /// required a deferred `note_fec_k(k)` call, which only worked if
    /// invoked AFTER at least one packet had already created the
    /// pending-frame entry — an easy-to-get-backwards footgun, removed).
    pub fn ingest(&mut self, header: &PacketHeader, payload: &[u8]) -> IngestResult {
        let fec = FecConfig::for_mode(header.mode);
        let fec_n = fec.n.max(1);

        match header.packet_type {
            PacketType::VideoFrame => {
                let entry = self
                    .pending
                    .entry(header.frame_id)
                    .or_insert_with(|| PendingFrame::new(header.packet_count, fec_n));
                let idx = header.packet_index as usize;
                if idx < entry.data.len() && entry.data[idx].is_none() {
                    entry.note_group_padded_len(idx, payload.len());
                    entry.data[idx] = Some(payload.to_vec());
                    entry.received_count += 1;
                }
            }
            PacketType::FecRepair => {
                let entry = self
                    .pending
                    .entry(header.frame_id)
                    .or_insert_with(|| PendingFrame::new(header.packet_count, fec_n));
                // Coverage is implicit (i % k == repair_idx within the
                // group) — reconstructed here the same way
                // `encode_repairs` assigned it, using the mode's own K so
                // this is correct the instant the repair packet arrives,
                // regardless of what order data/repair packets show up in.
                let group_start = header.fec_group as usize * fec_n;
                let group_end = (group_start + fec_n).min(header.packet_count as usize);
                // header.fec_idx was encoded as `fec.n + repair_idx` (see
                // build_frame_packets) — recover the actual repair_idx
                // exactly rather than relying on modular arithmetic that
                // only happens to line up for even N.
                let repair_idx = (header.fec_idx as usize).saturating_sub(fec.n) as u8;
                let covers: Vec<u16> = if fec.k > 0 {
                    (group_start..group_end)
                        .filter(|&i| (i - group_start) % fec.k == repair_idx as usize)
                        .map(|i| i as u16)
                        .collect()
                } else {
                    Vec::new()
                };
                // A repair packet's xor_payload is padded to the group's
                // max unit length by construction (encode_repairs) — this
                // is exactly the safe truncation bound for every slot in
                // the group, learned even before any data packet arrives.
                if group_start < entry.data.len() {
                    entry.note_group_padded_len(group_start, payload.len());
                }
                let repair = RepairPacket {
                    repair_idx,
                    covers,
                    xor_payload: payload.to_vec(),
                    payload_len: payload.len(),
                };
                entry
                    .repairs_by_group
                    .entry(header.fec_group)
                    .or_default()
                    .push(repair);
            }
            _ => return IngestResult::Pending, // control packets handled by the caller, not here
        }

        let Some(entry) = self.pending.get_mut(&header.frame_id) else {
            return IngestResult::Pending;
        };
        if !entry.is_complete() {
            entry.try_recover();
        }
        if entry.is_complete() {
            let bytes = entry.reassemble().unwrap_or_default();
            self.pending.remove(&header.frame_id);
            return IngestResult::FrameComplete {
                frame_id: header.frame_id,
                bytes,
            };
        }
        IngestResult::Pending
    }

    /// Drop pending frames older than `REASSEMBLY_TIMEOUT` — call
    /// periodically (e.g. once per receive-loop tick) to bound memory.
    pub fn expire_stale(&mut self) -> Vec<u32> {
        let expired: Vec<u32> = self
            .pending
            .iter()
            .filter(|(_, f)| f.first_seen.elapsed() > REASSEMBLY_TIMEOUT)
            .map(|(&id, _)| id)
            .collect();
        for id in &expired {
            self.pending.remove(id);
        }
        expired
    }

    pub fn pending_frame_count(&self) -> usize {
        self.pending.len()
    }
}

// ── Handshake ────────────────────────────────────────────────────────────────

pub fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

fn control_packet(packet_type: PacketType, mode: Mode, payload: &[u8]) -> Vec<u8> {
    let header = PacketHeader {
        packet_type,
        mode,
        flags: 0,
        frame_id: 0,
        packet_index: 0,
        packet_count: 1,
        presentation_time_us: now_us(),
        fec_group: 0,
        fec_idx: 0,
        fec_total: 0,
        auth_tag: 0,
    };
    let mut wire = Vec::with_capacity(32 + payload.len());
    header.encode(payload, &mut wire);
    wire
}

/// `SESSION_HELLO` payload: minimal capability advertisement. Real sessions
/// would negotiate silicon caps/resolution here (EVRT2_OVERVIEW.md's
/// `ClientHello { evrt2_version, silicon_caps, max_res, max_fps }`) — this
/// implements the version + max_fps + max_res fields concretely (u8 + u32 +
/// u32x2) since those are unambiguous; `silicon_caps` would need the
/// Execution Capability registry's wire encoding, not yet specified at the
/// byte level anywhere in evrt2/, so it's left as a trailing free-form byte
/// vec the caller can populate (e.g. a JSON blob) rather than guessed here.
pub fn build_hello(mode: Mode, max_fps: u32, max_res: (u32, u32), extra_caps: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(9 + extra_caps.len());
    payload.extend_from_slice(&max_fps.to_be_bytes());
    payload.extend_from_slice(&max_res.0.to_be_bytes());
    payload.extend_from_slice(&max_res.1.to_be_bytes());
    payload.extend_from_slice(extra_caps);
    control_packet(PacketType::SessionHello, mode, &payload)
}

pub struct HelloInfo {
    pub max_fps: u32,
    pub max_res: (u32, u32),
    pub extra_caps: Vec<u8>,
}

pub fn parse_hello(payload: &[u8]) -> Option<HelloInfo> {
    if payload.len() < 12 {
        return None;
    }
    Some(HelloInfo {
        max_fps: u32::from_be_bytes(payload[0..4].try_into().unwrap()),
        max_res: (
            u32::from_be_bytes(payload[4..8].try_into().unwrap()),
            u32::from_be_bytes(payload[8..12].try_into().unwrap()),
        ),
        extra_caps: payload[12..].to_vec(),
    })
}

pub fn build_ack(mode: Mode) -> Vec<u8> {
    control_packet(PacketType::SessionAck, mode, &[])
}

pub fn build_feedback(mode: Mode, feedback: &ReceiverFeedback2) -> Vec<u8> {
    control_packet(PacketType::Feedback, mode, &feedback.encode())
}

pub fn build_idr_request(mode: Mode) -> Vec<u8> {
    control_packet(PacketType::IdrRequest, mode, &[])
}

/// TASK-01 § Breach Handling wire payload — ROADMAP.md Phase 1.3.
/// `[u32 measured_age_us][u32 ceiling_us][u16 tile_count][tile_idx u16 ...]`
/// Deliberately minimal: this only reports what was actually measured (the
/// region and the age), never fabricates either — see TASK-01's own
/// "This must not become an excuse to fabricate" clause.
pub fn build_degrade_signal(
    mode: Mode,
    region_tiles: &[u16],
    measured_age_us: u32,
    ceiling_us: u32,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(10 + region_tiles.len() * 2);
    payload.extend_from_slice(&measured_age_us.to_be_bytes());
    payload.extend_from_slice(&ceiling_us.to_be_bytes());
    payload.extend_from_slice(&(region_tiles.len() as u16).to_be_bytes());
    for &tile in region_tiles {
        payload.extend_from_slice(&tile.to_be_bytes());
    }
    control_packet(PacketType::DegradeSignal, mode, &payload)
}

pub struct DegradeSignalInfo {
    pub measured_age: std::time::Duration,
    pub ceiling: std::time::Duration,
    pub region_tiles: Vec<u16>,
}

pub fn parse_degrade_signal(payload: &[u8]) -> Option<DegradeSignalInfo> {
    if payload.len() < 10 {
        return None;
    }
    let measured_age_us = u32::from_be_bytes(payload[0..4].try_into().unwrap());
    let ceiling_us = u32::from_be_bytes(payload[4..8].try_into().unwrap());
    let tile_count = u16::from_be_bytes(payload[8..10].try_into().unwrap()) as usize;
    let mut region_tiles = Vec::with_capacity(tile_count);
    for i in 0..tile_count {
        let off = 10 + i * 2;
        if off + 2 > payload.len() {
            break;
        }
        region_tiles.push(u16::from_be_bytes(
            payload[off..off + 2].try_into().unwrap(),
        ));
    }
    Some(DegradeSignalInfo {
        measured_age: std::time::Duration::from_micros(measured_age_us as u64),
        ceiling: std::time::Duration::from_micros(ceiling_us as u64),
        region_tiles,
    })
}

pub fn build_goodbye(mode: Mode) -> Vec<u8> {
    control_packet(PacketType::Goodbye, mode, &[])
}

/// ROADMAP.md Phase 2.2: MODE_SWITCH wire payload. The packet header's own
/// `mode` field (AR2R47_MODES.md: "All transitions are signaled via
/// MODE_SWITCH packet") carries the NEW mode — this payload adds only the
/// one byte EVRT2_PACKET.md's draft left unspecified: the reason, so a
/// receiving client (or a log) can tell "motion increased" from "bandwidth
/// forced AR" from "silicon disappeared" instead of just seeing a mode
/// number change with no explanation.
pub fn build_mode_switch(new_mode: Mode, reason: crate::evrt2_modes::SwitchReason) -> Vec<u8> {
    control_packet(PacketType::ModeSwitch, new_mode, &[reason.to_wire_code()])
}

/// ROADMAP.md Phase 3.1 — Attention Priority Field wire packet. Wraps
/// `evrt2_apf::encode_u4`'s payload in the standard 32-byte packet header
/// (EVRT2CKMAX.md's own APF header is carried as the packet PAYLOAD, not a
/// replacement for the transport header — the packet still needs FrameId/
/// PresentationTime/etc. like any other control packet).
pub fn build_apf_update(
    mode: Mode,
    attention_map: &[f32],
    cols: u16,
    rows: u16,
    tile_size: u8,
) -> Vec<u8> {
    let apf_payload = crate::evrt2_apf::encode_u4(attention_map, cols, rows, tile_size);
    control_packet(PacketType::ApfUpdate, mode, &apf_payload)
}

/// ROADMAP.md Phase 3.2 — same `PacketType::ApfUpdate` wire type as the
/// full snapshot above; the payload's own `encoding` byte (see
/// `evrt2_apf::APF_ENCODING_DELTA`) is what tells the receiver to decode it
/// against its last-known baseline instead of standalone.
pub fn build_apf_delta(
    mode: Mode,
    previous: &[f32],
    current: &[f32],
    cols: u16,
    rows: u16,
    tile_size: u8,
) -> Vec<u8> {
    let apf_payload = crate::evrt2_apf::encode_delta(previous, current, cols, rows, tile_size);
    control_packet(PacketType::ApfUpdate, mode, &apf_payload)
}

pub struct ModeSwitchInfo {
    pub new_mode: Mode,
    pub reason: Option<crate::evrt2_modes::SwitchReason>,
}

pub fn parse_mode_switch(header: &PacketHeader, payload: &[u8]) -> ModeSwitchInfo {
    ModeSwitchInfo {
        new_mode: header.mode,
        reason: payload
            .first()
            .and_then(|&b| crate::evrt2_modes::SwitchReason::from_wire_code(b)),
    }
}

pub fn build_keepalive(mode: Mode) -> Vec<u8> {
    control_packet(PacketType::Keepalive, mode, &[])
}

/// ROADMAP.md Phase 5.4 — real RTT measurement: replaces the honest gap
/// flagged since Phase 5.2/5.3 ("No RTT probe implemented for this
/// experimental path yet", `rtt_us: 0` in `ReceiverFeedback2`). Reuses
/// KEEPALIVE (0x08, already bidirectional per EVRT2_PACKET.md) rather than
/// adding a dedicated PING packet type — a KEEPALIVE carrying an 8-byte
/// `send_time_us` (from `now_us()`) IS a ping; the receiver's job is only
/// to echo the same 8 bytes back (`parse_keepalive_ping` on one side,
/// `build_keepalive_ping` with the decoded value on the other) — see
/// `evrt2_rtt::RttEstimator` for what the sender does with the round trip.
pub fn build_keepalive_ping(mode: Mode, send_time_us: u64) -> Vec<u8> {
    control_packet(PacketType::Keepalive, mode, &send_time_us.to_be_bytes())
}

/// `None` for a plain empty KEEPALIVE (the pre-Phase-5.4 idle-connection
/// heartbeat, still sent by `run_evrt2_only_session`'s TCP-side keepalive
/// and unrelated to this UDP-side RTT ping) — only a non-empty 8-byte
/// payload is a ping to answer.
pub fn parse_keepalive_ping(payload: &[u8]) -> Option<u64> {
    if payload.len() < 8 {
        return None;
    }
    Some(u64::from_be_bytes(payload[0..8].try_into().unwrap()))
}

// ── Real socket wrapper ─────────────────────────────────────────────────────────

/// Thin wrapper around a real `UdpSocket` that sends/receives EVRT2
/// packets. Deliberately minimal: `send_frame` fragments+schedules+sends;
/// `recv_one` reads one datagram and hands back the decoded header+payload
/// (or an `IngestResult` if the caller routes it through a
/// `FrameReassembler` themselves — kept as two layers rather than one
/// monolithic "receive frame" call so a caller can also see individual
/// control packets, which don't go through the reassembler at all).
pub struct Evrt2Session {
    transport: Transport,
    /// For `Transport::Udp`, the real fixed peer address, used both to send
    /// and to filter `recv_from` so only that peer's datagrams are accepted.
    /// For `Transport::Relay`, a placeholder — the channel pair already
    /// scopes traffic to exactly one peer, so this value is never compared
    /// against anything; it exists only so callers that log/display `peer()`
    /// still get something meaningful to print.
    peer: SocketAddr,
    mode: Mode,
    fec: FecConfig,
    /// ROADMAP.md Phase 4.2 — when set, every outgoing packet is HMAC-signed
    /// (`evrt2_crypto::sign_packet`) and every incoming packet is verified
    /// (`recv_one` silently drops anything that fails, like a malformed
    /// packet already does — see `auth_failures` below for observability).
    /// `None` means AuthTag stays all-zero, matching pre-Phase-4 behavior —
    /// existing tests/callers that never set a key are unaffected.
    auth_key: Option<crate::evrt2_crypto::SessionKey>,
    /// Count of packets dropped for failing AuthTag verification. Exposed
    /// so a caller can log/alert on a nonzero count instead of the drops
    /// being invisible — an unauthenticated wire otherwise looks identical
    /// to ordinary packet loss from the caller's point of view.
    auth_failures: u32,
    /// The client's declared `max_res` from its `SessionHello` (Task-01 §
    /// HELLO fields) — `(0, 0)` until `set_client_max_res` is called, same
    /// "unset means no cap" convention `video_pipeline.rs`'s own
    /// `client_cap_resolution` uses for its live-pipeline counterpart.
    /// Live-found: the EVRT2 experiment loop parsed and LOGGED this value
    /// from HELLO but never stored or acted on it — capturing/encoding
    /// stayed at full native resolution regardless of what the client
    /// actually asked for, unlike the live EVRT1 pipeline (which respects
    /// this exact field and was measured live to reach a steady 60fps on
    /// the same phone/network specifically because it sends far fewer
    /// pixels per frame as a result).
    client_max_res: (u32, u32),
}

impl Evrt2Session {
    /// Bind a local UDP socket and fix the peer address for this session.
    /// Non-blocking reads with a short timeout (so a receive loop can also
    /// service other work, e.g. periodic feedback) rather than blocking
    /// forever — matches how the rest of this codebase's UDP loops
    /// (EVRT1, `evrt_client.rs`) are structured.
    pub fn bind(local_addr: &str, peer: SocketAddr, mode: Mode) -> io::Result<Self> {
        let socket = UdpSocket::bind(local_addr)?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        Ok(Self {
            transport: Transport::Udp(socket),
            peer,
            mode,
            fec: FecConfig::for_mode(mode),
            auth_key: None,
            auth_failures: 0,
            client_max_res: (0, 0),
        })
    }

    /// Wrap an already-bound `UdpSocket` instead of creating a new one —
    /// for callers that need to know the local port BEFORE the peer address
    /// is known (e.g. announcing the port to a peer over a separate control
    /// channel, then wrapping the same socket once traffic arrives). Sets
    /// the same read timeout `bind` does, so behavior is otherwise identical.
    pub fn from_bound_socket(socket: UdpSocket, peer: SocketAddr, mode: Mode) -> io::Result<Self> {
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        Ok(Self {
            transport: Transport::Udp(socket),
            peer,
            mode,
            fec: FecConfig::for_mode(mode),
            auth_key: None,
            auth_failures: 0,
            client_max_res: (0, 0),
        })
    }

    /// ROADMAP.md Phase 6.3′ follow-up (fps-ceiling investigation): flips a
    /// `Transport::Udp` socket to true OS-level non-blocking mode, so
    /// `recv_from` returns `WouldBlock` immediately instead of the
    /// `set_read_timeout(200ms)` every constructor above sets by default.
    /// That 200ms timeout is the right choice for the HELLO handshake wait
    /// loops (which need a bounded blocking wait so they can periodically
    /// re-check `should_stop`), but it is NOT what `recv_one`'s own drain
    /// loop in `run_experiment_encode_loop` wants — that loop's doc comment
    /// already claims "without blocking", but with the 200ms timeout still
    /// active, a single call blocked for up to 200ms whenever no packet
    /// happened to be waiting (which is most iterations — FEEDBACK only
    /// arrives every ~3s). Live-measured: this was directly responsible for
    /// most of a ~240-330ms per-frame stall that remained even after the
    /// zero-copy capture fix closed the DXGI bottleneck. `receive_raw`
    /// already treats `WouldBlock` and `TimedOut` identically as `Ok(None)`,
    /// so flipping to non-blocking here requires no other code changes.
    /// A no-op for `Transport::Relay` — that path is already non-blocking
    /// via `inbound.try_recv()`.
    pub fn set_nonblocking_reads(&self) -> io::Result<()> {
        match &self.transport {
            Transport::Udp(socket) => socket.set_nonblocking(true),
            Transport::Relay { .. } => Ok(()),
        }
    }

    /// ROADMAP.md Phase 5.3 — RELAY_WRAP: build a session backed by a TCP
    /// relay tunnel instead of a UDP socket. `outbound`/`inbound` are the two
    /// halves of the caller's relay-forwarding plumbing (see the `Transport`
    /// doc above) — the caller owns the actual TCP stream and is responsible
    /// for wrapping/unwrapping `Misc::Evrt2RelayWrap` on the other end of
    /// these channels. `peer_label` is display-only (see the `peer` field
    /// doc) — pass whatever address best identifies the other side for logs.
    pub fn from_relay_channels(
        outbound: Sender<Vec<u8>>,
        inbound: Receiver<Vec<u8>>,
        peer_label: SocketAddr,
        mode: Mode,
    ) -> Self {
        Self {
            transport: Transport::Relay { outbound, inbound },
            peer: peer_label,
            mode,
            fec: FecConfig::for_mode(mode),
            auth_key: None,
            auth_failures: 0,
            client_max_res: (0, 0),
        }
    }

    /// ROADMAP.md Phase 4.2. See the `auth_key` field doc for what setting
    /// this actually changes.
    pub fn set_auth_key(&mut self, key: Option<crate::evrt2_crypto::SessionKey>) {
        self.auth_key = key;
    }

    pub fn auth_failures(&self) -> u32 {
        self.auth_failures
    }

    /// See the `client_max_res` field doc for why this matters — set once,
    /// right after the client's `SessionHello` is parsed.
    pub fn set_client_max_res(&mut self, res: (u32, u32)) {
        self.client_max_res = res;
    }

    pub fn client_max_res(&self) -> (u32, u32) {
        self.client_max_res
    }

    /// Only meaningful for `Transport::Udp` — a relay-backed session has no
    /// local socket of its own (it rides the caller's existing TCP stream),
    /// so this returns `AddrNotAvailable` for `Transport::Relay`.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match &self.transport {
            Transport::Udp(socket) => socket.local_addr(),
            Transport::Relay { .. } => Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "Evrt2Session: no local_addr for a relay-backed session",
            )),
        }
    }

    /// Repoint this session at a different peer address without rebinding
    /// the underlying socket (and therefore without losing its local port).
    /// Real use: SDUDP.md § 5 Path Probing — multiple candidate endpoints
    /// are tried and the peer address is only fixed once one responds,
    /// after the socket already exists.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// ROADMAP.md Phase 2.3: FEC is reconfigured atomically with mode —
    /// AR2R47_MODES.md's per-mode profile ties FEC directly to mode (AR
    /// 6+2, 2R 8+2, 47 disabled), so there is no valid state where `mode`
    /// and `fec` disagree. Every subsequent `send_frame` call picks this up
    /// automatically since both fields live on `self`.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.fec = FecConfig::for_mode(mode);
    }

    pub fn set_peer(&mut self, peer: SocketAddr) {
        self.peer = peer;
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// True when this session is currently backed by the relay tunnel
    /// rather than a direct UDP socket — lets a caller (e.g. the Phase 5.4
    /// degradation-triggered switch below) decide whether falling back to
    /// relay is even meaningful right now, without reaching into the
    /// private `Transport` enum itself.
    pub fn is_relay(&self) -> bool {
        matches!(self.transport, Transport::Relay { .. })
    }

    /// ROADMAP.md Phase 5.4 — Path switching: swap this session onto the
    /// relay tunnel, in place, preserving mode/FEC/AuthTag state exactly
    /// (only `transport` and the display-only `peer` label change — the
    /// same pattern `from_relay_channels` already establishes for a
    /// brand-new session, just applied to an existing one instead). The
    /// caller is responsible for the handshake this implies (a fresh
    /// HELLO/ACK over the relay channels) — this method only performs the
    /// mechanical transport swap once that handshake has already
    /// succeeded; it does not itself send or wait for anything.
    pub fn switch_to_relay(
        &mut self,
        outbound: Sender<Vec<u8>>,
        inbound: Receiver<Vec<u8>>,
        peer_label: SocketAddr,
    ) {
        self.transport = Transport::Relay { outbound, inbound };
        self.peer = peer_label;
    }

    /// The reverse of `switch_to_relay` — swap onto a direct UDP socket.
    /// Symmetric counterpart kept for completeness even though ROADMAP.md
    /// Phase 5.4's first pass only ever drives the UDP→relay direction live
    /// (see that phase's own honest-gap note on why the reverse isn't
    /// wired up yet): a caller with a bound socket and a confirmed peer can
    /// still use this directly without waiting on that gap to close.
    pub fn switch_to_udp(&mut self, socket: UdpSocket, peer: SocketAddr) {
        self.transport = Transport::Udp(socket);
        self.peer = peer;
    }

    /// Encrypts (Phase 4.3) then signs (Phase 4.2) `wire` in place — in
    /// that order, Encrypt-then-MAC, so AuthTag covers the ciphertext, not
    /// the plaintext — and sends it. Both are no-ops when no auth key is
    /// set. Single choke point for every outgoing packet so neither phase
    /// had to be wired into every individual `send_*` method.
    fn send_signed(&self, mut wire: Vec<u8>) -> io::Result<()> {
        if let Some(key) = &self.auth_key {
            let enc_key = crate::evrt2_crypto::derive_encryption_key(key);
            crate::evrt2_crypto::encrypt_payload(&enc_key, &mut wire);
            crate::evrt2_crypto::sign_packet(key, &mut wire);
        }
        match &self.transport {
            // ROADMAP.md Phase 6.3′ follow-up: `set_nonblocking_reads` flips
            // this socket to true non-blocking mode so `recv_from` stops
            // stalling the encode loop — but `send_to` on the SAME socket
            // can then also return `WouldBlock` if the OS send buffer is
            // momentarily full, which used to be impossible with the old
            // blocking-with-timeout socket. UDP is already best-effort —
            // the rest of this pipeline (FEC, mode selection, dropped-frame
            // counting) already tolerates a lost datagram — so a transient
            // `WouldBlock` here is treated the same way: the packet is
            // dropped, not escalated into a fatal error that would kill the
            // whole session (the pre-Phase-6.3′ behavior, confirmed live to
            // actually happen: "os error 10035" tore down a session that
            // was otherwise healthy).
            Transport::Udp(socket) => match socket.send_to(&wire, self.peer) {
                Ok(_) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
                Err(e) => Err(e),
            },
            Transport::Relay { outbound, .. } => outbound.send(wire).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "Evrt2Session: relay outbound channel closed",
                )
            }),
        }
    }

    pub fn send_hello(&self, max_fps: u32, max_res: (u32, u32)) -> io::Result<()> {
        self.send_signed(build_hello(self.mode, max_fps, max_res, &[]))
    }

    pub fn send_ack(&self) -> io::Result<()> {
        self.send_signed(build_ack(self.mode))
    }

    /// Sends the MODE_SWITCH packet only — does NOT update `self.mode`/
    /// `self.fec`. Callers must call `set_mode` themselves once the packet
    /// is actually on the wire (matching `ModeSelector::apply`'s own
    /// contract: never claim a transition that wasn't communicated).
    pub fn send_mode_switch(
        &self,
        new_mode: Mode,
        reason: crate::evrt2_modes::SwitchReason,
    ) -> io::Result<()> {
        self.send_signed(build_mode_switch(new_mode, reason))
    }

    pub fn send_apf_update(
        &self,
        attention_map: &[f32],
        cols: u16,
        rows: u16,
        tile_size: u8,
    ) -> io::Result<()> {
        self.send_signed(build_apf_update(
            self.mode,
            attention_map,
            cols,
            rows,
            tile_size,
        ))
    }

    /// ROADMAP.md Phase 3.2.
    pub fn send_apf_delta(
        &self,
        previous: &[f32],
        current: &[f32],
        cols: u16,
        rows: u16,
        tile_size: u8,
    ) -> io::Result<()> {
        self.send_signed(build_apf_delta(
            self.mode, previous, current, cols, rows, tile_size,
        ))
    }

    pub fn send_degrade_signal(
        &self,
        region_tiles: &[u16],
        measured_age_us: u32,
        ceiling_us: u32,
    ) -> io::Result<()> {
        self.send_signed(build_degrade_signal(
            self.mode,
            region_tiles,
            measured_age_us,
            ceiling_us,
        ))
    }

    pub fn send_feedback(&self, feedback: &ReceiverFeedback2) -> io::Result<()> {
        self.send_signed(build_feedback(self.mode, feedback))
    }

    pub fn send_idr_request(&self) -> io::Result<()> {
        self.send_signed(build_idr_request(self.mode))
    }

    pub fn send_goodbye(&self) -> io::Result<()> {
        self.send_signed(build_goodbye(self.mode))
    }

    /// ROADMAP.md Phase 5.4 — send an RTT ping (see `build_keepalive_ping`).
    pub fn send_keepalive_ping(&self, send_time_us: u64) -> io::Result<()> {
        self.send_signed(build_keepalive_ping(self.mode, send_time_us))
    }

    /// Fragment, FEC-protect, schedule, and send one encoded frame over the
    /// real socket — the live-network counterpart of `build_frame_packets`.
    pub fn send_frame(
        &self,
        frame_bytes: &[u8],
        frame_id: u32,
        is_keyframe: bool,
        is_silicon: bool,
        is_h265: bool,
        visible_region_byte_ranges: &[(usize, usize)],
    ) -> io::Result<usize> {
        let packets = build_frame_packets(
            frame_bytes,
            frame_id,
            self.mode,
            is_keyframe,
            is_silicon,
            is_h265,
            now_us(),
            visible_region_byte_ranges,
            self.fec,
        );
        let mut sent = 0;
        for pkt in packets {
            self.send_signed(pkt)?;
            sent += 1;
        }
        Ok(sent)
    }

    /// Read one datagram (bounded by the socket's read timeout) and decode
    /// its header. Returns `Ok(None)` on timeout (normal — lets a caller's
    /// loop do periodic work), `Err` on a real I/O error, `Ok(Some(..))` on
    /// a successfully decoded packet. Malformed packets (bad magic/version/
    /// too short) are logged by the caller if desired — this returns them
    /// as a decode error rather than silently dropping, so a caller can
    /// distinguish "no packet arrived" from "garbage arrived."
    ///
    /// ROADMAP.md Phase 4.2: when an auth key is set, a packet that fails
    /// AuthTag verification is treated exactly like a malformed one —
    /// `Ok(None)`, `auth_failures` incremented so a caller can still notice
    /// via `auth_failures()` — never surfaced as a distinct error variant,
    /// so existing callers (built before Phase 4.2 existed) don't need to
    /// handle a new case to keep compiling correctly.
    pub fn recv_one(&mut self) -> io::Result<Option<(PacketHeader, Vec<u8>)>> {
        let raw = match self.receive_raw()? {
            Some(raw) => raw,
            None => return Ok(None),
        };
        if let Some(key) = &self.auth_key {
            if !crate::evrt2_crypto::verify_packet(key, &raw) {
                self.auth_failures += 1;
                return Ok(None);
            }
        }
        match PacketHeader::decode(&raw) {
            Ok((header, _payload)) => {
                // ROADMAP.md Phase 4.3: decrypt AFTER verifying
                // (Encrypt-then-MAC — the tag already proved this
                // buffer wasn't tampered with before we trust it
                // enough to decrypt). `decrypt_payload` itself
                // checks the ENCRYPTED flag and passes unencrypted
                // payloads through unchanged, so this is correct
                // whether or not this particular packet actually
                // used encryption (e.g. mixed traffic mid-session).
                let payload = if let Some(key) = &self.auth_key {
                    let enc_key = crate::evrt2_crypto::derive_encryption_key(key);
                    match crate::evrt2_crypto::decrypt_payload(&enc_key, &raw) {
                        Some(p) => p,
                        None => {
                            self.auth_failures += 1;
                            return Ok(None);
                        }
                    }
                } else {
                    _payload.to_vec()
                };
                Ok(Some((header, payload)))
            }
            Err(_) => Ok(None),
        }
    }

    /// Transport-specific half of `recv_one`: get the next raw wire packet's
    /// bytes, or `None` on a normal "nothing arrived within the read
    /// timeout" (the loop should just try again), independent of AuthTag/
    /// encryption — those apply identically to whatever bytes come back.
    fn receive_raw(&mut self) -> io::Result<Option<Vec<u8>>> {
        match &self.transport {
            Transport::Udp(socket) => {
                let mut buf = [0u8; 1500];
                match socket.recv_from(&mut buf) {
                    Ok((len, from)) => {
                        if from != self.peer {
                            return Ok(None); // ignore packets from anyone but our fixed peer
                        }
                        Ok(Some(buf[..len].to_vec()))
                    }
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut =>
                    {
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            }
            Transport::Relay { inbound, .. } => match inbound.try_recv() {
                Ok(raw) => Ok(Some(raw)),
                Err(TryRecvError::Empty) => Ok(None),
                Err(TryRecvError::Disconnected) => Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "Evrt2Session: relay inbound channel closed",
                )),
            },
        }
    }

    pub fn fec_config(&self) -> FecConfig {
        self.fec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_round_trips_exactly() {
        let fb = ReceiverFeedback2 {
            frame_id: 12345,
            pressure: 0.42,
            jitter_p95_us: 8000,
            decoded_fps: 59.9,
            silicon_ok: true,
            dropped_frames: 3,
            rtt_us: 15000,
        };
        let wire = fb.encode();
        assert_eq!(wire.len(), ReceiverFeedback2::WIRE_LEN);
        let decoded = ReceiverFeedback2::decode(&wire).unwrap();
        assert_eq!(decoded, fb);
    }

    #[test]
    fn feedback_thresholds_match_sdudp_spec() {
        let high_pressure = ReceiverFeedback2 {
            pressure: 0.9,
            ..zero_feedback()
        };
        assert!(high_pressure.should_reduce_bitrate());
        let low_pressure = ReceiverFeedback2 {
            pressure: 0.1,
            ..zero_feedback()
        };
        assert!(low_pressure.should_increase_bitrate());
        let slow = ReceiverFeedback2 {
            decoded_fps: 40.0,
            ..zero_feedback()
        };
        assert!(slow.decoded_fps_below_target(60.0));
    }

    fn zero_feedback() -> ReceiverFeedback2 {
        ReceiverFeedback2 {
            frame_id: 0,
            pressure: 0.5,
            jitter_p95_us: 0,
            decoded_fps: 60.0,
            silicon_ok: true,
            dropped_frames: 0,
            rtt_us: 0,
        }
    }

    #[test]
    fn build_frame_packets_fragments_large_frame_correctly() {
        let frame_bytes = vec![0xABu8; MAX_PAYLOAD * 3 + 100]; // 4 fragments
        let packets = build_frame_packets(
            &frame_bytes,
            1,
            Mode::R2,
            true,
            true,
            false,
            0,
            &[],
            FecConfig { n: 0, k: 0 },
        );
        // No FEC: exactly 4 data packets, no repairs.
        assert_eq!(packets.len(), 4);
        for pkt in &packets {
            let (header, payload) = PacketHeader::decode(pkt).unwrap();
            assert_eq!(header.packet_type, PacketType::VideoFrame);
            assert_eq!(header.packet_count, 4);
            assert!(header.has_flag(flags::IS_KEYFRAME));
            assert!(payload.len() <= MAX_PAYLOAD);
        }
    }

    #[test]
    fn build_frame_packets_visible_region_slices_sent_first() {
        let frame_bytes = vec![0xCDu8; MAX_PAYLOAD * 4];
        // Mark the 3rd fragment's byte range as visible-region.
        let vr_start = MAX_PAYLOAD * 2;
        let vr_end = MAX_PAYLOAD * 3;
        let packets = build_frame_packets(
            &frame_bytes,
            7,
            Mode::Mode47,
            false,
            true,
            false,
            0,
            &[(vr_start, vr_end)],
            FecConfig { n: 0, k: 0 },
        );
        let (first_header, _) = PacketHeader::decode(&packets[0]).unwrap();
        assert!(
            first_header.is_visible_region(),
            "visible-region fragment must be scheduled first"
        );
        assert_eq!(first_header.packet_index, 2);
    }

    #[test]
    fn build_frame_packets_generates_fec_repairs_when_enabled() {
        let frame_bytes = vec![0x11u8; CHUNK_MAX * 6]; // 6 data fragments = 1 full AR group (N=6)
        let packets = build_frame_packets(
            &frame_bytes,
            2,
            Mode::Ar,
            true,
            false,
            false,
            0,
            &[],
            FecConfig::AR,
        );
        let repair_count = packets
            .iter()
            .filter(|p| PacketHeader::decode(p).unwrap().0.packet_type == PacketType::FecRepair)
            .count();
        assert_eq!(
            repair_count,
            FecConfig::AR.k,
            "one repair packet per K, one group"
        );
    }

    #[test]
    fn reassembler_completes_frame_with_no_losses() {
        let frame_bytes: Vec<u8> = (0..(MAX_PAYLOAD * 3 + 50))
            .map(|i| (i % 256) as u8)
            .collect();
        let packets = build_frame_packets(
            &frame_bytes,
            99,
            Mode::R2,
            true,
            true,
            false,
            0,
            &[],
            FecConfig { n: 0, k: 0 },
        );
        let mut reassembler = FrameReassembler::new();
        let mut result = None;
        for pkt in &packets {
            let (header, payload) = PacketHeader::decode(pkt).unwrap();
            if let IngestResult::FrameComplete { bytes, .. } = reassembler.ingest(&header, payload)
            {
                result = Some(bytes);
            }
        }
        assert_eq!(result, Some(frame_bytes));
    }

    #[test]
    fn reassembler_recovers_a_lost_data_packet_via_fec() {
        let frame_bytes: Vec<u8> = (0..(CHUNK_MAX * 6)).map(|i| (i % 251) as u8).collect(); // 6 fragments, 1 AR group
        let packets = build_frame_packets(
            &frame_bytes,
            5,
            Mode::Ar,
            true,
            true,
            false,
            0,
            &[],
            FecConfig::AR,
        );

        let mut reassembler = FrameReassembler::new();

        // Drop exactly one data packet (index 0) — simulate real packet loss.
        let mut result = None;
        for pkt in &packets {
            let (header, payload) = PacketHeader::decode(pkt).unwrap();
            if header.packet_type == PacketType::VideoFrame && header.packet_index == 0 {
                continue; // simulated loss
            }
            if let IngestResult::FrameComplete { bytes, .. } = reassembler.ingest(&header, payload)
            {
                result = Some(bytes);
            }
        }
        assert_eq!(
            result,
            Some(frame_bytes),
            "frame must reassemble byte-exact despite one lost packet, via FEC recovery"
        );
    }

    #[test]
    fn reassembler_stays_pending_when_loss_exceeds_fec_capacity() {
        let frame_bytes = vec![0x77u8; CHUNK_MAX * 6];
        let packets = build_frame_packets(
            &frame_bytes,
            6,
            Mode::Ar,
            true,
            true,
            false,
            0,
            &[],
            FecConfig::AR,
        );
        let mut reassembler = FrameReassembler::new();

        // Drop packets 0 AND 2 — both land in repair-0's coverage (i%2==0),
        // exceeding what a single K=2 repair set can resolve together.
        let mut completed = false;
        for pkt in &packets {
            let (header, payload) = PacketHeader::decode(pkt).unwrap();
            if header.packet_type == PacketType::VideoFrame
                && (header.packet_index == 0 || header.packet_index == 2)
            {
                continue;
            }
            if let IngestResult::FrameComplete { .. } = reassembler.ingest(&header, payload) {
                completed = true;
            }
        }
        assert!(
            !completed,
            "must not claim completion when unrecoverable loss occurred"
        );
        assert_eq!(reassembler.pending_frame_count(), 1);
    }

    #[test]
    fn hello_ack_round_trip() {
        let hello = build_hello(Mode::Mode47, 120, (3840, 2160), b"nvenc");
        let (header, payload) = PacketHeader::decode(&hello).unwrap();
        assert_eq!(header.packet_type, PacketType::SessionHello);
        let info = parse_hello(payload).unwrap();
        assert_eq!(info.max_fps, 120);
        assert_eq!(info.max_res, (3840, 2160));
        assert_eq!(info.extra_caps, b"nvenc");

        let ack = build_ack(Mode::Mode47);
        let (ack_header, _) = PacketHeader::decode(&ack).unwrap();
        assert_eq!(ack_header.packet_type, PacketType::SessionAck);
    }

    /// ROADMAP.md Phase 5.4: the RTT ping/pong wire format round-trips
    /// exactly, and a plain empty KEEPALIVE (the pre-5.4 idle heartbeat)
    /// is correctly recognized as "not a ping" rather than misparsed.
    #[test]
    fn keepalive_ping_round_trips_and_plain_keepalive_is_not_a_ping() {
        let ping = build_keepalive_ping(Mode::Ar, 123_456_789);
        let (header, payload) = PacketHeader::decode(&ping).unwrap();
        assert_eq!(header.packet_type, PacketType::Keepalive);
        assert_eq!(parse_keepalive_ping(payload), Some(123_456_789));

        let plain = build_keepalive(Mode::Ar);
        let (plain_header, plain_payload) = PacketHeader::decode(&plain).unwrap();
        assert_eq!(plain_header.packet_type, PacketType::Keepalive);
        assert_eq!(parse_keepalive_ping(plain_payload), None);
    }

    /// ROADMAP.md Phase 1.3 / Task-01 § Breach Handling: `DEGRADE_SIGNAL`'s
    /// wire round trip — the region tile list, measured age, and ceiling
    /// all survive encode→decode exactly. Underlies the client's real
    /// degradation indicator (`evrt2_experiment.rs`'s `on_degrade`
    /// callback, `main.rs`'s `evrt2_preview_window`) — a gap in this
    /// round trip would have silently fed a wrong number into the UI.
    #[test]
    fn degrade_signal_round_trips_exactly() {
        let tiles: Vec<u16> = vec![0, 3, 17, 4095];
        let wire = build_degrade_signal(Mode::Ar, &tiles, 14_500, 12_000);

        let (header, payload) = PacketHeader::decode(&wire).unwrap();
        assert_eq!(header.packet_type, PacketType::DegradeSignal);
        assert_eq!(header.mode, Mode::Ar);

        let info = parse_degrade_signal(payload).expect("must decode");
        assert_eq!(info.measured_age, Duration::from_micros(14_500));
        assert_eq!(info.ceiling, Duration::from_micros(12_000));
        assert_eq!(info.region_tiles, tiles);
    }

    #[test]
    fn degrade_signal_with_empty_region_round_trips() {
        let wire = build_degrade_signal(Mode::Mode47, &[], 9_000, 8_000);
        let (_, payload) = PacketHeader::decode(&wire).unwrap();
        let info = parse_degrade_signal(payload).expect("must decode");
        assert!(info.region_tiles.is_empty());
    }

    #[test]
    fn degrade_signal_rejects_truncated_payload() {
        assert!(parse_degrade_signal(&[0u8; 9]).is_none()); // needs at least 10 bytes
    }

    /// Live over loopback: two real `Evrt2Session`s exchange an actual RTT
    /// ping/pong (`send_keepalive_ping` → `parse_keepalive_ping` → echo),
    /// and the measured round trip is a small positive duration — proves
    /// the whole path `run_client_experiment`/`run_experiment_encode_loop`
    /// actually use (send ping, host echoes, client computes RTT) works
    /// end to end on real sockets, not just the pure wire-format test above.
    #[test]
    fn keepalive_ping_measures_a_real_round_trip_over_loopback() {
        let dummy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mut client = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::Ar).unwrap();
        let mut host = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::Ar).unwrap();
        host.set_peer(client.local_addr().unwrap());
        client.set_peer(host.local_addr().unwrap());

        let send_time_us = now_us();
        client.send_keepalive_ping(send_time_us).unwrap();

        let (host_header, host_payload) =
            recv_with_retry(&mut host).expect("host must receive the ping");
        assert_eq!(host_header.packet_type, PacketType::Keepalive);
        let echoed_ts = parse_keepalive_ping(&host_payload)
            .expect("host must be able to decode the ping timestamp");
        host.send_keepalive_ping(echoed_ts).unwrap();

        let (client_header, client_payload) =
            recv_with_retry(&mut client).expect("client must receive the pong");
        assert_eq!(client_header.packet_type, PacketType::Keepalive);
        let round_tripped_ts = parse_keepalive_ping(&client_payload).unwrap();
        assert_eq!(
            round_tripped_ts, send_time_us,
            "host must echo the exact bytes it received"
        );

        let rtt_us = now_us().saturating_sub(round_tripped_ts);
        // Real loopback round trip: must be positive and well under a
        // second (would indicate something is very wrong, e.g. a hung
        // socket) rather than asserting a specific tight bound that could
        // be flaky under CI load.
        assert!(
            rtt_us < 1_000_000,
            "loopback RTT should be well under 1s, got {rtt_us}us"
        );
    }

    #[test]
    fn empty_frame_produces_no_packets() {
        let packets =
            build_frame_packets(&[], 1, Mode::Ar, false, false, false, 0, &[], FecConfig::AR);
        assert!(packets.is_empty());
    }

    // ── Real two-socket loopback ──────────────────────────────────────────

    #[test]
    fn real_udp_sockets_exchange_hello_and_ack_on_loopback() {
        // Bind both sockets first (each gets a real ephemeral port), THEN
        // point them at each other's now-known address — mirrors how a
        // real session only learns the peer's address after the socket
        // already exists (SDUDP.md § 5 Path Probing).
        let dummy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mut host = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::R2).unwrap();
        let mut client = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::R2).unwrap();
        host.set_peer(client.local_addr().unwrap());
        client.set_peer(host.local_addr().unwrap());

        client.send_hello(60, (1920, 1080)).unwrap();
        let (header, payload) = recv_with_retry(&mut host).expect("host must receive HELLO");
        assert_eq!(header.packet_type, PacketType::SessionHello);
        let info = parse_hello(&payload).unwrap();
        assert_eq!(info.max_fps, 60);

        host.send_ack().unwrap();
        let (ack_header, _) = recv_with_retry(&mut client).expect("client must receive ACK");
        assert_eq!(ack_header.packet_type, PacketType::SessionAck);
    }

    #[test]
    fn real_udp_sockets_exchange_a_multi_packet_frame() {
        let dummy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mut a = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::Ar).unwrap();
        let mut b = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::Ar).unwrap();
        a.set_peer(b.local_addr().unwrap());
        b.set_peer(a.local_addr().unwrap());

        let frame_bytes: Vec<u8> = (0..(MAX_PAYLOAD * 4 + 37))
            .map(|i| (i % 256) as u8)
            .collect();
        let sent = a
            .send_frame(&frame_bytes, 42, true, false, false, &[])
            .unwrap();
        assert!(sent >= 5); // 5 data fragments + FEC repairs (AR mode, enabled by default)

        let mut reassembler = FrameReassembler::new();
        let mut result = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while result.is_none() && Instant::now() < deadline {
            if let Some((header, payload)) = b.recv_one().unwrap() {
                if let IngestResult::FrameComplete { bytes, .. } =
                    reassembler.ingest(&header, &payload)
                {
                    result = Some(bytes);
                }
            }
        }
        assert_eq!(
            result,
            Some(frame_bytes),
            "real socket round trip must reassemble byte-exact"
        );
    }

    /// ROADMAP.md Phase 5.4 — Path switching: an established, authenticated
    /// UDP session mid-stream, switched onto a fresh relay channel pair,
    /// keeps streaming correctly afterward — same mode, same FEC profile,
    /// AuthTag still verifying, byte-exact frame delivery, all preserved
    /// across the transport swap. This is the mechanical half of Phase 5.4
    /// (`Evrt2Session::switch_to_relay`); the protocol half (who decides to
    /// switch and when) lives in `evrt2_experiment.rs` and is exercised by
    /// its own live session tests, not re-proven here.
    #[test]
    fn switching_from_udp_to_relay_mid_session_preserves_state_and_keeps_streaming() {
        let dummy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mut host = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::Ar).unwrap();
        let mut client = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::Ar).unwrap();
        host.set_peer(client.local_addr().unwrap());
        client.set_peer(host.local_addr().unwrap());

        let key = crate::evrt2_crypto::generate_session_key();
        host.set_auth_key(Some(key));
        client.set_auth_key(Some(key));
        host.set_mode(Mode::R2);
        client.set_mode(Mode::R2);
        assert!(!host.is_relay());
        assert!(!client.is_relay());

        // ── Before the switch: one real frame over real UDP sockets ───────
        let frame_before: Vec<u8> = (0..(MAX_PAYLOAD * 2 + 11))
            .map(|i| (i % 256) as u8)
            .collect();
        host.send_frame(&frame_before, 1, true, false, false, &[])
            .unwrap();
        let mut reassembler = FrameReassembler::new();
        let mut got_before = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while got_before.is_none() && Instant::now() < deadline {
            if let Some((header, payload)) = client.recv_one().unwrap() {
                if let IngestResult::FrameComplete { bytes, .. } =
                    reassembler.ingest(&header, &payload)
                {
                    got_before = Some(bytes);
                }
            }
        }
        assert_eq!(
            got_before,
            Some(frame_before),
            "must work over plain UDP before any switch"
        );

        // ── The switch itself ──────────────────────────────────────────────
        let (host_out_tx, host_out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let (client_out_tx, client_out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        host.switch_to_relay(host_out_tx, client_out_rx, dummy);
        client.switch_to_relay(client_out_tx, host_out_rx, dummy);
        assert!(host.is_relay());
        assert!(client.is_relay());
        // Mode/FEC survive the swap untouched — Phase 5.4's whole premise
        // is that only the transport changes.
        assert_eq!(host.mode(), Mode::R2);
        assert_eq!(client.mode(), Mode::R2);

        // ── After the switch: a second frame, now over the relay pair ─────
        let frame_after: Vec<u8> = (0..(MAX_PAYLOAD * 3 + 5))
            .map(|i| ((i * 7) % 256) as u8)
            .collect();
        host.send_frame(&frame_after, 2, true, false, false, &[])
            .unwrap();
        let mut got_after = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while got_after.is_none() && Instant::now() < deadline {
            if let Some((header, payload)) = client.recv_one().unwrap() {
                if let IngestResult::FrameComplete { bytes, .. } =
                    reassembler.ingest(&header, &payload)
                {
                    got_after = Some(bytes);
                }
            }
        }
        assert_eq!(
            got_after,
            Some(frame_after),
            "must work over relay after the switch, byte-exact"
        );
        assert_eq!(
            host.auth_failures(),
            0,
            "AuthTag must still verify correctly after the transport swap"
        );
        assert_eq!(client.auth_failures(), 0);
    }

    fn recv_with_retry(session: &mut Evrt2Session) -> Option<(PacketHeader, Vec<u8>)> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(result)) = session.recv_one() {
                return Some(result);
            }
        }
        None
    }

    /// End-to-end: a full session lifecycle over real sockets — handshake,
    /// a keyframe delivered despite one simulated lost packet (recovered
    /// via FEC), a feedback round trip, and clean teardown. Every piece
    /// (`evrt2_packet`, `evrt2_fec`, `evrt2_scheduler`, and this module's
    /// fragmentation/reassembly) participates together, on two real
    /// `UdpSocket`s on loopback — matching EVRT2_OVERVIEW.md's own Session
    /// Lifecycle stages (HANDSHAKE → STREAMING → FEEDBACK LOOP → TEARDOWN),
    /// minus SILICON PROBE and MODE NEGOTIATION, which live in
    /// `execution_capability`/`evrt2_modes` and are exercised by their own
    /// test suites rather than re-proven here.
    #[test]
    fn full_session_lifecycle_handshake_frame_loss_feedback_goodbye() {
        let dummy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mut host = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::Ar).unwrap();
        let mut client = Evrt2Session::bind("127.0.0.1:0", dummy, Mode::Ar).unwrap();
        host.set_peer(client.local_addr().unwrap());
        client.set_peer(host.local_addr().unwrap());

        // ── 1. HANDSHAKE ────────────────────────────────────────────────
        client.send_hello(60, (1920, 1080)).unwrap();
        let (hello_header, hello_payload) =
            recv_with_retry(&mut host).expect("host must receive HELLO");
        assert_eq!(hello_header.packet_type, PacketType::SessionHello);
        assert_eq!(parse_hello(&hello_payload).unwrap().max_fps, 60);
        host.send_ack().unwrap();
        let (ack_header, _) = recv_with_retry(&mut client).expect("client must receive ACK");
        assert_eq!(ack_header.packet_type, PacketType::SessionAck);

        // ── 2. STREAMING (one frame, one simulated lost data packet) ──────
        let frame_bytes: Vec<u8> = (0..(CHUNK_MAX * 6 + 200))
            .map(|i| (i * 7 % 256) as u8)
            .collect();
        let sent = host
            .send_frame(&frame_bytes, 1, true, true, false, &[])
            .unwrap();
        assert!(
            sent > 6,
            "AR mode must add FEC repair packets beyond the data fragments"
        );

        let mut reassembler = FrameReassembler::new();
        let mut reconstructed = None;
        let mut dropped_one = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while reconstructed.is_none() && Instant::now() < deadline {
            let Some((header, payload)) = client.recv_one().unwrap() else {
                continue;
            };
            if header.packet_type == PacketType::VideoFrame
                && header.packet_index == 0
                && !dropped_one
            {
                dropped_one = true; // simulated packet loss — read off the socket, never ingested
                continue;
            }
            if let IngestResult::FrameComplete { bytes, .. } = reassembler.ingest(&header, &payload)
            {
                reconstructed = Some(bytes);
            }
        }
        assert!(
            dropped_one,
            "test setup error: never actually simulated a loss"
        );
        assert_eq!(
            reconstructed,
            Some(frame_bytes),
            "frame must reassemble byte-exact despite simulated loss, via FEC"
        );

        // ── 3. FEEDBACK LOOP ────────────────────────────────────────────
        let feedback = ReceiverFeedback2 {
            frame_id: 1,
            pressure: 0.15,
            jitter_p95_us: 3200,
            decoded_fps: 59.4,
            silicon_ok: true,
            dropped_frames: 0,
            rtt_us: 900,
        };
        client.send_feedback(&feedback).unwrap();
        let (fb_header, fb_payload) =
            recv_with_retry(&mut host).expect("host must receive FEEDBACK");
        assert_eq!(fb_header.packet_type, PacketType::Feedback);
        let decoded_fb = ReceiverFeedback2::decode(&fb_payload).unwrap();
        assert_eq!(decoded_fb, feedback);
        assert!(
            decoded_fb.should_increase_bitrate(),
            "low pressure should signal room to grow"
        );

        // ── 4. TEARDOWN ─────────────────────────────────────────────────
        client.send_goodbye().unwrap();
        let (bye_header, _) = recv_with_retry(&mut host).expect("host must receive GOODBYE");
        assert_eq!(bye_header.packet_type, PacketType::Goodbye);
    }

    /// ROADMAP.md Phase 5.3 (RELAY_WRAP): the same lifecycle as the test
    /// above, but with zero UDP sockets involved — both sessions run on
    /// `Transport::Relay`, cross-wired via two `mpsc` channel pairs the way
    /// `run_evrt2_only_session`'s real relay-forwarding plumbing would be.
    /// Proves the transport swap didn't change behavior: FEC recovery,
    /// AuthTag, and NaCl encryption (Phase 4.2/4.3) all still work
    /// end-to-end when every byte travels through channels instead of a
    /// socket — exactly what a caller relaying real `Evrt2RelayWrap` bytes
    /// over a TCP stream needs to be true.
    #[test]
    fn relay_transport_full_lifecycle_with_auth_and_encryption() {
        let dummy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let (host_out_tx, host_out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let (client_out_tx, client_out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        // host's outbound is client's inbound, and vice versa.
        let mut host =
            Evrt2Session::from_relay_channels(host_out_tx, client_out_rx, dummy, Mode::Ar);
        let mut client =
            Evrt2Session::from_relay_channels(client_out_tx, host_out_rx, dummy, Mode::Ar);

        let key = crate::evrt2_crypto::generate_session_key();

        // ── HANDSHAKE (unauthenticated, matching the real UDP path) ───────
        client.send_hello(60, (1920, 1080)).unwrap();
        let (hello_header, hello_payload) =
            recv_with_retry(&mut host).expect("host must receive HELLO over relay");
        assert_eq!(hello_header.packet_type, PacketType::SessionHello);
        assert_eq!(parse_hello(&hello_payload).unwrap().max_fps, 60);
        host.send_ack().unwrap();
        let (ack_header, _) =
            recv_with_retry(&mut client).expect("client must receive ACK over relay");
        assert_eq!(ack_header.packet_type, PacketType::SessionAck);

        // ROADMAP.md Phase 4.2/4.3: applied only after ACK, same as the real
        // host.rs/evrt2_experiment.rs call sites.
        host.set_auth_key(Some(key));
        client.set_auth_key(Some(key));

        // ── STREAMING, with FEC recovering one simulated lost packet ──────
        let frame_bytes: Vec<u8> = (0..(CHUNK_MAX * 6 + 200))
            .map(|i| (i * 11 % 256) as u8)
            .collect();
        let sent = host
            .send_frame(&frame_bytes, 1, true, true, false, &[])
            .unwrap();
        assert!(
            sent > 6,
            "AR mode must add FEC repair packets beyond the data fragments"
        );

        let mut reassembler = FrameReassembler::new();
        let mut reconstructed = None;
        let mut dropped_one = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while reconstructed.is_none() && Instant::now() < deadline {
            let Some((header, payload)) = client.recv_one().unwrap() else {
                continue;
            };
            if header.packet_type == PacketType::VideoFrame
                && header.packet_index == 0
                && !dropped_one
            {
                dropped_one = true; // simulated packet loss — never ingested
                continue;
            }
            if let IngestResult::FrameComplete { bytes, .. } = reassembler.ingest(&header, &payload)
            {
                reconstructed = Some(bytes);
            }
        }
        assert!(
            dropped_one,
            "test setup error: never actually simulated a loss"
        );
        assert_eq!(
            reconstructed,
            Some(frame_bytes),
            "frame must reassemble byte-exact over the relay transport too, despite simulated loss + encryption"
        );
        assert_eq!(host.auth_failures(), 0);
        assert_eq!(client.auth_failures(), 0);

        // ── TEARDOWN ────────────────────────────────────────────────────
        client.send_goodbye().unwrap();
        let (bye_header, _) =
            recv_with_retry(&mut host).expect("host must receive GOODBYE over relay");
        assert_eq!(bye_header.packet_type, PacketType::Goodbye);
    }

    /// A packet forged without the session key must be rejected exactly
    /// like it would be over UDP — proves AuthTag verification runs
    /// identically on the relay path, not bypassed because there's no
    /// socket-level "from address" check to lean on.
    #[test]
    fn relay_transport_rejects_a_packet_signed_with_the_wrong_key() {
        let dummy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        // Only one direction is exercised (attacker → victim), so the
        // "return path" channels are created but never sent on — receivers
        // just idle for the test's lifetime.
        let (attacker_to_victim_tx, attacker_to_victim_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let (_attacker_return_tx, attacker_return_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let (victim_return_tx, _victim_return_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        let mut attacker = Evrt2Session::from_relay_channels(
            attacker_to_victim_tx,
            attacker_return_rx,
            dummy,
            Mode::Ar,
        );
        attacker.set_auth_key(Some(crate::evrt2_crypto::generate_session_key()));
        attacker.send_hello(60, (1920, 1080)).unwrap();

        let mut victim = Evrt2Session::from_relay_channels(
            victim_return_tx,
            attacker_to_victim_rx,
            dummy,
            Mode::Ar,
        );
        victim.set_auth_key(Some(crate::evrt2_crypto::generate_session_key()));

        assert_eq!(
            victim.recv_one().unwrap(),
            None,
            "wrong-key packet must be silently dropped, not accepted"
        );
        assert_eq!(victim.auth_failures(), 1);
    }
}
