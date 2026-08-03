// =============================================================================
// EVRT2 — Experimental live host stream (game-screen "EVRT2" button)
// =============================================================================
//
//! A fully independent capture → EVRTCK-encode → `evrt2_session` → UDP loop,
//! triggered by the experimental "EVRT2" button on the game screen (both PC
//! and Android clients). This is deliberately **not** wired into
//! `video_pipeline.rs`'s live `encode_loop` — it opens its own UDP socket,
//! runs its own capture+encode cycle, and cannot affect the existing
//! EVRT1/EVRTCK game-mode pipeline in any way. That's the point: the user
//! asked for a way to experiment with EVRT2 without risking the pipeline
//! that's already been tested live (phone ↔ PC, "работает как мёд").
//!
//! Scope for this experimental slice: single client, no encryption.
//! ROADMAP.md Phase 2 wired `evrt2_modes::ModeSelector` in for real — the
//! session starts in AR and moves to 2R on measured motion (see
//! `run_host_experiment`'s `ModeSignals`), governed by the actual AR2R47
//! state machine rather than a hardcoded mode. Good enough to prove the
//! wire format, FEC, scheduler, mode transitions, and session lifecycle
//! work against a real capture source end-to-end — not a finished feature.
//!
//! ROADMAP.md Phase 6.1: this file now ALSO opens a real NVENC session,
//! encodes every captured frame with it, and feeds both EVRTCK's and
//! NVENC's real measured per-frame costs into a `CapabilityRegistry` so
//! `schedule()` makes a genuine marginal-utility decision between them —
//! closing the "no hardware-encoder Execution Capability provider" gap
//! this doc used to describe. When NVENC wins that decision, its real
//! H264 bytes are what actually go out (`is_silicon=true`, no
//! VISIBLE_REGION marking — NVENC's monolithic bitstream can't be
//! reordered by region the way EVRTCK's independent tiles can, an honest
//! Task-01 gap for the silicon path specifically). The client decodes
//! those with `openh264` (the same software decoder the live EVRT1 client
//! already uses on both PC and Android — reused here rather than routing
//! through Android's Surface-only MediaCodec path, which has no Rust-side
//! pixel buffer to hand back at all). This does mean a second NVENC
//! session runs concurrently with whatever the live EVRT1 pipeline might
//! also be using — the module doc used to avoid this on purpose; whether
//! concurrent NVENC sessions are safe on a given GPU/driver has not been
//! verified by a live test yet, see ROADMAP.md 6.1.

use crate::evrt2_attention::{visible_region_from_map, AttentionMapBuilder};
use crate::evrt2_jitter::{JitterEstimator, ModeProfile};
use crate::evrt2_modes::{ModeSelector, ModeSignals};
use crate::evrt2_packet::Mode;
use crate::evrt2_scheduler::{check_breach, visible_threshold};
use crate::evrt2_session::{
    build_hello, parse_degrade_signal, parse_hello, Evrt2Session, FrameReassembler, IngestResult,
    ReceiverFeedback2,
};
use crate::evrtck::EvrtckEncoder;
use crate::execution_capability::{
    Capability, CapabilityRegistry, Provider, PROVIDER_CPU_EVRTCK, PROVIDER_NVENC_H264,
};
use crate::host::HostEvent;
use crate::nvenc::{NvencCodec, NvencEncoder};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Target frame rate for the experimental stream. ROADMAP.md Phase 1.1:
/// raised from the original 20fps proof-of-life target once the protocol
/// itself (FEC, reassembly, session lifecycle) was confirmed live — still
/// well short of Mode47's 120fps target (that needs the silicon encoder
/// path from Phase 6, not the CPU EVRTCK encoder used here), but enough to
/// judge whether the transport itself is the latency bottleneck.
///
/// Capped at 30, not 60: this loop's own `AcquireNextFrame`/`ReleaseFrame`
/// pair runs every single iteration on a `thread_local` D3D11 device
/// entirely separate from the main window's own WGPU device (see
/// `capture.rs`'s DXGI_CAPTURE thread_local and its KMD-lock deadlock
/// comment, also documented in `video_pipeline.rs` near
/// `leak_capture_resources`) — at 60fps that's ~60 D3D11 driver calls/sec
/// from this thread racing the GUI's own Present() calls for the shared
/// NVIDIA kernel-mode driver lock. Live-found: a full host-process hang
/// (caught by the `hung-guardian` watchdog / VS debugger's "unresponsive"
/// kill) during a static-desktop EVRT2 test, right as this loop was
/// running near its 16.7ms floor. Halving the rate halves how often this
/// thread touches the driver at all — real remote-desktop viewing doesn't
/// need 60fps, and this doesn't remove the underlying cross-device race
/// (that would need a shared/synchronized D3D11 device with WGPU, not
/// available from application code), just how often it's rolled.
const EXPERIMENT_FPS: u32 = 60;

/// Give up waiting for a client HELLO after this long.
const HELLO_TIMEOUT: Duration = Duration::from_secs(30);

/// Stop the stream if nothing arrives from the client for this long
/// (connection presumed gone — client closed the experiment screen, app
/// backgrounded, network dropped).
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// The client is receive-only in steady state (no per-frame ACKs), so
/// without an explicit heartbeat the host never sees any packet from it
/// after the initial HELLO and its `IDLE_TIMEOUT` fires ~15s in even though
/// the client is still happily consuming frames. Send a lightweight
/// FEEDBACK packet on this cadence purely to keep the host's activity
/// clock alive.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(3);

/// ROADMAP.md Phase 6.1: how often to (re-)time a real NVENC encode call
/// for `CapabilityRegistry` calibration. Once per second, not every frame —
/// NVENC's own per-frame cost is essentially constant for a fixed
/// resolution/bitrate, so re-measuring every single frame would just be
/// wasted GPU work for no better a number; a live-refreshed value every
/// ~60 frames still catches the thermal-throttle/driver-reset cases the
/// task doc's Phase 4 comment names, without doubling GPU load constantly.
const NVENC_CALIBRATION_INTERVAL: Duration = Duration::from_secs(1);

/// ROADMAP.md Phase 5.2: how long the client waits for ANY candidate to
/// answer its raced HELLO before giving up entirely. Generous compared to a
/// single-candidate `HELLO_TIMEOUT` wait would need — LAN candidates
/// realistically answer in well under 100ms, so this mostly bounds the
/// worst case (all candidates unreachable, e.g. WAN candidate blocked by a
/// symmetric NAT with no LAN fallback available).
const RACE_TIMEOUT: Duration = Duration::from_secs(5);

fn log(events: &Sender<HostEvent>, msg: impl Into<String>) {
    let _ = events.send(HostEvent::Log(msg.into()));
}

/// Diagnostic-only: the `log` crate (→ logcat via `android_logger`) is only
/// a dependency on Android (see Cargo.toml's `cfg(target_os = "android")`
/// dependency block) — this macro compiles to nothing at all on other
/// platforms rather than a runtime no-op, so `log::` is never referenced
/// (and never needs resolving) outside Android builds. Used at a few
/// diagnostic points in the client-side EVRT2 relay race (ROADMAP.md Phase
/// 5.3 investigation) where `on_status`'s free-text channel isn't reliably
/// observable during a live test (nothing currently polls/displays it on
/// Android outside the in-session status line).
macro_rules! evrt2log {
    ($($arg:tt)*) => {
        #[cfg(target_os = "android")]
        log::info!($($arg)*);
    };
}

/// ROADMAP.md Phase 6.4 — cross-codec splicing container. The IS_SILICON
/// wire flag already means "this VideoFrame's bytes are not a plain EVRTCK
/// stream"; this magic disambiguates the two things it can now mean: a pure
/// NVENC Annex-B bitstream (no magic, decoded as before), or this container
/// carrying BOTH an NVENC background layer and an EVRTCK visible-region
/// overlay for the SAME frame. `b"SPL2"` starts with `0x53`, which can never
/// be the first byte of a real Annex-B stream (those always start with a
/// `00 00 00 01` or `00 00 01` start code) — so a receiver can tell the two
/// apart from the first byte alone, no extra wire flag needed.
const SPLICE_MAGIC: &[u8; 4] = b"SPL2";

/// Builds the spliced payload: `SPL2` + u32-BE-length-prefixed background +
/// u32-BE-length-prefixed overlay. Returns the payload and the byte range
/// (within that payload) the overlay occupies — the caller needs that exact
/// range to mark it VISIBLE_REGION at the packet-scheduler level (Task-01
/// M3/M4: send-order preemption and jitter-bypass), the same mechanism
/// already used for plain EVRTCK frames' visible-region byte ranges.
fn build_spliced_payload(background: &[u8], overlay: &[u8]) -> (Vec<u8>, (usize, usize)) {
    let mut out = Vec::with_capacity(4 + 4 + background.len() + 4 + overlay.len());
    out.extend_from_slice(SPLICE_MAGIC);
    out.extend_from_slice(&(background.len() as u32).to_be_bytes());
    out.extend_from_slice(background);
    out.extend_from_slice(&(overlay.len() as u32).to_be_bytes());
    let overlay_start = out.len();
    out.extend_from_slice(overlay);
    let overlay_range = (overlay_start, overlay_start + overlay.len());
    (out, overlay_range)
}

/// The receive-side counterpart of `build_spliced_payload`. `None` means
/// "not a spliced payload" (either a plain NVENC stream, or malformed) —
/// callers fall back to treating `data` as a plain NVENC bitstream in that
/// case, exactly like before this container format existed.
fn parse_spliced_payload(data: &[u8]) -> Option<(&[u8], &[u8])> {
    if data.len() < 8 || &data[0..4] != SPLICE_MAGIC {
        return None;
    }
    let bg_len = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
    let bg_start: usize = 8;
    let bg_end = bg_start.checked_add(bg_len)?;
    if data.len() < bg_end + 4 {
        return None;
    }
    let ov_len = u32::from_be_bytes(data[bg_end..bg_end + 4].try_into().unwrap()) as usize;
    let ov_start = bg_end + 4;
    let ov_end = ov_start.checked_add(ov_len)?;
    if data.len() < ov_end {
        return None;
    }
    Some((&data[bg_start..bg_end], &data[ov_start..ov_end]))
}

#[cfg(test)]
mod splice_container_tests {
    use super::*;

    #[test]
    fn spliced_payload_round_trips() {
        let bg = vec![0x00, 0x00, 0x00, 0x01, 0xAB, 0xCD]; // fake Annex-B-ish bytes
        let overlay = vec![0xDEu8, 0xAD, 0xBE, 0xEF, 0x01, 0x02];
        let (payload, overlay_range) = build_spliced_payload(&bg, &overlay);

        let (parsed_bg, parsed_overlay) = parse_spliced_payload(&payload).expect("must parse");
        assert_eq!(parsed_bg, bg.as_slice());
        assert_eq!(parsed_overlay, overlay.as_slice());
        assert_eq!(
            &payload[overlay_range.0..overlay_range.1],
            overlay.as_slice()
        );
    }

    #[test]
    fn a_plain_nvenc_stream_is_not_mistaken_for_a_spliced_payload() {
        // Real Annex-B always starts with a start code (0x00 first byte),
        // which can never collide with SPLICE_MAGIC's 'S' (0x53).
        let plain_h264 = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1F];
        assert_eq!(parse_spliced_payload(&plain_h264), None);
    }

    #[test]
    fn truncated_or_malformed_spliced_payloads_are_rejected_not_panicking() {
        assert_eq!(parse_spliced_payload(b"SPL2"), None); // too short for even the length prefix
        assert_eq!(parse_spliced_payload(b"SPL2\x00\x00\x00\xFF"), None); // claims 255-byte bg, has 0
        let (mut payload, _) = build_spliced_payload(&[1, 2, 3], &[4, 5]);
        payload.truncate(payload.len() - 1); // chop the last overlay byte
        assert_eq!(parse_spliced_payload(&payload), None);
    }

    #[test]
    fn empty_layers_round_trip() {
        let (payload, overlay_range) = build_spliced_payload(&[], &[]);
        let (bg, overlay) = parse_spliced_payload(&payload).unwrap();
        assert!(bg.is_empty());
        assert!(overlay.is_empty());
        assert_eq!(overlay_range.0, overlay_range.1);
    }
}

/// ROADMAP.md Phase 6.3 — Codec Race (First Light). `NvencEncoder` wraps
/// raw D3D/NVENC handles (`*mut c_void` in `nvenc_shim.cpp`'s FFI surface)
/// and is NOT `Send` — confirmed by the compiler, not assumed: an earlier
/// version of this code tried spawning a fresh `std::thread::scope` thread
/// per frame to encode with it (matching what `EvrtckEncoder`, pure Rust
/// state, can safely do), and it flatly refused to compile
/// ("`*mut c_void` cannot be sent between threads safely"). D3D11's own
/// threading model expects a context to be used consistently from ONE
/// thread for its whole lifetime anyway, not just "not concurrently" — so
/// even if the Send bound could be forced with `unsafe impl`, doing so
/// would trade a compile error for an unverified runtime risk.
///
/// This is the actually-safe shape: the encoder is CREATED on a single
/// dedicated worker thread and never leaves it. The calling (main
/// experiment) thread sends an `Encode` request, does its OWN EVRTCK work
/// while the worker races in parallel, then blocks on the reply — genuine
/// concurrency without ever moving the encoder itself across a thread
/// boundary.
struct NvencWorker {
    tx: Sender<NvencWorkerMsg>,
    rx: Receiver<NvencWorkerReply>,
    handle: Option<std::thread::JoinHandle<()>>,
}

enum NvencWorkerMsg {
    Encode {
        cap_buf: Arc<Vec<u8>>,
        force_key: bool,
    },
    /// Zero-copy path (ROADMAP Phase 6.3 follow-up): `shared_handle` is a
    /// DXGI shared-texture HANDLE (see `capture::capture_display_into_shared`),
    /// stored as `isize` because a raw HANDLE isn't `Send` — the value is
    /// just an opaque OS handle, so round-tripping it through an integer
    /// across the channel is sound. Cast back to `*mut c_void` only inside
    /// the worker thread, right before calling `encode_texture`.
    EncodeTexture {
        shared_handle: isize,
        force_key: bool,
    },
    Shutdown,
}

#[derive(Debug)]
enum NvencWorkerReply {
    Encoded {
        finish: Instant,
        elapsed: Duration,
        result: Result<Option<crate::nvenc::NvencPacket>, String>,
    },
}

impl NvencWorker {
    /// Blocks briefly (same cost `NvencEncoder::new` always had — this
    /// just runs it on the worker thread instead of the caller's) to
    /// confirm the session actually opened before returning, so callers
    /// keep the same "did this attempt succeed" contract they had before
    /// this was a worker thread at all.
    fn spawn(
        codec: crate::nvenc::NvencCodec,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> Result<Self, String> {
        let (tx, worker_rx) = std::sync::mpsc::channel::<NvencWorkerMsg>();
        let (worker_tx, rx) = std::sync::mpsc::channel::<NvencWorkerReply>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let handle = std::thread::Builder::new()
            .name("evrt2-nvenc-worker".into())
            .spawn(move || {
                let mut enc =
                    match crate::nvenc::NvencEncoder::new(codec, width, height, fps, bitrate) {
                        Ok(enc) => {
                            let _ = ready_tx.send(Ok(()));
                            enc
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                loop {
                    match worker_rx.recv() {
                        Ok(NvencWorkerMsg::Encode { cap_buf, force_key }) => {
                            let start = Instant::now();
                            let result = enc.encode_bgra(&cap_buf, force_key);
                            let finish = Instant::now();
                            if worker_tx
                                .send(NvencWorkerReply::Encoded {
                                    finish,
                                    elapsed: finish.duration_since(start),
                                    result,
                                })
                                .is_err()
                            {
                                break; // caller dropped its receiver — nothing left to report to
                            }
                        }
                        Ok(NvencWorkerMsg::EncodeTexture {
                            shared_handle,
                            force_key,
                        }) => {
                            let start = Instant::now();
                            let result = enc
                                .encode_texture(shared_handle as *mut std::ffi::c_void, force_key);
                            let finish = Instant::now();
                            if worker_tx
                                .send(NvencWorkerReply::Encoded {
                                    finish,
                                    elapsed: finish.duration_since(start),
                                    result,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Ok(NvencWorkerMsg::Shutdown) | Err(_) => break,
                    }
                }
            })
            .expect("spawn evrt2-nvenc-worker");

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                tx,
                rx,
                handle: Some(handle),
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => Err("NVENC worker thread died before reporting readiness".to_owned()),
        }
    }

    /// Send this frame's buffer to the worker without waiting for a reply —
    /// call `recv_result` afterward, once the caller has also done its own
    /// (e.g. EVRTCK) work for the frame, so the two genuinely overlap.
    fn send_request(&self, cap_buf: Arc<Vec<u8>>, force_key: bool) {
        let _ = self.tx.send(NvencWorkerMsg::Encode { cap_buf, force_key });
    }

    /// Zero-copy variant of `send_request` — hands NVENC a GPU shared-texture
    /// handle instead of CPU bytes. See `NvencWorkerMsg::EncodeTexture`.
    fn send_texture_request(&self, shared_handle: isize, force_key: bool) {
        let _ = self.tx.send(NvencWorkerMsg::EncodeTexture {
            shared_handle,
            force_key,
        });
    }

    /// Bounded wait for this frame's result — the bound exists only to
    /// avoid hanging forever if the worker thread has died; a healthy
    /// NVENC encode is expected to finish in low single-digit milliseconds,
    /// nowhere near this budget.
    fn recv_result(&self, timeout: Duration) -> Option<NvencWorkerReply> {
        self.rx.recv_timeout(timeout).ok()
    }
}

impl Drop for NvencWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(NvencWorkerMsg::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Binds a fresh UDP socket and starts the experiment thread. Returns the
/// bound port, a public (WAN) candidate address if STUN succeeded
/// (ROADMAP.md Phase 5.1 — same `netif::discover_public_endpoint` EVRT1
/// already uses, done on this exact socket so the NAT mapping matches),
/// and a fresh AuthTag session key immediately (caller announces all three
/// to the client via `Misc::Evrt2ExperimentEndpoints` — Phase 4.2, same
/// channel `evrt_session_token` already uses for EVRT1) — the thread
/// itself waits for the client's HELLO before doing any capture/encode
/// work, so no host resources are spent until the client actually shows up.
pub fn start_host_experiment(
    events: Sender<HostEvent>,
) -> std::io::Result<(
    u16,
    Option<std::net::SocketAddr>,
    crate::evrt2_crypto::SessionKey,
    Arc<AtomicBool>,
)> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    let port = socket.local_addr()?.port();
    // Blocking, ≤500ms (see netif::discover_public_endpoint) — same
    // synchronous-before-thread-spawn timing EVRT1's own host-side STUN
    // call already uses; a WAN candidate is worth a short, bounded delay
    // here rather than complicating this into an async follow-up message.
    let public_addr = crate::netif::discover_public_endpoint(&socket);
    if let Some(addr) = public_addr {
        log(
            &events,
            format!("EVRT2 (experimental): STUN обнаружил публичный адрес {addr}"),
        );
    }
    let auth_key = crate::evrt2_crypto::generate_session_key();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    std::thread::Builder::new()
        .name("evrt2-experiment-host".into())
        .spawn(move || run_host_experiment(socket, auth_key, stop_thread, events))
        .expect("spawn evrt2-experiment-host");

    Ok((port, public_addr, auth_key, stop))
}

/// ROADMAP.md Phase 5.3 — RELAY_WRAP: same contract as `start_host_experiment`,
/// plus a relay candidate racing the UDP ones (SDUDP.md § Path Probing —
/// "Relay tunnel endpoint" is the third candidate kind that spec names,
/// alongside LAN and public/STUN). Use this instead of `start_host_experiment`
/// when the caller already owns a TCP relay stream it can tunnel bytes over
/// — currently only `run_evrt2_only_session` (host.rs) does. `relay_inbound`
/// receives raw bytes the caller already unwrapped from incoming
/// `Misc::Evrt2RelayWrap` messages; the returned `Receiver<Vec<u8>>` is
/// where the caller drains bytes THIS session wants wrapped and sent back
/// out over that same TCP stream. Whichever candidate (UDP LAN, UDP/STUN
/// WAN, or relay) completes a HELLO/ACK handshake first wins; the other
/// candidates' wait loops notice within one poll interval (≤200ms) and give
/// up, so only one encode loop ever actually runs.
pub fn start_host_experiment_with_relay_race(
    events: Sender<HostEvent>,
    relay_inbound: Receiver<Vec<u8>>,
) -> std::io::Result<(
    u16,
    Option<std::net::SocketAddr>,
    crate::evrt2_crypto::SessionKey,
    Arc<AtomicBool>,
    Receiver<Vec<u8>>,
)> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    let port = socket.local_addr()?.port();
    let public_addr = crate::netif::discover_public_endpoint(&socket);
    if let Some(addr) = public_addr {
        log(
            &events,
            format!("EVRT2 (experimental): STUN обнаружил публичный адрес {addr}"),
        );
    }
    let auth_key = crate::evrt2_crypto::generate_session_key();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let (relay_outbound_tx, relay_outbound_rx) = std::sync::mpsc::channel::<Vec<u8>>();

    std::thread::Builder::new()
        .name("evrt2-experiment-host-race".into())
        .spawn(move || {
            run_host_experiment_race(
                socket,
                relay_outbound_tx,
                relay_inbound,
                auth_key,
                stop_thread,
                events,
            )
        })
        .expect("spawn evrt2-experiment-host-race");

    Ok((port, public_addr, auth_key, stop, relay_outbound_rx))
}

/// Races `wait_for_udp_hello_and_ack` against `wait_for_relay_hello_and_ack`
/// on two throwaway threads, each reporting its session (if any) back over
/// one shared channel — first one in wins. `race_won` is purely internal
/// (distinct from `stop`, which governs the whole session's later lifetime,
/// not just this race): it tells the loser to stop polling within ~200ms of
/// a winner being decided, without touching `stop` itself.
fn run_host_experiment_race(
    socket: UdpSocket,
    relay_outbound: Sender<Vec<u8>>,
    relay_inbound: Receiver<Vec<u8>>,
    auth_key: crate::evrt2_crypto::SessionKey,
    stop: Arc<AtomicBool>,
    events: Sender<HostEvent>,
) {
    log(
        &events,
        "EVRT2 (experimental): ожидание HELLO — гонка UDP vs relay…".to_owned(),
    );
    let race_won = Arc::new(AtomicBool::new(false));
    let (winner_tx, winner_rx) = std::sync::mpsc::channel::<Evrt2Session>();
    // ROADMAP.md Phase 5.4: if the relay racer loses, it hands its still-live
    // channel pair back over this side channel instead of dropping it, so a
    // later degradation-triggered switch (inside `run_experiment_encode_loop`)
    // has a relay fallback ready without re-establishing anything from
    // scratch — the TCP relay connection itself never went away, only this
    // particular race for it.
    let (relay_return_tx, relay_return_rx) =
        std::sync::mpsc::channel::<(Sender<Vec<u8>>, Receiver<Vec<u8>>)>();

    let udp_stop = stop.clone();
    let udp_race_won = race_won.clone();
    let udp_events = events.clone();
    let udp_winner_tx = winner_tx.clone();
    let udp_thread = std::thread::Builder::new()
        .name("evrt2-race-udp".into())
        .spawn(move || {
            let should_stop =
                || udp_stop.load(Ordering::Relaxed) || udp_race_won.load(Ordering::Relaxed);
            if let Some(session) =
                wait_for_udp_hello_and_ack(socket, auth_key, &should_stop, &udp_events)
            {
                let _ = udp_winner_tx.send(session);
            }
        })
        .expect("spawn evrt2-race-udp");

    let relay_stop = stop.clone();
    let relay_race_won = race_won.clone();
    let relay_events = events.clone();
    let relay_thread = std::thread::Builder::new()
        .name("evrt2-race-relay".into())
        .spawn(move || {
            let should_stop =
                || relay_stop.load(Ordering::Relaxed) || relay_race_won.load(Ordering::Relaxed);
            match wait_for_relay_hello_and_ack(
                relay_outbound,
                relay_inbound,
                auth_key,
                &should_stop,
                &relay_events,
            ) {
                RelayRaceOutcome::Won(session) => {
                    let _ = winner_tx.send(session);
                }
                RelayRaceOutcome::GaveUp(outbound, inbound) => {
                    let _ = relay_return_tx.send((outbound, inbound));
                }
                RelayRaceOutcome::None => {}
            }
        })
        .expect("spawn evrt2-race-relay");

    // Both racer threads hold their own clone of the sender; once BOTH have
    // exited without winning, this original `winner_rx.recv()` call
    // correctly returns Err instead of blocking forever.
    match winner_rx.recv() {
        Ok(session) => {
            race_won.store(true, Ordering::Relaxed);
            let _ = udp_thread.join();
            let _ = relay_thread.join();
            // If UDP won, the relay racer's `GaveUp` send (if it happens at
            // all — the relay stream could itself be gone) is already
            // sitting in the channel by the time both threads are joined,
            // so a non-blocking `try_recv` is enough — no need to wait.
            let relay_fallback = relay_return_rx.try_recv().ok();
            // Live bug found during Phase 6.4 network testing: the client
            // races HELLO over BOTH paths simultaneously by design (see
            // `wait_for_relay_hello_and_ack`'s doc). `should_stop()` in that
            // function is checked BEFORE draining `inbound`, so when UDP
            // wins first, the client's own simultaneous relay-path HELLO
            // can already be sitting unconsumed in `inbound` at the moment
            // `GaveUp` fires — it's stale race noise, not a later genuine
            // degradation-triggered switch request. Without this drain,
            // `run_experiment_encode_loop`'s mid-session poller picked it
            // up on its very first iteration and switched transport to
            // relay milliseconds after connecting, while the client (which
            // never attempted any switch) kept sending over UDP — the host
            // then only listened on the relay channel, causing a false
            // "нет активности клиента" timeout ~15s later. Confirmed live:
            // host log showed "переключение на relay" in the same second
            // as "клиент подключён (UDP)".
            if let Some((_, inbound)) = relay_fallback.as_ref() {
                while inbound.try_recv().is_ok() {}
            }
            run_experiment_encode_loop(session, stop, events, relay_fallback);
        }
        Err(_) => {
            let _ = udp_thread.join();
            let _ = relay_thread.join();
            log(
                &events,
                "EVRT2 (experimental): ни UDP, ни relay-путь не получили HELLO — сессия отменена"
                    .to_owned(),
            );
        }
    }
}

fn run_host_experiment(
    socket: UdpSocket,
    auth_key: crate::evrt2_crypto::SessionKey,
    stop: Arc<AtomicBool>,
    events: Sender<HostEvent>,
) {
    log(
        &events,
        "EVRT2 (experimental): ожидание HELLO от клиента (UDP)…".to_owned(),
    );
    let should_stop = || stop.load(Ordering::Relaxed);
    let Some(session) = wait_for_udp_hello_and_ack(socket, auth_key, &should_stop, &events) else {
        return;
    };
    // ROADMAP.md Phase 5.4: this call site (`start_host_experiment`, the
    // plain UDP-only path used by the mid-session "EVRT2" button — see
    // RELAY_TUNNEL.md's "Scope" section) has no relay channel to fall back
    // to at all, so a degradation-triggered switch is honestly impossible
    // here — `None`, not a fabricated pair.
    run_experiment_encode_loop(session, stop, events, None);
}

/// ROADMAP.md Phase 5.3 — RELAY_WRAP: the relay-tunneled counterpart of
/// `wait_for_udp_hello_and_ack`, run by `run_evrt2_only_session`'s own TCP
/// loop (it already owns the relay stream) as a second, parallel candidate
/// racing the UDP one above — same "first path to complete a handshake
/// wins" principle SDUDP.md § Path Probing already describes for LAN/WAN
/// UDP candidates, just extended to the relay tunnel as a third candidate
/// kind. `inbound` receives raw wire bytes the caller already unwrapped
/// from incoming `Misc::Evrt2RelayWrap` messages; `outbound` is where this
/// function's session hands back bytes for the caller to wrap and send.
/// Returns `None` if `stop` fires or `inbound` is dropped (caller gave up)
/// before a HELLO ever arrives — deliberately no independent timeout here,
/// since the caller's own TCP session loop already has its own lifecycle
/// bound (`host_stop`/`take_kick_request`) that governs how long this is
/// worth waiting.
fn wait_for_relay_hello_and_ack(
    outbound: Sender<Vec<u8>>,
    inbound: Receiver<Vec<u8>>,
    auth_key: crate::evrt2_crypto::SessionKey,
    should_stop: &dyn Fn() -> bool,
    events: &Sender<HostEvent>,
) -> RelayRaceOutcome {
    let placeholder_peer: std::net::SocketAddr = "0.0.0.0:0".parse().unwrap();
    loop {
        if should_stop() {
            return RelayRaceOutcome::GaveUp(outbound, inbound);
        }
        match inbound.recv_timeout(Duration::from_millis(200)) {
            Ok(raw) => match crate::evrt2_packet::PacketHeader::decode(&raw) {
                Ok((header, payload))
                    if header.packet_type == crate::evrt2_packet::PacketType::SessionHello =>
                {
                    let client_max_res = if let Some(info) = parse_hello(payload) {
                        log(events, format!(
                                "EVRT2 (experimental): HELLO через relay (max_fps={} max_res={}x{})",
                                info.max_fps, info.max_res.0, info.max_res.1
                            ));
                        info.max_res
                    } else {
                        (0, 0)
                    };
                    let mut session = Evrt2Session::from_relay_channels(
                        outbound,
                        inbound,
                        placeholder_peer,
                        Mode::Ar,
                    );
                    session.set_client_max_res(client_max_res);
                    return if finish_hello_handshake(&mut session, auth_key, events, "relay") {
                        RelayRaceOutcome::Won(session)
                    } else {
                        RelayRaceOutcome::None
                    };
                }
                Ok((header, _)) => {
                    log(events, format!(
                            "EVRT2 (experimental): relay-инбаунд {} байт декодирован, но type={:?} (не HELLO)",
                            raw.len(), header.packet_type
                        ));
                }
                Err(e) => {
                    log(events, format!(
                            "EVRT2 (experimental): relay-инбаунд {} байт НЕ декодировался как EVRT2-заголовок: {e}",
                            raw.len()
                        ));
                }
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return RelayRaceOutcome::None,
        }
    }
}

/// ROADMAP.md Phase 5.4: `wait_for_relay_hello_and_ack`'s three possible
/// outcomes. Unlike a plain `Option<Evrt2Session>`, the "lost the race"
/// case is split in two: `GaveUp` hands back the still-live `(outbound,
/// inbound)` channel pair — the UDP side won, but the relay channels
/// themselves are fine and can be kept around for a later
/// degradation-triggered switch (see `run_host_experiment_race`) — while
/// `None` means the channels are gone for good (the caller's relay TCP
/// stream itself disconnected, or ACK send failed after already consuming
/// them into a session) and there's nothing left to keep.
enum RelayRaceOutcome {
    Won(Evrt2Session),
    GaveUp(Sender<Vec<u8>>, Receiver<Vec<u8>>),
    None,
}

/// UDP counterpart of `wait_for_relay_hello_and_ack` — the pre-Phase-5.3
/// raw HELLO-wait loop, unchanged in behavior, just extracted into its own
/// function so `run_evrt2_only_session` can race it against the relay path
/// instead of it always being the only candidate.
fn wait_for_udp_hello_and_ack(
    socket: UdpSocket,
    auth_key: crate::evrt2_crypto::SessionKey,
    should_stop: &dyn Fn() -> bool,
    events: &Sender<HostEvent>,
) -> Option<Evrt2Session> {
    let deadline = Instant::now() + HELLO_TIMEOUT;
    // See `Evrt2Session::client_max_res`'s doc for why this is captured at
    // all — set from the client's own HELLO below, applied to `session`
    // once it exists.
    let mut client_max_res: (u32, u32) = (0, 0);
    let peer = loop {
        if should_stop() {
            log(
                events,
                "EVRT2 (experimental): остановлено до подключения клиента".to_owned(),
            );
            return None;
        }
        if Instant::now() > deadline {
            log(
                events,
                "EVRT2 (experimental): клиент не подключился за 30с (UDP) — сессия отменена"
                    .to_owned(),
            );
            return None;
        }
        let mut buf = [0u8; 1500];
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                if let Ok((header, payload)) =
                    crate::evrt2_packet::PacketHeader::decode(&buf[..len])
                {
                    if header.packet_type == crate::evrt2_packet::PacketType::SessionHello {
                        if let Some(info) = parse_hello(payload) {
                            log(
                                events,
                                format!(
                                "EVRT2 (experimental): HELLO от {from} (max_fps={} max_res={}x{})",
                                info.max_fps, info.max_res.0, info.max_res.1
                            ),
                            );
                            client_max_res = info.max_res;
                        }
                        break from;
                    }
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                log(
                    events,
                    format!("EVRT2 (experimental): socket error while waiting for HELLO: {e}"),
                );
                return None;
            }
        }
    };

    // ROADMAP.md Phase 2: start in AR — the transition diagram's base state
    // — rather than the previously-hardcoded Mode47. Mode47 mandates a
    // REQUIRED silicon encoder (AR2R47_MODES.md § 47), and this experiment
    // has none (CPU EVRTCK only; a real hardware-encoder RoiEncoding
    // provider for the EVRT2 path is Phase 6 work) — starting in Mode47
    // was never honestly reachable once `ModeSelector` actually governs the
    // session, so it isn't claimed here.
    let mut session = match Evrt2Session::from_bound_socket(socket, peer, Mode::Ar) {
        Ok(s) => s,
        Err(e) => {
            log(
                events,
                format!("EVRT2 (experimental): session setup failed: {e}"),
            );
            return None;
        }
    };
    session.set_client_max_res(client_max_res);
    finish_hello_handshake(&mut session, auth_key, events, "UDP").then_some(session)
}

/// Shared tail of both HELLO-wait paths: send ACK, then apply the AuthTag/
/// encryption key only after that — see the comment inside for exactly why
/// the ordering matters. Returns whether it succeeded (`send_ack` is the
/// only fallible step).
fn finish_hello_handshake(
    session: &mut Evrt2Session,
    auth_key: crate::evrt2_crypto::SessionKey,
    events: &Sender<HostEvent>,
    via: &str,
) -> bool {
    if session.send_ack().is_err() {
        log(
            events,
            format!("EVRT2 (experimental): failed to send ACK ({via})"),
        );
        return false;
    }
    // ROADMAP.md Phase 4.2/4.3 — set *after* ACK is sent, deliberately: the
    // raw pre-session HELLO wait loop above decodes the client's HELLO
    // before any `Evrt2Session`/key exists to check it against, and the
    // client sets its own key only after receiving this ACK (see the
    // client-side comment below) — so HELLO/ACK genuinely travel
    // unauthenticated/unencrypted on both sides, matching what the earlier
    // version of this comment claimed but the code didn't yet enforce
    // (setting the key before `send_ack` would have silently encrypted
    // ACK, which is harmless in practice — the client already has the key
    // by then — but inconsistent with the stated design).
    session.set_auth_key(Some(auth_key));
    log(
        events,
        format!("EVRT2 (experimental): клиент подключён ({via}), стрим начат ✓"),
    );
    true
}

/// ROADMAP.md Phase 2.4′ — stateful loss/jitter-based bandwidth-floor
/// estimator, with hysteresis. **Rewritten after a live test caught a real
/// bug in the first (stateless, pure-function) version**: under a heavily
/// loaded live session the host's actual frame rate dropped far below its
/// 60fps target (observed ~3fps), so a 3-second FEEDBACK window only
/// covered a handful of real frames — a single dropped frame against that
/// tiny denominator computed as a nonsensical loss rate (a live log showed
/// "потери 411.1% (37/9 кадров)", loss over 100%, immediate proof the
/// number was noise, not signal), and reacting to that ONE noisy sample
/// with no debouncing flipped `bandwidth_bps` back and forth every
/// feedback, producing a MODE_SWITCH AR↔2R flapping loop live — the EXACT
/// bug class the original send-rate-based rule (see the history comment on
/// `run_experiment_encode_loop`'s `bandwidth_bps` local) was disabled for.
/// Two independent fixes, both required:
/// 1. `MIN_FRAMES_FOR_RATE` — a loss RATE computed from too few frames sent
///    in the window is never trusted at all (jitter can still trigger
///    independently of frame count, since it's not a ratio).
/// 2. Hysteresis — `CONSECUTIVE_REQUIRED` matching samples (constrained OR
///    clear) are required before the forced state actually flips, the same
///    debouncing principle `evrt2_rtt::RttEstimator` already uses for RTT
///    degradation (3 consecutive breaches, not one spike).
struct BandwidthEstimator {
    consecutive_constrained: u32,
    consecutive_clear: u32,
    forced: bool,
}

impl BandwidthEstimator {
    const LOSS_RATE_FORCES_AR: f32 = 0.02;
    const JITTER_FORCES_AR_US: u32 = 50_000;
    /// Below this many frames sent in the window, a computed loss RATE is
    /// meaningless (one dropped frame out of three looks like 33% loss) —
    /// the live bug this whole struct exists to fix.
    const MIN_FRAMES_FOR_RATE: u32 = 20;
    const CONSECUTIVE_REQUIRED: u32 = 2;

    fn new() -> Self {
        Self {
            consecutive_constrained: 0,
            consecutive_clear: 0,
            forced: false,
        }
    }

    /// `dropped_delta`/`frames_sent_delta` are THIS feedback window's counts
    /// (already-subtracted deltas, not cumulative totals — the caller owns
    /// tracking the previous cumulative values). Returns the `bandwidth_bps`
    /// to feed `ModeSignals`, plus the loss rate that produced this sample's
    /// verdict (for logging; `None` when the window was too small to trust
    /// a rate at all — distinct from "0% loss", which the caller's log
    /// message should say plainly rather than printing a fabricated number).
    fn on_feedback(
        &mut self,
        dropped_delta: u32,
        frames_sent_delta: u32,
        jitter_p95_us: u32,
    ) -> (u32, Option<f32>) {
        let loss_rate = if frames_sent_delta >= Self::MIN_FRAMES_FOR_RATE {
            Some(dropped_delta as f32 / frames_sent_delta as f32)
        } else {
            None
        };
        let sample_constrained = loss_rate.is_some_and(|r| r > Self::LOSS_RATE_FORCES_AR)
            || jitter_p95_us > Self::JITTER_FORCES_AR_US;

        if sample_constrained {
            self.consecutive_constrained += 1;
            self.consecutive_clear = 0;
        } else {
            self.consecutive_clear += 1;
            self.consecutive_constrained = 0;
        }
        if self.consecutive_constrained >= Self::CONSECUTIVE_REQUIRED {
            self.forced = true;
        } else if self.consecutive_clear >= Self::CONSECUTIVE_REQUIRED {
            self.forced = false;
        }

        let bandwidth_bps = if self.forced {
            // Comfortably under the floor, not just barely — this is a
            // real, sustained observed constraint, not a borderline guess.
            crate::evrt2_modes::BANDWIDTH_FORCES_AR_BPS / 2
        } else {
            u32::MAX
        };
        (bandwidth_bps, loss_rate)
    }
}

/// The capture → EVRTCK encode → EVRT2 send loop, transport-agnostic — runs
/// identically whether `session` is UDP- or relay-backed (ROADMAP.md
/// Phase 5.3), since `Evrt2Session` itself is the only thing that knows
/// the difference.
///
/// `relay_fallback`: ROADMAP.md Phase 5.4 — `Some((outbound, inbound))`
/// when `session` won via UDP AND a live relay channel pair is still
/// available (the relay racer lost the initial HELLO race but its TCP
/// relay connection itself is still up) — kept ready so a client-initiated,
/// degradation-triggered switch mid-session (see `run_client_experiment`'s
/// RTT handling) has somewhere host-side to land. `None` for a session that
/// either already won via relay (nothing to fall back FURTHER to) or has no
/// relay channel at all (`start_host_experiment`'s plain UDP-only path).
fn run_experiment_encode_loop(
    mut session: Evrt2Session,
    stop: Arc<AtomicBool>,
    events: Sender<HostEvent>,
    mut relay_fallback: Option<(Sender<Vec<u8>>, Receiver<Vec<u8>>)>,
) {
    // ROADMAP.md Phase 1.1: same Windows perf hints the live EVRT1 path
    // already relies on (1ms timer resolution, HIGH process priority,
    // display-sleep block) — without these the encode loop's sleep
    // granularity alone can cost several ms of jitter per frame. RAII: hints
    // are released automatically when this function returns/breaks.
    let _perf_hints = crate::evrt_session::WindowsPerfHints::enable(&events);

    // ROADMAP.md Phase 6.3′ follow-up: the per-iteration `recv_one()` drain
    // below is documented as running "without blocking", but a
    // `Transport::Udp` session's socket keeps the 200ms read timeout its
    // HELLO-handshake constructor set — live-measured to stall roughly
    // 200ms of EVERY capture/encode/send iteration whenever no incoming
    // packet happened to be waiting at that exact instant (the common case:
    // FEEDBACK only arrives every ~3s). Flip to true non-blocking here, now
    // that the handshake (which genuinely needs a bounded blocking wait) is
    // already done. See `set_nonblocking_reads`'s own doc for the full story.
    if let Err(e) = session.set_nonblocking_reads() {
        log(
            &events,
            format!("EVRT2 (experimental): set_nonblocking_reads failed: {e}"),
        );
    }

    // ── Capture → EVRTCK encode → EVRT2 send loop ────────────────────────
    let mut encoder: Option<EvrtckEncoder> = None;
    // Task 01's Visible Region source, honestly degraded: this experiment
    // has no real cursor input (EVRT2-only sessions don't process
    // MouseEvent — see run_evrt2_only_session in host.rs), so there is no
    // explicit client focus to use. Falls back to the spec's documented
    // alternative — "top-percentile tiles from the Attention Map" — built
    // from real motion+surprise signals (evrt2_attention.rs), not faked.
    let mut attention: Option<AttentionMapBuilder> = None;
    let mut last_breach_log = Instant::now() - Duration::from_secs(10);
    let mut last_apf_too_large_log = Instant::now() - Duration::from_secs(20);
    // ROADMAP.md Phase 3.2: the last APF (cols, rows, tile_size all locked
    // together with the map) this session actually put on the wire — the
    // baseline every delta is computed against. Reset to `None` whenever
    // the encoder itself resets (resolution change, below) since a stale
    // baseline at the wrong dimensions can't be delta'd against safely.
    let mut last_sent_apf: Option<(Vec<f32>, u16, u16, u8)> = None;
    // ROADMAP.md Phase 6.1: registry both providers' real measured costs
    // get fed into, and the NVENC session itself (opened lazily once a
    // resolution is known — see the encoder-reset block below, which also
    // clears this back to `None` on a resolution change since an NVENC
    // session is tied to a fixed width/height).
    let mut registry = CapabilityRegistry::probe(false);
    let mut nvenc: Option<NvencWorker> = None;
    let mut nvenc_probe_failed_once = false;
    let mut last_nvenc_calibration = Instant::now() - NVENC_CALIBRATION_INTERVAL;
    // Cross-vendor hardware encode fallback: NVENC only exists on NVIDIA
    // silicon. On Intel/AMD hardware (no NVENC at all — `nvenc_probe_failed_once`
    // fires every calibration tick), this test used to fall all the way
    // back to CPU-only EVRTCK (~50-150ms/frame under real motion, the exact
    // slow baseline this whole H265 investigation started from). Media
    // Foundation's own hardware encoder MFT enumeration is vendor-agnostic
    // — it's the SAME mechanism `host.rs`'s live EVRT1 pipeline already
    // uses to reach Intel Quick Sync or AMD's hardware encoder blocks (see
    // `mf_encode.rs`'s own doc comment: "MFT encoders directly", no
    // NVIDIA-specific code at all), calling the vendor's real silicon
    // through Windows' standard hardware-encoder abstraction rather than a
    // slower generic path. Tried only when NVENC itself isn't available —
    // NVENC's own zero-copy GPU-texture path stays the preferred one on
    // NVIDIA hardware, this is purely the fallback for everyone else.
    let mut mf_encoder: Option<crate::mf_encode::MfVideoEncoder> = None;
    // ROADMAP.md Phase 6.3 — Codec Race (First Light) profiling counters.
    let mut codec_race_samples: u64 = 0;
    let mut codec_race_nvenc_wins: u64 = 0;
    // ROADMAP.md Phase 6.4 — cross-codec splicing counter.
    let mut spliced_frames: u64 = 0;
    // ROADMAP.md Phase 6.3′ — Adaptive Codec Race. Live fps testing found
    // the always-race-both-every-frame design capped the WHOLE pipeline at
    // EVRTCK's own speed (50-150ms+/frame under real full-motion
    // 2560×1440 content) even once NVENC had proven reliably faster,
    // because EVRTCK's full-frame encode ran synchronously on the
    // critical path every frame — its output is the actual sent packet
    // whenever NVENC isn't chosen. Once NVENC wins `NVENC_TRUST_THRESHOLD`
    // races in a row, skip EVRTCK's expensive full-frame pass for the next
    // `NVENC_TRUST_WINDOW` frames and trust NVENC directly, re-racing
    // periodically (not permanently) so a real regression is still caught.
    //
    // Follow-up (same investigation that produced the recv_one()/zero-copy
    // fixes): live-measured NVENC cost (~7.5ms) vs EVRTCK cost (50-150ms
    // under real motion) means NVENC wins essentially every race it runs —
    // the original 45-frame window with a full 10-consecutive-win bar to
    // RE-enter it meant ~10 of every ~55 frames still paid EVRTCK's full
    // cost in steady state, for zero benefit (NVENC was always going to be
    // chosen anyway). `NVENC_TRUST_WINDOW` is now far longer, and
    // `NVENC_TRUST_RENEWAL_THRESHOLD` (lower than the initial-trust bar)
    // governs RE-entering trust once a window expires — trust, once
    // genuinely earned, doesn't need to be re-proven from scratch every
    // time, just lightly reconfirmed. `nvenc_trust_established` tracks
    // whether that initial, harder-won proof has happened yet this
    // session; it's reset wherever `nvenc` itself is reset (resolution
    // change, worker death, timeout) so a fresh NVENC session always earns
    // trust the hard way once, honoring the same "not an excuse to
    // fabricate" principle the rest of this codebase holds to — trust is
    // still periodically re-verified, never assumed forever.
    let mut consecutive_nvenc_wins: u32 = 0;
    let mut trust_nvenc_frames_left: u32 = 0;
    let mut nvenc_trust_established = false;
    const NVENC_TRUST_THRESHOLD: u32 = 10;
    const NVENC_TRUST_RENEWAL_THRESHOLD: u32 = 2;
    const NVENC_TRUST_WINDOW: u32 = 600;
    let mut cap_buf: Vec<u8> = Vec::new();
    // Live-found (chasing the "why doesn't EVRT2 reach 60fps like the live
    // EVRT1 pipeline" gap, continued): comparing a live EVRT1 session and a
    // live EVRT2 session side by side, on the SAME phone over the SAME
    // WiFi, showed EVRT1 holding a rock-solid 60fps while EVRT2 struggled
    // at 27-38fps under identical motion — but EVRT1's own session log
    // read "EVRT SessionConfig: 1080×608@60" while EVRT2 had been sending
    // full native 2560×1440 the entire time. EVRT1 respects the client's
    // own declared `max_res` from its `SessionHello` (`evrt_session.rs`'s
    // `client_cap_resolution`); EVRT2's HELLO parsing captured the SAME
    // field (`Evrt2Session::client_max_res`) but nothing ever downscaled to
    // it — every downstream cost (the attention-map diff scan, NVENC's own
    // encode, the actual wire bytes) had been paying for ~5.5x more pixels
    // than the phone's own screen can even show. `downscale_buf` is the
    // persistent scratch buffer the per-frame downscale (see the capture
    // block below) writes into.
    // Live-found follow-up: matching EVRT1's own conservative downscale
    // target (a real fix for the fps gap above) turned out to be the WRONG
    // quality target once actually compared side by side — EVRT1's
    // 1080×608 default exists to keep a SOFTWARE encoder viable
    // (`video_pipeline.rs`'s own `software_encoder_downscale_target`
    // comment: "1440p/4K в OpenH264 = сотни мс/кадр"), a constraint EVRT2
    // doesn't have (NVENC hardware encode has plenty of headroom at
    // 1080p60 once the OTHER real bottlenecks — blocking capture, the
    // attention-map scan — are gone, which they now are). Live-reported:
    // the phone's picture looked soft/blurry at the downscaled resolution,
    // not just lower-fps. `MIN_ENCODE_RES` floors the effective cap so a
    // narrow-width portrait phone (e.g. 1080px wide) doesn't drag the
    // stream down to sub-720p — `client_max_res` still applies as a CEILING
    // above this floor (a client that genuinely can't handle 1080p, e.g. a
    // real low-end device, still gets respected), it just no longer
    // applies as an aggressive default on every device.
    const MIN_ENCODE_RES: (u32, u32) = (1920, 1080);
    let client_max_res = {
        let (w, h) = session.client_max_res();
        if w == 0 || h == 0 {
            (0, 0) // still "unset" — no cap at all, unchanged behavior
        } else {
            (w.max(MIN_ENCODE_RES.0), h.max(MIN_ENCODE_RES.1))
        }
    };
    let mut downscale_buf: Vec<u8> = Vec::new();
    // ROADMAP.md Phase 1.3/6.4 debug hook: EVRT2-only test sessions never
    // process a real MouseEvent (see the `attention` doc above), so
    // `region.tiles` can't reliably cross R2's 0.80 visible-region
    // threshold from motion+surprise alone — measured live ceiling ~0.637.
    // This isn't fakeable in the wire protocol (the client's focus really
    // is absent), but for exercising the DOWNSTREAM code that only runs
    // when a region exists (DEGRADE_SIGNAL, Phase 6.4 splicing) this env
    // var injects a synthetic center-screen focus tile, standing in for
    // the cursor position a real game session would supply. Off by
    // default — only for a deliberate live debug session, never silently
    // changes production behavior.
    let force_focus_center = std::env::var("EVRTDESK_EVRT2_FORCE_FOCUS_CENTER")
        .map(|v| v == "1")
        .unwrap_or(false);
    // ROADMAP.md task #30: H265 is now the DEFAULT silicon codec for EVRT2 —
    // live-measured on real hardware (MI_8/Adreno630) at 25-36 decoded fps,
    // 4-6× H264's own measured range (~5-8fps) on the identical phone,
    // network, and content, because HEVC's better compression at the same
    // bitrate means less data per frame through the same network/decode
    // ceiling this session's whole investigation (recv_one blocking, NVENC
    // trust, AIMD pacing) already found. `EVRTDESK_EVRT2_CODEC` still
    // overrides — set to "h264" to force the old default back (e.g. for
    // A/B comparison, or a client where H265 doesn't apply — see below).
    // H265 client-side decode routes through Android's MediaCodec Surface
    // path (`crate::android_video::decode_frame_to_surface`), NOT the
    // `openh264` software decoder used for H264 — see the client-side
    // IS_SILICON branch in `run_client_experiment` for why: `openh264` is a
    // Cisco H.264-only implementation, there is no H.265 software decoder
    // wired into this codebase, so a real H265 comparison needs the
    // platform's own hardware decoder, same as the live (non-EVRT2)
    // pipeline already uses for H265. On a non-Android EVRT2 test client
    // this decode path is unavailable (`decoded_frame_to_surface` compiles
    // to an always-`false` stub off-Android) — that client's own
    // `silicon_decode_healthy` flag turns `false`, its FEEDBACK reports
    // `silicon_ok: false`, and the host's existing `rebalance()`/demotion
    // mechanism (Phase 6.2) already handles that gracefully, falling back
    // to EVRTCK — the same honest degradation path a genuinely NVENC-less
    // host already goes through, not a new failure mode.
    let evrt2_want_h265 = std::env::var("EVRTDESK_EVRT2_CODEC")
        .map(|v| !v.eq_ignore_ascii_case("h264"))
        .unwrap_or(true);
    let mut frame_id: u32 = 0;
    let frame_interval = Duration::from_secs_f32(1.0 / EXPERIMENT_FPS as f32);
    // ROADMAP.md Phase 6.3′ follow-up: closed-loop send-rate pacing. Live
    // testing found that extending NVENC trust (task #31, above) let this
    // loop run ~4x faster than before (iter_gap_ms ~120-190ms -> ~35ms) —
    // which sounds like a win, but EVRTCK's own encode cost had been
    // ACCIDENTALLY the only rate limit this pipeline ever had. Removing it
    // exposed that nothing downstream (the network path and/or the phone's
    // software H264 decode) can actually sustain anywhere near that rate:
    // confirmed live, `dropped_frames` climbed continuously (1 -> 199+ over
    // ~1 minute) and `decoded_fps` collapsed toward 0. This is the exact
    // gap FEEDBACK.md already documented as missing — "decoded_fps <
    // target × 0.8 → reduce resolution or FPS cap: No — not acted on."
    // `dynamic_frame_interval` closes it with a simple AIMD controller
    // (additive-increase/multiplicative-decrease, the same shape TCP
    // congestion control uses, for the same reason: back off hard and fast
    // on real evidence of overload, recover slowly and cautiously once
    // healthy) driven by the `dropped_delta`/`frames_sent_delta` this loop
    // already computes per FEEDBACK cycle for the Phase 2.4′ bandwidth
    // estimator — reusing that existing signal, not inventing a new one.
    // Live-found (chasing the initial-connection loss burst): starting
    // this at the full `frame_interval` (60fps) means the FIRST FEEDBACK —
    // the only signal that can ever slow this down — doesn't arrive for
    // ~3s (the client's own feedback cadence), so a new session always
    // blasts at max rate with zero backpressure for that whole window.
    // Confirmed live: 58% loss (105/180) in the very first feedback
    // window, immediately after a clean connect. Same fix TCP's own
    // slow-start uses for the identical problem — don't assume full
    // capacity before anything has confirmed it — starting at half rate
    // and letting the SAME speedup logic below ramp it up if the network
    // really can sustain more costs a couple of seconds of lower initial
    // fps, in exchange for not immediately overwhelming whatever the real
    // ceiling turns out to be.
    let mut dynamic_frame_interval = frame_interval.mul_f32(2.0);
    const PACE_MAX_INTERVAL: Duration = Duration::from_millis(200); // floor: never below 5fps
    const PACE_SLOWDOWN_FACTOR: f32 = 1.5;
    // Live-found: backing off on ANY loss at all — even a single dropped
    // frame out of 100+ sent — made the pacing (and so the actual
    // capture/send cadence, i.e. visible motion smoothness) saw-tooth
    // constantly under real sustained motion, since some baseline packet
    // loss is normal on real networks and was tripping a full 1.5x
    // slowdown every few seconds. A rate threshold treats single-packet
    // noise as noise and only backs off on genuine congestion.
    const PACE_LOSS_RATE_THRESHOLD: f32 = 0.02; // 2%
                                                // Live-found (chasing visible "рывки" — motion judder — despite
                                                // healthy average fps): a single over-threshold window still
                                                // triggered a full 1.5x slowdown, and every slowdown/recovery step
                                                // audibly/visibly changes the send cadence — the AIMD controller
                                                // itself was a source of perceived judder, not just network jitter.
                                                // Same hysteresis shape `BandwidthEstimator` (Phase 2.4′) already uses
                                                // for its own loss-rate decision, for the same reason documented
                                                // there: one bad window is at least as likely to be a transient blip
                                                // as real sustained congestion, and reacting to noise is worse than a
                                                // one-window delay in reacting to the real thing.
    const PACE_SLOWDOWN_CONSECUTIVE_WINDOWS: u32 = 2;
    let mut consecutive_high_loss_windows: u32 = 0;
    // Live-found (a fixed-step follow-up to the 2ms→5ms fix below): a
    // FIXED per-window recovery step means the time to fully recover after
    // one legitimate backoff (a real scene change — most of the screen
    // changing at once — genuinely does spike encoded size and can trip
    // one real loss event) scales with how far the multiplicative
    // slowdown pushed the interval, since a fixed step closes the SAME
    // number of ms every window regardless of how big the remaining gap
    // is. Reported live as "a few seconds where it feels like it's
    // recalibrating" after any big on-screen change — the fixed-step
    // recovery from that one legitimate backoff was doing exactly what it
    // was designed to do, just slower than a professional-tool user
    // (fast typing, 3D work) wants after a single settled event. Closing a
    // FRACTION of the remaining gap each window (proportional/exponential
    // recovery, the same shape a PID controller's own decay term uses)
    // fixes this without weakening the safety property at all: still
    // additive-shaped (never jumps straight back to full speed in one
    // step), still can't overshoot past `frame_interval`, just closes most
    // of a large gap in the first couple of windows instead of taking as
    // long as a small gap would with a fixed step.
    const PACE_RECOVERY_FRACTION: f32 = 0.5; // halve the remaining gap each window
    let mut next_due = Instant::now();
    let mut last_activity = Instant::now();
    // TEMP DEBUG (fps-ceiling investigation, following the zero-copy
    // capture fix): measures the HOST LOOP's own real iteration rate,
    // independent of `decoded_fps` — if this stays near 60 while
    // `decoded_fps` stays low, the cap has moved past capture/encode into
    // send pacing, the network, or the client's own decode throughput.
    let mut last_iter_start: Option<Instant> = None;
    // ROADMAP.md Phase 5.4 — live-found bug #2: draining `relay_fallback`
    // ONCE at capture time (see `run_host_experiment_race`) only clears
    // packets already queued at that exact instant. It does NOT protect
    // against the client's own initial-race relay HELLO arriving a few
    // milliseconds LATER — the relay tunnel has strictly higher latency
    // than the winning UDP path, so this is the common case, not an edge
    // case: confirmed live twice, "переключение на relay" firing in the
    // same second as "клиент подключён (UDP)", far too early for any real
    // `RttEstimator` sample (that needs multiple KEEPALIVE round trips) to
    // have fired. Fix: ignore (but still drain, so it doesn't pile up and
    // get consumed the instant the grace period ends) any HELLO arriving
    // in the first few seconds of the session — a genuine RTT-degradation
    // switch physically cannot trigger this early.
    let switch_grace_until = Instant::now() + Duration::from_secs(3);
    // The encoder only self-heals from lost/corrupted delta frames when it's
    // told to emit a full keyframe. Without this, the very first frame was
    // the ONLY keyframe ever sent (`is_keyframe` used to be `frame_id == 0`)
    // — any dropped delta packet on real WiFi (this loop has no perf/QoS
    // tuning, unlike the live EVRT1 path) corrupted the picture forever,
    // matching the "ссыпется" (image crumbling over time) symptom. The live
    // pipeline avoids this by periodically re-sending IDR frames; mirror
    // that here.
    let mut last_keyframe = Instant::now();
    // ROADMAP.md task #30 — live-found on real hardware (MI_8/Adreno630,
    // H265): a WALL-CLOCK periodic keyframe interval interacts badly with
    // DXGI's change-driven capture (a captured frame only exists when the
    // screen actually changed — see this session's own earlier discussion
    // of why "shaking windows raises fps"). Under near-static content,
    // captures come in sparsely (one every several real seconds), and each
    // one individually exceeds a short wall-clock threshold — so EVERY
    // single frame during a static stretch was being forced as a full IDR,
    // producing an unusual near-all-keyframe bitstream with large gaps
    // between frames. Live-confirmed this exact pattern correlates with the
    // black-screen/frozen-picture symptom: `onOutputBufferAvailable`'s own
    // frame counter kept climbing normally (decode never actually stalled)
    // while the picture stayed frozen, and every logged frame during that
    // stretch was `IDR frame: codec=H265`. Switched the periodic trigger to
    // count actual SENT frames instead of wall-clock time — a genuinely
    // sparse/idle stream naturally avoids re-triggering a keyframe on every
    // rare frame, closer to how a real, mostly-P-frame bitstream normally
    // looks. `KEYFRAME_INTERVAL_MAX_WALL_CLOCK` stays as a pure safety net
    // (long-term drift protection for a session that's ACTUALLY streaming
    // continuously, not one that's merely idle) — it should rarely fire in
    // practice once the frame-count trigger is doing its job.
    const KEYFRAME_INTERVAL_FRAMES: u32 = 120;
    const KEYFRAME_INTERVAL_MAX_WALL_CLOCK: Duration = Duration::from_secs(30);
    let mut frames_since_keyframe: u32 = 0;
    // ROADMAP.md Phase 1.4: client-requested keyframe (decode error on its
    // end) jumps the queue instead of waiting up to the periodic interval.
    let mut client_requested_keyframe = false;

    // ROADMAP.md Phase 2.1/2.4: AR2R47 mode selector now actually governs
    // this session (previously the module existed but nothing called it).
    // `silicon_available`/`game_detected`/`user_requested_game_mode` are
    // honestly false here — this experiment has no real detector for any
    // of them yet (see the Mode::Ar start-state comment above) — so the
    // state machine can only ever move between AR and 2R until those
    // signals exist for real; that's the correct, honest behavior, not a
    // limitation being hidden.
    let mut mode_selector = ModeSelector::new(Mode::Ar);
    let mut low_motion_since: Option<Instant> = None;
    // ROADMAP.md Phase 2.4 — history: an earlier version of this code
    // measured "bytes actually sent" and fed that in as `bandwidth_bps`, on
    // the theory that low traffic means low available bandwidth. That's
    // wrong — it measures the OPPOSITE thing. AR mode is *designed* to send
    // very little on a static screen (the spec's own "floors at 10KB/s"
    // claim), so the instant motion pushed the selector AR→2R, the
    // still-modest CPU-EVRTCK traffic measured under 200KB/s again and
    // immediately forced it back to AR — a MODE_SWITCH every ~1s, visible
    // live as AR/2R ping-ponging in the log. "Bytes sent" is never a valid
    // proxy for "bandwidth available".
    //
    // ROADMAP.md Phase 2.4′ — closed now that Phase 5.4 gave this loop a
    // real congestion signal to work with: `ReceiverFeedback2.dropped_frames`
    // (cumulative frame-sequence gaps the client actually observed) and
    // `.jitter_p95_us` (the client's own real jitter estimator). This is
    // deliberately NOT a literal bits-per-second measurement — passively
    // estimating true available bandwidth needs active probing (packet-pair
    // dispersion or similar), out of scope here — it's a translation of
    // genuine congestion symptoms into the `bandwidth_bps` interface
    // `ModeSelector` already expects, the same "proxy, not the real formula"
    // honesty this codebase already applies elsewhere (see the EVRT Gain
    // proxy comment). Starts at "assume plenty" and only drops below
    // `evrt2_modes::BANDWIDTH_FORCES_AR_BPS` when a FEEDBACK packet reports
    // real, sustained loss or real, high jitter — never from how much this
    // loop itself chose to send, so the AR↔2R flapping bug above cannot
    // recur through this path.
    let mut bandwidth_estimator = BandwidthEstimator::new();
    let mut bandwidth_bps: u32 = u32::MAX;
    let mut last_feedback_frame_id: Option<u32> = None;
    let mut last_feedback_dropped_frames: u32 = 0;
    // Live-found bug (Phase 6.4 splice-gate investigation): once NVENC is
    // demoted, `use_nvenc` goes false, which stops any more NVENC frames
    // from being sent — so the client's `silicon_decode_healthy` never gets
    // another decode attempt to prove it's recovered, and it just reports
    // the SAME stale `silicon_ok=false` in every subsequent FEEDBACK.
    // `demote_provider` resets its 30s cooldown clock on every call
    // (deliberately, for genuinely repeated fresh failures — see its own
    // doc), so a single early hiccup turned into a PERMANENT lockout: every
    // stale repeat re-armed the timeout before it could ever expire.
    // Confirmed live: `nvenc_demoted` flipped true early in a session and
    // never recovered for the rest of an 80s+ test, `use_nvenc` staying
    // false throughout even though NVENC was consistently measured cheaper
    // than EVRTCK. Fix: only feed `rebalance()` a FRESH negative edge (a
    // transition from healthy to unhealthy), not every repeated stale
    // report — the 30s cooldown can then actually expire once, giving
    // NVENC the "timeout, not a life sentence" second chance the code's
    // own doc already promises.
    let mut last_silicon_ok = true;

    // ROADMAP.md task #29 — `capture_cost_ms` variance (6ms steady-state vs
    // 80-160ms occasionally) turned out NOT to be jitter inside the DXGI
    // capture call itself: per-stage timers added to `capture.rs`
    // (`EVRTDESK_CAPTURE_DEBUG=1`) showed `capture_frame`'s own internal
    // cost never exceeded ~10ms across an entire 378-frame live session.
    // The expensive samples were real, but they landed in the OUTER
    // `capture_dxgi_into` layer instead — live-confirmed: frame 0 measured
    // 139.94ms of `capture_cost_ms` while that same frame's inner
    // `capture_frame` cost only 9.57ms, and one more elevated sample
    // (79.57ms) appeared a few frames later before settling permanently at
    // ~6-7ms for the rest of the session. That gap matches known one-time
    // costs this call path pays exactly once per session: `DxgiCapture::new`
    // (D3D11 device + `DuplicateOutput` creation) and the GPU driver's own
    // clock ramp from its idle power state up to sustained load. Rather
    // than let the client's first several real frames absorb that cost,
    // pay it here — a throwaway capture before the loop starts, while the
    // client is still just past HELLO/ACK and hasn't seen a frame yet.
    let mut warmup_buf: Vec<u8> = Vec::new();
    let _ = crate::capture::capture_display_into_shared(0, &mut warmup_buf);

    // Live-found (chasing the "why doesn't EVRT2 reach 60fps like the live
    // EVRT1 pipeline" gap): `AcquireNextFrame(0, ...)` on Windows 11 can
    // block until DWM actually composites a new frame — the SAME
    // documented quirk `video_pipeline.rs`'s own capture thread already
    // works around (see its "Game-mode static-frame cache" comment) — even
    // though the "0" timeout nominally means non-blocking. Confirmed live:
    // `capture_cost_ms` alternated between ~5ms and ~75-100ms roughly every
    // 3rd-4th frame under real sustained motion, not just once at session
    // start. Calling capture INLINE in this same loop (as this code used
    // to) meant every one of those blocking calls stalled attention-map +
    // encode + send too, directly capping the achievable frame rate to
    // DWM's own composite cadence. Fixed the same way EVRT1 already does:
    // a dedicated capture thread fills `cap_slot`; the main loop below
    // takes whatever's there (non-blocking) and reuses the last captured
    // frame in place when nothing new has arrived yet, decoupling this
    // loop's pacing entirely from DXGI's blocking behavior.
    type CapSlot = Arc<Mutex<Option<(u32, u32, Vec<u8>, Option<isize>)>>>;
    let cap_slot: CapSlot = Arc::new(Mutex::new(None));
    let cap_stop = stop.clone();
    let cap_slot_thread = cap_slot.clone();
    let _cap_handle = std::thread::Builder::new()
        .name("evrt2-experiment-capture".into())
        .spawn(move || {
            let mut buf = Vec::new();
            let mut next_capture_due = Instant::now();
            loop {
                if cap_stop.load(Ordering::Relaxed) {
                    break;
                }
                let now = Instant::now();
                if now < next_capture_due {
                    std::thread::sleep((next_capture_due - now).min(Duration::from_millis(2)));
                    continue;
                }
                next_capture_due += frame_interval;
                if next_capture_due < Instant::now() {
                    next_capture_due = Instant::now() + frame_interval;
                }
                if let Some((w, h, shared_handle)) =
                    crate::capture::capture_display_into_shared(0, &mut buf)
                {
                    if let Ok(mut slot) = cap_slot_thread.lock() {
                        match slot.as_mut() {
                            Some(s) if s.0 == w && s.1 == h => {
                                std::mem::swap(&mut s.2, &mut buf);
                                s.3 = shared_handle;
                            }
                            _ => *slot = Some((w, h, buf.clone(), shared_handle)),
                        }
                    }
                }
            }
            // Same KMD-lock precaution as `video_pipeline.rs`'s own capture
            // thread — leak the D3D11 device rather than let its destructor
            // race WGPU/WGL during teardown.
            crate::capture::leak_capture_resources();
        })
        .expect("spawn evrt2 experiment capture thread");
    let mut have_first_frame = false;
    let mut cap_w: u32 = 0;
    let mut cap_h: u32 = 0;
    let mut cap_shared_handle: Option<isize> = None;

    loop {
        if stop.load(Ordering::Relaxed) {
            log(&events, "EVRT2 (experimental): остановлено".to_owned());
            break;
        }
        // Drain any incoming control packets (FEEDBACK/GOODBYE/IDR_REQUEST/KEEPALIVE) without blocking.
        while let Ok(Some((header, payload))) = session.recv_one() {
            last_activity = Instant::now();
            match header.packet_type {
                crate::evrt2_packet::PacketType::Goodbye => {
                    log(
                        &events,
                        "EVRT2 (experimental): клиент отключился (GOODBYE)".to_owned(),
                    );
                    return;
                }
                crate::evrt2_packet::PacketType::IdrRequest => {
                    client_requested_keyframe = true;
                }
                // ROADMAP.md Phase 6.2: closes the loop `rebalance()` was
                // built for — the client's own decode-side silicon health
                // (`silicon_ok`) now genuinely means something once it can
                // decode NVENC frames at all (Phase 6.1 step 4). A `false`
                // here demotes NVENC_H264 for `DEMOTION_COOLDOWN`, so
                // `use_nvenc` above naturally falls back to CPU_EVRTCK
                // without either side needing a dedicated "give up on
                // silicon" message.
                crate::evrt2_packet::PacketType::Feedback => {
                    if let Some(feedback) = ReceiverFeedback2::decode(&payload) {
                        log(&events, format!(
                            "EVRT2 (experimental): FEEDBACK decoded_fps={:.1} jitter_p95={:.1}мс dropped_frames={} silicon_ok={}",
                            feedback.decoded_fps, feedback.jitter_p95_us as f32 / 1000.0,
                            feedback.dropped_frames, feedback.silicon_ok
                        ));
                        if !feedback.silicon_ok && last_silicon_ok {
                            registry.rebalance(&feedback, PROVIDER_NVENC_H264);
                        }
                        last_silicon_ok = feedback.silicon_ok;

                        // ROADMAP.md Phase 2.4′ — real bandwidth-floor
                        // signal, loss/jitter-based (see this function's
                        // `bandwidth_bps` doc for why not send-rate, and
                        // `BandwidthEstimator`'s own doc for why it needs
                        // hysteresis + a minimum sample size — both found
                        // necessary by a live flapping bug, not designed in
                        // up front). Loss RATE needs a denominator — how
                        // many frames this loop actually sent since the
                        // LAST feedback, tracked via this host's own
                        // `frame_id` counter delta, not anything the client
                        // reports (the client only reports the numerator:
                        // how many of those it never saw).
                        let dropped_delta = feedback
                            .dropped_frames
                            .saturating_sub(last_feedback_dropped_frames);
                        last_feedback_dropped_frames = feedback.dropped_frames;
                        let frames_sent_delta = last_feedback_frame_id
                            .map(|prev| frame_id.saturating_sub(prev))
                            .unwrap_or(0);
                        last_feedback_frame_id = Some(frame_id);

                        // AIMD send-rate pacing (see this loop's own doc
                        // comment on `dynamic_frame_interval` for the live
                        // regression that made this necessary). Genuine
                        // congestion this window (loss rate over the
                        // threshold) → back off hard, immediately; a clean
                        // — or just noisy — window → ease back toward the
                        // 60fps target a little at a time, never
                        // overshooting straight back to full speed.
                        let loss_rate = if frames_sent_delta > 0 {
                            dropped_delta as f32 / frames_sent_delta as f32
                        } else {
                            0.0
                        };
                        let old_pace_ms = dynamic_frame_interval.as_secs_f32() * 1000.0;
                        if loss_rate > PACE_LOSS_RATE_THRESHOLD {
                            consecutive_high_loss_windows += 1;
                            if consecutive_high_loss_windows >= PACE_SLOWDOWN_CONSECUTIVE_WINDOWS {
                                dynamic_frame_interval = dynamic_frame_interval
                                    .mul_f32(PACE_SLOWDOWN_FACTOR)
                                    .min(PACE_MAX_INTERVAL);
                            }
                        } else {
                            consecutive_high_loss_windows = 0;
                            if frames_sent_delta > 0 {
                                let gap = dynamic_frame_interval.saturating_sub(frame_interval);
                                dynamic_frame_interval =
                                    frame_interval + gap.mul_f32(PACE_RECOVERY_FRACTION);
                            }
                        }
                        if (dynamic_frame_interval.as_secs_f32() * 1000.0 - old_pace_ms).abs() > 0.5
                        {
                            log(&events, format!(
                                "EVRT2 (experimental): AIMD pacing {:.1}ms → {:.1}ms (dropped {dropped_delta}/{frames_sent_delta} this window)",
                                old_pace_ms, dynamic_frame_interval.as_secs_f32() * 1000.0
                            ));
                        }
                        let was_constrained =
                            bandwidth_bps < crate::evrt2_modes::BANDWIDTH_FORCES_AR_BPS;
                        let (new_bandwidth_bps, loss_rate) = bandwidth_estimator.on_feedback(
                            dropped_delta,
                            frames_sent_delta,
                            feedback.jitter_p95_us,
                        );
                        bandwidth_bps = new_bandwidth_bps;
                        let constrained =
                            bandwidth_bps < crate::evrt2_modes::BANDWIDTH_FORCES_AR_BPS;
                        if constrained && !was_constrained {
                            let loss_desc = match loss_rate {
                                Some(r) => format!("потери {:.1}% ({dropped_delta}/{frames_sent_delta} кадров)", r * 100.0),
                                None => format!("окно слишком мало для доли потерь ({dropped_delta}/{frames_sent_delta} кадров)"),
                            };
                            log(&events, format!(
                                "EVRT2 (experimental): реальная деградация канала — {loss_desc}, jitter {:.1}мс — форсирую AR (Phase 2.4′)",
                                feedback.jitter_p95_us as f32 / 1000.0
                            ));
                        }
                    }
                }
                // ROADMAP.md Phase 5.4: an RTT ping from the client — echo
                // the same 8 bytes back verbatim. See `build_keepalive_ping`
                // doc for why this rides KEEPALIVE instead of a dedicated
                // packet type. A payload-less KEEPALIVE (the pre-5.4 idle
                // heartbeat) simply has no ping to answer here.
                crate::evrt2_packet::PacketType::Keepalive => {
                    if let Some(send_time_us) = crate::evrt2_session::parse_keepalive_ping(&payload)
                    {
                        let _ = session.send_keepalive_ping(send_time_us);
                    }
                }
                _ => {}
            }
        }

        // ROADMAP.md Phase 5.4 — Path switching, host side: a client that
        // decided its current UDP path degraded sends a fresh HELLO over
        // the relay channel this session's initial race kept on standby
        // (see `relay_fallback`'s doc on this function). Only polled while
        // NOT already relay-backed (once switched, `session.recv_one()`
        // above is already reading these exact bytes as normal traffic)
        // and only while a fallback pair is actually still available.
        // `and_then` here fully resolves to an owned value before the
        // later `relay_fallback.take()` — deliberately, so the immutable
        // borrow needed to poll doesn't overlap the mutable borrow needed
        // to consume, which the borrow checker would otherwise reject.
        if !session.is_relay() {
            let switch_hello: Option<Vec<u8>> = relay_fallback
                .as_ref()
                .and_then(|(_, inbound)| inbound.try_recv().ok());
            let past_grace = Instant::now() >= switch_grace_until;
            if let Some(raw) = switch_hello {
                if !past_grace {
                    // Still inside the grace window — this is the tail of
                    // the initial UDP-vs-relay race arriving late over the
                    // slower relay path, not a genuine degradation switch.
                    // Drained above already; nothing more to do here.
                } else if let Ok((header, _payload)) =
                    crate::evrt2_packet::PacketHeader::decode(&raw)
                {
                    if header.packet_type == crate::evrt2_packet::PacketType::SessionHello {
                        if let Some((outbound, inbound)) = relay_fallback.take() {
                            let ack = crate::evrt2_session::build_ack(session.mode());
                            if outbound.send(ack).is_ok() {
                                let placeholder_peer: std::net::SocketAddr =
                                    "0.0.0.0:0".parse().unwrap();
                                session.switch_to_relay(outbound, inbound, placeholder_peer);
                                log(&events, "EVRT2 (experimental): переключение на relay — клиент сообщил о деградации UDP-пути (Phase 5.4)".to_owned());
                            } else {
                                log(&events, "EVRT2 (experimental): переключение на relay не удалось — ACK не отправлен".to_owned());
                            }
                        }
                    }
                }
            }
        }

        if last_activity.elapsed() > IDLE_TIMEOUT {
            log(
                &events,
                "EVRT2 (experimental): нет активности клиента — остановлено".to_owned(),
            );
            break;
        }

        let now = Instant::now();
        if now < next_due {
            std::thread::sleep((next_due - now).min(Duration::from_millis(5)));
            continue;
        }
        next_due += dynamic_frame_interval;
        if next_due < Instant::now() {
            next_due = Instant::now() + dynamic_frame_interval;
        }

        let capture_start = Instant::now();
        let iter_gap_ms =
            last_iter_start.map(|prev| capture_start.duration_since(prev).as_secs_f32() * 1000.0);
        last_iter_start = Some(capture_start);
        // ROADMAP.md Phase 6.3 follow-up (zero-copy): `cap_buf` carries CPU
        // bytes — EVRTCK genuinely needs them for tile-diffing — alongside a
        // GPU-shareable handle (`shared_handle`) so NVENC can be fed via
        // `encode_texture` instead of a CPU buffer roundtrip when
        // available. `shared_handle` is `None` on the GDI fallback path or
        // if the shared-texture create failed; callers below must fall
        // back to the CPU path in that case.
        //
        // Non-blocking take from the dedicated capture thread's slot (see
        // this function's own doc comment above the thread spawn for why
        // capture no longer runs inline here) — reuses the last captured
        // frame in place (leaves `cap_buf`/`cap_w`/`cap_h`/
        // `cap_shared_handle` untouched) whenever the capture thread hasn't
        // produced a new one since the last time this loop checked, the
        // same "identical content = near-zero-cost repeat P-frame"
        // semantics `video_pipeline.rs`'s own capture-thread cache uses.
        if let Ok(mut slot) = cap_slot.lock() {
            if let Some((w, h, buf, shared_handle)) = slot.take() {
                // See `client_max_res`'s own doc comment (near `cap_buf`'s
                // declaration) for why this downscale exists at all.
                // Applied here, once per freshly captured native frame —
                // not once per loop iteration — so the "reuse the last
                // frame" path above stays a true no-copy cache hit even on
                // a session that's downscaling: the cached bytes are
                // already at the client's resolution, not the native one.
                if let Some((dw, dh)) = client_downscale_target(w, h, client_max_res) {
                    downscale_bgra_box(&buf, w, h, &mut downscale_buf, dw, dh);
                    std::mem::swap(&mut cap_buf, &mut downscale_buf);
                    cap_w = dw;
                    cap_h = dh;
                    // The GPU shared texture is still at native resolution
                    // — feeding it to NVENC here would silently encode the
                    // wrong (full-res) pixels. Force the CPU path below
                    // (`cap_buf`, now downscaled) instead.
                    cap_shared_handle = None;
                } else {
                    cap_buf = buf;
                    cap_w = w;
                    cap_h = h;
                    cap_shared_handle = shared_handle;
                }
                have_first_frame = true;
            }
        }
        if !have_first_frame {
            continue; // capture thread hasn't produced its first frame yet
        }
        let (w, h, shared_handle) = (cap_w, cap_h, cap_shared_handle);
        let capture_cost_ms = capture_start.elapsed().as_secs_f32() * 1000.0;
        if encoder.as_ref().map(|e| (e.width(), e.height())) != Some((w as usize, h as usize)) {
            let mut fresh = EvrtckEncoder::new(w as usize, h as usize);
            fresh.request_keyframe();
            encoder = Some(fresh);
            attention = Some(AttentionMapBuilder::new(w as usize, h as usize));
            last_keyframe = Instant::now();
            last_sent_apf = None; // stale baseline at the old resolution — must not delta against it
                                  // ROADMAP.md Phase 6.1: an NVENC session is opened for a fixed
                                  // width/height — a resolution change invalidates it exactly
                                  // like the EVRTCK encoder above, so it's dropped and re-opened
                                  // lazily by the calibration tick below. Same for the MF
                                  // hardware-encoder fallback — `MfVideoEncoder` is also tied to
                                  // a fixed width/height.
            nvenc = None;
            mf_encoder = None;
            // A fresh NVENC session at the new resolution hasn't proven
            // anything yet — any trust from the old session doesn't carry
            // over (see NVENC_TRUST_RENEWAL_THRESHOLD's own doc comment).
            trust_nvenc_frames_left = 0;
            consecutive_nvenc_wins = 0;
            nvenc_trust_established = false;
        }
        let enc = encoder.as_mut().unwrap();
        let attn = attention.as_mut().unwrap();

        // Attention Map → Visible Region (Task 01), and physically steer the
        // encoder's own tile ordering toward it: `set_focus_pixel` already
        // makes EVRTCK emit tiles nearest the given point FIRST in the wire
        // stream, so marking a byte prefix as VISIBLE_REGION below lines up
        // with what's actually at the front of `packet.data`, not a guess.
        // Uses the mode ACTIVE for this frame — session.mode() is only
        // updated after a MODE_SWITCH is actually sent (see end of loop),
        // so this always matches what's really on the wire right now.
        let active_mode = session.mode();
        let focus_tile = force_focus_center.then(|| (attn.tiles_x() / 2, attn.tiles_y() / 2));
        let attn_start = Instant::now();
        // `EVRTDESK_EVRT2_RAW_MODE=1` — a deliberately blunt debug knob, not
        // a shipped feature: skip the Attention Map/APF machinery entirely
        // (this whole block down to the APF send below) so a live session
        // can be A/B'd against "just NVENC, nothing else" to find out how
        // much of the remaining gap to Moonlight/Sunshine-class streaming
        // is this protocol's own CPU overhead vs. something else (network,
        // client render pacing). `region` stays empty either way, which
        // already no-ops `set_focus_pixel` below and the splicing overlay
        // further down — raw mode doesn't need separate gates there.
        let raw_mode = std::env::var("EVRTDESK_EVRT2_RAW_MODE")
            .map(|v| v == "1")
            .unwrap_or(false);
        let attn_map = if raw_mode {
            Vec::new()
        } else {
            attn.compute(&cap_buf, focus_tile, active_mode)
        };
        let attn_cost_ms = attn_start.elapsed().as_secs_f32() * 1000.0;
        let region = if raw_mode {
            crate::evrt2_scheduler::VisibleRegion { tiles: Vec::new() }
        } else {
            visible_region_from_map(&attn_map, attn.tiles_x(), visible_threshold(active_mode))
        };
        if let Some(&top_tile) = region.tiles.first() {
            let tx = top_tile as usize % attn.tiles_x();
            let ty = top_tile as usize / attn.tiles_x();
            enc.set_focus_pixel(
                (tx * crate::evrtck::TILE_SIZE) as u32,
                (ty * crate::evrtck::TILE_SIZE) as u32,
            );
        }

        let is_keyframe = frame_id == 0
            || frames_since_keyframe >= KEYFRAME_INTERVAL_FRAMES
            || last_keyframe.elapsed() >= KEYFRAME_INTERVAL_MAX_WALL_CLOCK
            || client_requested_keyframe;
        client_requested_keyframe = false;
        if is_keyframe {
            enc.request_keyframe();
            last_keyframe = Instant::now();
            frames_since_keyframe = 0;
        } else {
            frames_since_keyframe += 1;
        }

        // ROADMAP.md Phase 3.1: APF sent alongside each keyframe (EVRT2CKMAX.md
        // § APF: "transmitted as part of each keyframe"). At native 32px
        // tiles this can exceed one UDP datagram on larger captures (2560×1440
        // → 3600 tiles, 1800 bytes — measured live, not hypothetical). Rather
        // than fragment a field whose whole point is being a *compact*
        // priority summary, the APF cell size is coarsened (grouping
        // native tiles, MAX priority per group) just enough to fit — the
        // spec's own `tile_size` header field exists precisely to allow
        // this. Falls back to a one-time log only in the — currently
        // unreachable for any real resolution — case `fit_scale_for_budget`
        // can't find a fitting scale at all.
        //
        // ROADMAP.md Phase 3.2: on a NON-keyframe, send a delta against the
        // last APF this session actually put on the wire instead of staying
        // silent until the next keyframe (the pre-3.2 behavior) — "delta-
        // updated mid-session when content context changes" per
        // EVRT2CKMAX.md's own APF description. `last_sent_apf` is the
        // baseline both sides need to agree on; it's only trustworthy when
        // the coarsening scale hasn't changed since (a resolution change
        // mid-session invalidates it, so this falls back to waiting for the
        // next keyframe's full snapshot rather than delta-ing against a
        // baseline the client doesn't have).
        let apf_start = Instant::now();
        if raw_mode {
            // See `raw_mode`'s own doc comment above — skip the APF send
            // entirely in this debug mode.
        } else {
            match crate::evrt2_apf::fit_scale_for_budget(
                attn.tiles_x(),
                attn.tiles_y(),
                crate::evrt2_packet::MAX_PAYLOAD,
            ) {
                Some(scale) => {
                    let (apf_map, apf_cols, apf_rows) = if scale == 1 {
                        (attn_map.clone(), attn.tiles_x(), attn.tiles_y())
                    } else {
                        crate::evrt2_apf::downsample_max(
                            &attn_map,
                            attn.tiles_x(),
                            attn.tiles_y(),
                            scale,
                        )
                    };
                    let apf_tile_size = (crate::evrtck::TILE_SIZE * scale).min(255) as u8;
                    let can_delta = !is_keyframe
                        && last_sent_apf.as_ref().is_some_and(|(prev, pc, pr, pts)| {
                            *pc == apf_cols as u16
                                && *pr == apf_rows as u16
                                && *pts == apf_tile_size
                                && prev.len() == apf_map.len()
                        });
                    let send_result = if can_delta {
                        let (prev, _, _, _) = last_sent_apf.as_ref().unwrap();
                        session.send_apf_delta(
                            prev,
                            &apf_map,
                            apf_cols as u16,
                            apf_rows as u16,
                            apf_tile_size,
                        )
                    } else if is_keyframe {
                        session.send_apf_update(
                            &apf_map,
                            apf_cols as u16,
                            apf_rows as u16,
                            apf_tile_size,
                        )
                    } else {
                        // No usable baseline yet (session/resolution just
                        // started) — wait for the next keyframe's full snapshot
                        // rather than send a delta against nothing.
                        Ok(())
                    };
                    match send_result {
                        Ok(()) => {
                            if is_keyframe || can_delta {
                                last_sent_apf = Some((
                                    apf_map,
                                    apf_cols as u16,
                                    apf_rows as u16,
                                    apf_tile_size,
                                ));
                            }
                        }
                        Err(e) => log(
                            &events,
                            format!("EVRT2 (experimental): APF send failed: {e}"),
                        ),
                    }
                }
                None => {
                    if last_apf_too_large_log.elapsed() >= Duration::from_secs(10) {
                        last_apf_too_large_log = Instant::now();
                        log(&events, format!(
                        "EVRT2 (experimental): APF skipped — {}x{} native grid has no fitting cell size up to 255 tiles/edge",
                        attn.tiles_x(), attn.tiles_y()
                    ));
                    }
                }
            }
        }
        let apf_cost_ms = apf_start.elapsed().as_secs_f32() * 1000.0;
        let send_start = Instant::now();
        // Session OPEN attempts are throttled (a failed/successful open is
        // expensive — see nvenc_shim.cpp's own session-open cost) — but once
        // a session exists, ENCODE runs every frame, not throttled.
        if nvenc.is_none()
            && mf_encoder.is_none()
            && last_nvenc_calibration.elapsed() >= NVENC_CALIBRATION_INTERVAL
        {
            last_nvenc_calibration = Instant::now();
            let evrt2_codec = if evrt2_want_h265 {
                NvencCodec::H265
            } else {
                NvencCodec::H264
            };
            // Live-found (same EVRT1-parity fps investigation as the
            // downscale above): this used to hardcode 8Mbps regardless of
            // resolution — reusing EVRT1's own resolution-aware formula
            // (`host.rs`'s `h264_target_bitrate_bps_pub`) fixed that, but
            // at EVRT1's own default `quality_milli` (1000) the formula
            // gives ~6.8Mbps at 1080p60 — well under what NVENC hardware
            // encode can comfortably push (unlike EVRT1's own
            // quality/bitrate defaults, which are tuned to also cover a
            // SOFTWARE encoder fallback path EVRT2 doesn't have). Live
            // comparison against Moonlight/Sunshine-class game streaming —
            // typically 10-20+ Mbps at 1080p60 — found EVRT1's default too
            // conservative for what this hardware-only pipeline can
            // afford. `quality_milli=2000` both doubles the base rate AND
            // crosses the formula's own `DEFAULT_QUALITY_MILLI` threshold,
            // unlocking its higher ceiling (`PERFORMANCE_MAX_BPS` instead
            // of `DEFAULT_MAX_BPS`) — ~13.7Mbps at 1080p60, close to
            // Moonlight/Sunshine's own usual range for the same resolution.
            let evrt2_bitrate_bps =
                crate::host::h264_target_bitrate_bps_pub(w, h, EXPERIMENT_FPS, 2_000);
            match NvencWorker::spawn(evrt2_codec, w, h, EXPERIMENT_FPS, evrt2_bitrate_bps) {
                Ok(worker) => {
                    log(&events, format!(
                        "EVRT2 (experimental): NVENC session opened (dedicated worker thread, codec={})",
                        if evrt2_want_h265 { "H265" } else { "H264" }
                    ));
                    nvenc = Some(worker);
                }
                Err(e) => {
                    // Expected and unremarkable on non-NVIDIA hardware or
                    // when the driver's session limit is already used by
                    // the live EVRT1 pipeline. Log once, not every retry
                    // for the rest of the session.
                    if !nvenc_probe_failed_once {
                        nvenc_probe_failed_once = true;
                        log(&events, format!("EVRT2 (experimental): NVENC unavailable ({e}) — trying Media Foundation hardware encode instead"));
                    }
                    // Cross-vendor fallback (Intel Quick Sync, AMD's
                    // hardware encoder block, or NVIDIA via MF if the raw
                    // NVENC SDK path above somehow failed) — see
                    // `mf_encoder`'s own doc comment for why this reaches
                    // real vendor silicon, not a slow generic path. Only
                    // a HARDWARE MFT counts here — a software-only MFT
                    // would just be a slower, worse-latency version of
                    // the CPU_EVRTCK fallback this is trying to avoid.
                    let mf_status = crate::mf_encode::mf_encoder_status();
                    let mf_hw_available = if evrt2_want_h265 {
                        mf_status.has_hardware_h265()
                    } else {
                        mf_status.has_hardware_h264()
                    };
                    if mf_hw_available {
                        match crate::mf_encode::MfVideoEncoder::new(
                            evrt2_codec,
                            w,
                            h,
                            EXPERIMENT_FPS,
                            evrt2_bitrate_bps,
                        ) {
                            Ok(enc) => {
                                log(&events, format!(
                                    "EVRT2 (experimental): Media Foundation hardware {} encoder opened (Intel Quick Sync/AMD/other — not NVENC)",
                                    if evrt2_want_h265 { "H265" } else { "H264" }
                                ));
                                mf_encoder = Some(enc);
                            }
                            Err(e) => {
                                log(&events, format!("EVRT2 (experimental): Media Foundation hardware encode also unavailable ({e}) — staying on CPU_EVRTCK"));
                            }
                        }
                    }
                }
            }
        }

        // ROADMAP.md Phase 6.3 — Codec Race (First Light): EVRTCK (on this,
        // the calling thread) and NVENC (on its dedicated `NvencWorker`
        // thread — see that type's doc for why it can't just be a
        // per-frame `std::thread::scope` spawn) encode the SAME captured
        // frame genuinely IN PARALLEL. The request is sent to the worker
        // BEFORE this thread starts its own EVRTCK work, specifically so
        // the two overlap instead of running back-to-back; each side
        // records its own completion `Instant`, so "who finished first"
        // reflects the actual wall-clock race, not call/reply order.
        //
        // ROADMAP.md Phase 1.2: exact per-tile byte offsets instead of an
        // averaged prefix estimate — the top-P_i tile from the Attention Map
        // is not always the tile nearest the encoder's focus anchor (P_i
        // also weighs motion and surprise), so a byte-prefix guess and the
        // true selected tiles can diverge. Only request offsets when there's
        // an actual region to look up — on an empty region this is exactly
        // `encode_with_stats`'s cost, no wasted allocation.
        let want_offsets = !region.tiles.is_empty();
        if let Some(worker) = nvenc.as_ref() {
            match shared_handle {
                Some(handle) => worker.send_texture_request(handle, is_keyframe),
                None => worker.send_request(Arc::new(cap_buf.clone()), is_keyframe),
            }
        }

        // Live-found (static-desktop freeze investigation): letting the
        // codec race pick EVRTCK mid-H265-session sends that frame as a
        // plain (non-IS_SILICON) VideoFrame, which the Android client
        // routes to the separate `evrt2Preview` bitmap overlay instead of
        // the H265 MediaCodec/GL surface — and since EVRTCK's own delta
        // chain only advances on frames it actually WON (the client never
        // applies deltas for frames NVENC won instead), an EVRTCK win
        // right after one or more NVENC wins decodes against a `prev` the
        // client never saw, corrupting the overlay. In this H265 test
        // specifically the overlay then sits on top of the screen (its
        // EVRT2ONLY layout is MATCH_PARENT) showing that corruption —
        // this IS the black/frozen-picture bug chased across many rounds
        // today. The point of this test is H265 itself, so there's no
        // reason to ever race EVRTCK in here: trust NVENC unconditionally
        // whenever it's up, bypassing the trust-window countdown entirely
        // (that countdown still governs the general H264 racing path
        // below, untouched, for non-H265 sessions).
        // `mf_encoder` only ever gets set when NVENC is unavailable (see
        // the calibration block above), so `nvenc.is_some()` and
        // `mf_encoder.is_some()` are mutually exclusive in practice —
        // combining them here just means "some hardware encoder is
        // active," regardless of which vendor. There's no meaningful
        // "race" to run against a synchronous CPU-call encoder like MF's
        // (no worker thread, no parallelism to measure), so it's always
        // forced too, exactly like the NVENC case.
        let nvenc_forced_for_h265 = evrt2_want_h265 && (nvenc.is_some() || mf_encoder.is_some());
        let skip_evrtck_race =
            nvenc_forced_for_h265 || (trust_nvenc_frames_left > 0 && nvenc.is_some());
        let debug_log_this_frame = frame_id % 30 == 0;
        let evrtck_start = Instant::now();
        let (packet, motion_ratio_direct, tile_offsets, evrtck_cost_ms) = if skip_evrtck_race {
            // Cheap diff-only scan (no compression) — just enough for the
            // mode selector's `motion_ratio` signal, without paying for
            // the full tile-diff + entropy-coding pass this frame.
            let ratio = enc.dirty_ratio(&cap_buf);
            (None, ratio, Vec::new(), 0.0)
        } else {
            let (packet, stats, tile_offsets) = if want_offsets {
                enc.encode_with_offsets(&cap_buf, frame_id)
            } else {
                let (packet, stats) = enc.encode_with_stats(&cap_buf, frame_id);
                (packet, stats, Vec::new())
            };
            let cost_ms = evrtck_start.elapsed().as_secs_f32() * 1000.0;
            (Some(packet), stats.dirty_ratio(), tile_offsets, cost_ms)
        };
        let evrtck_finish = Instant::now();

        // Bounded wait — the bound only guards against a dead worker
        // thread; a healthy NVENC encode finishes in low single-digit
        // milliseconds, nowhere near this budget.
        let nvenc_wait_start = Instant::now();
        let (evrtck_finish, nvenc_finish, nvenc_packet, nvenc_cost_ms) =
            if let Some(worker) = nvenc.as_ref() {
                match worker.recv_result(Duration::from_millis(500)) {
                    Some(NvencWorkerReply::Encoded {
                        finish,
                        elapsed,
                        result: Ok(Some(pkt)),
                    }) => (
                        evrtck_finish,
                        Some(finish),
                        Some(pkt),
                        Some(elapsed.as_secs_f32() * 1000.0),
                    ),
                    Some(NvencWorkerReply::Encoded {
                        result: Ok(None), ..
                    }) => {
                        (evrtck_finish, None, None, None) // encoder buffering internally, not an error
                    }
                    Some(NvencWorkerReply::Encoded { result: Err(e), .. }) => {
                        log(
                            &events,
                            format!("EVRT2 (experimental): NVENC encode failed: {e}"),
                        );
                        nvenc = None; // worker likely broken — drop it, next tick spawns a fresh one
                        trust_nvenc_frames_left = 0;
                        consecutive_nvenc_wins = 0;
                        nvenc_trust_established = false;
                        (evrtck_finish, None, None, None)
                    }
                    None => {
                        log(
                            &events,
                            "EVRT2 (experimental): NVENC worker timed out — dropping it".to_owned(),
                        );
                        nvenc = None;
                        trust_nvenc_frames_left = 0;
                        consecutive_nvenc_wins = 0;
                        nvenc_trust_established = false;
                        (evrtck_finish, None, None, None)
                    }
                }
            } else if let Some(mf) = mf_encoder.as_mut() {
                // Synchronous (no dedicated worker thread — see `mf_encoder`'s
                // own doc comment for why: `MfVideoEncoder::encode_bgra` runs
                // ProcessInput/ProcessOutput on this thread directly, and
                // there's nothing to overlap it against since EVRTCK's own
                // encode is always skipped whenever this path is forced
                // active, same as the NVENC-forced case).
                let mf_start = Instant::now();
                match mf.encode_bgra(&cap_buf, is_keyframe) {
                    Ok(Some(pkt)) => (
                        evrtck_finish,
                        Some(Instant::now()),
                        Some(pkt),
                        Some(mf_start.elapsed().as_secs_f32() * 1000.0),
                    ),
                    Ok(None) => (evrtck_finish, None, None, None), // encoder buffering internally, not an error
                    Err(e) => {
                        log(
                            &events,
                            format!("EVRT2 (experimental): Media Foundation encode failed: {e}"),
                        );
                        mf_encoder = None; // encoder likely broken — drop it, next tick spawns a fresh one
                        trust_nvenc_frames_left = 0;
                        consecutive_nvenc_wins = 0;
                        nvenc_trust_established = false;
                        (evrtck_finish, None, None, None)
                    }
                }
            } else {
                (evrtck_finish, None, None, None)
            };
        let nvenc_wait_ms = nvenc_wait_start.elapsed().as_secs_f32() * 1000.0;
        if debug_log_this_frame {
            log(&events, format!(
                "EVRT2 DEBUG adaptive-race: skip={} trust_left={} consecutive_wins={} capture_cost_ms={:.2} attn_cost_ms={:.2} nvenc_wait_ms={:.2} iter_gap_ms={:?}",
                skip_evrtck_race, trust_nvenc_frames_left, consecutive_nvenc_wins, capture_cost_ms, attn_cost_ms, nvenc_wait_ms, iter_gap_ms
            ));
        }
        frame_id = frame_id.wrapping_add(1);

        let use_nvenc = if skip_evrtck_race {
            // Trusting NVENC this frame — no race ran, so there's nothing
            // for schedule()'s marginal-utility test to compare against.
            // Bookkeeping for the trust window itself: expire it, and if
            // it just expired, force EVRTCK's NEXT real encode to be an
            // absolute keyframe — its internal `prev` buffer is stale
            // relative to whatever NVENC has been sending, so a delta
            // against it would XOR against the wrong baseline (the same
            // desync class as the Phase 6.1 client-decoder bug, just on
            // the host's own encoder this time).
            //
            // Skipped entirely under `nvenc_forced_for_h265`: that path
            // never lets EVRTCK's own encode run at all (see this session's
            // doc comment above `skip_evrtck_race`), so there's no future
            // EVRTCK send to prime a keyframe for, and this countdown
            // would otherwise underflow once it reaches 0 while still
            // being held true by the forced-H265 condition.
            if !nvenc_forced_for_h265 {
                trust_nvenc_frames_left -= 1;
                if trust_nvenc_frames_left == 0 {
                    enc.request_keyframe();
                }
            }
            nvenc_packet.is_some()
        } else {
            // ROADMAP.md Phase 6.1: feed CapabilityRegistry with real
            // measured costs and let schedule() make a real
            // marginal-utility decision.
            registry.register_provider(Provider {
                id: PROVIDER_CPU_EVRTCK.to_owned(),
                capability: Capability::RoiEncoding,
                cost_ms: evrtck_cost_ms,
                quality: 1.0, // EVRTCK is lossless
            });
            if let Some(nvenc_cost_ms) = nvenc_cost_ms {
                registry.register_provider(Provider {
                    id: PROVIDER_NVENC_H264.to_owned(),
                    capability: Capability::RoiEncoding,
                    cost_ms: nvenc_cost_ms,
                    quality: 0.85, // lossy, unlike EVRTCK's 1.0 — an honest approximation, not measured perceptually
                });
            }
            // First Light gain metric — EVRT2CKMAX.md's own "measure the
            // win by time to first signal" framing (ROADMAP.md 6.3
            // acceptance criterion), same shape as the existing EVRT-Gain
            // proxy (Phase 1.5) rather than a new ad-hoc number. Only
            // meaningful once both providers actually produced a real
            // result this frame.
            if let (Some(nv_finish), Some(_)) = (nvenc_finish, nvenc_packet.as_ref()) {
                let nvenc_won = nv_finish < evrtck_finish;
                let (winner, gain) = if nvenc_won {
                    (
                        "NVENC_H264",
                        evrtck_finish.saturating_duration_since(nv_finish),
                    )
                } else {
                    (
                        "CPU_EVRTCK",
                        nv_finish.saturating_duration_since(evrtck_finish),
                    )
                };
                codec_race_samples += 1;
                if nvenc_won {
                    codec_race_nvenc_wins += 1;
                    consecutive_nvenc_wins += 1;
                } else {
                    consecutive_nvenc_wins = 0;
                }
                // ROADMAP.md Phase 6.3′: NVENC has proven itself reliably
                // faster over a real streak, not just one lucky frame —
                // stop paying EVRTCK's full cost every frame for a while.
                // First time this session: the full bar. After that: a
                // lighter reconfirmation, not a fresh proof from scratch —
                // see this session's own doc comment above for why.
                let required_wins = if nvenc_trust_established {
                    NVENC_TRUST_RENEWAL_THRESHOLD
                } else {
                    NVENC_TRUST_THRESHOLD
                };
                if consecutive_nvenc_wins >= required_wins {
                    consecutive_nvenc_wins = 0;
                    trust_nvenc_frames_left = NVENC_TRUST_WINDOW;
                    nvenc_trust_established = true;
                }
                if codec_race_samples % 60 == 0 {
                    log(&events, format!(
                        "EVRT2 (experimental): Codec Race (First Light) — {winner} first this frame by {:.2}ms; NVENC won {}/{} races so far; evrtck_cost_ms={:.2} nvenc_cost_ms={:?}",
                        gain.as_secs_f32() * 1000.0, codec_race_nvenc_wins, codec_race_samples, evrtck_cost_ms, nvenc_cost_ms
                    ));
                }
            }
            // The actual marginal-utility decision (SDUDP.md/EVRT2CKMAX.md's
            // own test, same `schedule()` Phase 6.2 already uses for
            // rebalance()): only switch to NVENC if it's both ready THIS
            // frame and genuinely cheaper than EVRTCK's own just-measured
            // cost.
            nvenc_packet.is_some()
                && registry
                    .schedule(
                        Capability::RoiEncoding,
                        1000.0 / EXPERIMENT_FPS as f32,
                        evrtck_cost_ms,
                    )
                    .as_deref()
                    == Some(PROVIDER_NVENC_H264)
        };

        // Trust-window frame with no NVENC packet ready (rare — encoder
        // buffering internally) and no EVRTCK fallback computed this frame
        // either: nothing worth sending, same treatment as a capture miss.
        if !use_nvenc && packet.is_none() {
            continue;
        }

        let visible_ranges: Vec<(usize, usize)> = if region.tiles.is_empty() {
            Vec::new()
        } else {
            let selected: std::collections::HashSet<u16> = region.tiles.iter().copied().collect();
            tile_offsets
                .iter()
                .filter(|off| selected.contains(&off.tile_idx))
                .map(|off| (off.byte_start, off.byte_start + off.byte_len))
                .collect()
        };

        // ROADMAP.md Phase 6.1 step 3 / Phase 6.4: when NVENC is the chosen
        // path AND the Attention Map actually produced a visible region
        // this frame, splice — send NVENC's whole-frame bytes as a lossy
        // background PLUS an absolute-encoded EVRTCK overlay for exactly
        // the visible-region tiles (Phase 6.4's `encode_tile_subset_absolute`
        // / `EvrtckDecoder::apply_absolute_overlay`), closing the honest
        // Phase 6.1 gap where the silicon path carried no VISIBLE_REGION
        // guarantee at all. The overlay is built from `cap_buf` — the SAME
        // real captured pixels this frame's EVRTCK encode already ran
        // against — not NVENC's lossy reconstruction of them, so it's
        // exact, not an approximation of an approximation. When NVENC wins
        // but the region is empty (no Attention Map signal this frame),
        // there is nothing meaningful to overlay — falls back to the plain
        // Phase 6.1 pure-silicon send, same as before this phase existed.
        let send_frame_start = Instant::now();
        let (send_result, did_send_visible_region) =
            if let (true, Some(nv_pkt)) = (use_nvenc, nvenc_packet.as_ref()) {
                if !region.tiles.is_empty() {
                    let overlay = crate::evrtck::encode_tile_subset_absolute(
                        &cap_buf,
                        w as usize,
                        h as usize,
                        frame_id,
                        &region.tiles,
                    );
                    let (spliced, overlay_range) = build_spliced_payload(&nv_pkt.bytes, &overlay);
                    spliced_frames += 1;
                    (
                        session.send_frame(
                            &spliced,
                            frame_id,
                            nv_pkt.key,
                            true,
                            evrt2_want_h265,
                            &[overlay_range],
                        ),
                        true,
                    )
                } else {
                    (
                        session.send_frame(
                            &nv_pkt.bytes,
                            frame_id,
                            nv_pkt.key,
                            true,
                            evrt2_want_h265,
                            &[],
                        ),
                        false,
                    )
                }
            } else {
                // Guaranteed `Some` here — the early `continue` above already
                // ruled out `!use_nvenc && packet.is_none()`.
                let packet = packet
                    .as_ref()
                    .expect("packet present when not using nvenc");
                (
                    session.send_frame(
                        &packet.data,
                        frame_id,
                        is_keyframe,
                        false,
                        false,
                        &visible_ranges,
                    ),
                    !visible_ranges.is_empty(),
                )
            };
        let send_frame_cost_ms = send_frame_start.elapsed().as_secs_f32() * 1000.0;
        if debug_log_this_frame {
            log(&events, format!(
                "EVRT2 DEBUG frame-breakdown: apf_cost_ms={:.2} send_frame_cost_ms={:.2} total_frame_ms={:.2}",
                apf_cost_ms, send_frame_cost_ms, capture_start.elapsed().as_secs_f32() * 1000.0
            ));
        }
        if let Err(e) = send_result {
            log(
                &events,
                format!("EVRT2 (experimental): send_frame error: {e}"),
            );
            break;
        }
        if spliced_frames > 0 && spliced_frames % 60 == 0 {
            log(&events, format!(
                "EVRT2 (experimental): Phase 6.4 splice — {spliced_frames} spliced frames sent so far (NVENC background + EVRTCK visible-region overlay)"
            ));
        }

        // Task 01 § Breach Handling — ROADMAP.md Phase 1.3: now actually on
        // the wire (was log-only). Honest measurement either way: this only
        // reports what was actually measured, never fabricates a region or
        // an age — see TASK-01's "must not become an excuse to fabricate".
        // ROADMAP.md Phase 6.1/6.4: skipped only when NEITHER a plain
        // EVRTCK frame NOR a spliced overlay actually carried the visible
        // region on the wire this frame (pure-silicon-with-no-region case)
        // — `did_send_visible_region` is exact about which of the three
        // send paths above actually happened, not an assumption.
        if did_send_visible_region {
            let measured_age = send_start.elapsed();
            if let Some(breach) = check_breach(active_mode, region.clone(), measured_age) {
                let _ = session.send_degrade_signal(
                    &region.tiles,
                    breach.measured_age.as_micros() as u32,
                    breach.ceiling.as_micros() as u32,
                );
                if last_breach_log.elapsed() >= Duration::from_secs(2) {
                    last_breach_log = Instant::now();
                    log(&events, format!(
                        "EVRT2 (experimental): Task01 breach — visible region age {:?} > ceiling {:?}",
                        breach.measured_age, breach.ceiling
                    ));
                }
            }
        }

        // ROADMAP.md Phase 2.1/2.4: AR2R47 mode evaluation. `motion_ratio`
        // reuses the dirty-tile ratio this frame's encode already computed
        // — either from the full tile-diff pass (`FrameStats::dirty_ratio`)
        // or, during an adaptive-race trust window (Phase 6.3′), from the
        // cheap standalone `EvrtckEncoder::dirty_ratio` scan — either way
        // no second full-frame diff pays twice for the same signal.
        let motion_ratio = motion_ratio_direct;
        const MOTION_LOW_THRESHOLD: f32 = 0.30; // matches evrt2_modes' AR<->2R boundary
        if motion_ratio < MOTION_LOW_THRESHOLD {
            if low_motion_since.is_none() {
                low_motion_since = Some(Instant::now());
            }
        } else {
            low_motion_since = None;
        }
        let idle_duration = low_motion_since
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO);

        let signals = ModeSignals {
            motion_ratio,
            idle_duration,
            // game_detected: still honestly false — no game-process
            // detector exists in this experimental path.
            game_detected: false,
            // silicon_available: ROADMAP.md Phase 6.1 steps 3-4 made this
            // real — an open NVENC session now genuinely means silicon
            // frames can be sent (is_silicon=true, real H264 bytes) AND
            // decoded (the client's h264_decoder). Before steps 3-4 this
            // was deliberately left false even with a session open,
            // because Mode47 asserts "this stream is silicon-encoded" and
            // nothing was actually switching yet — that would have been a
            // lie on the wire, not just an incomplete feature. Now it
            // isn't: whenever `nvenc` is `Some`, this session CAN honor
            // Mode47's own contract.
            silicon_available: nvenc.is_some(),
            bandwidth_bps,
            user_requested_game_mode: false,
        };
        if let Some((new_mode, reason)) = mode_selector.evaluate(&signals) {
            match session.send_mode_switch(new_mode, reason) {
                Ok(()) => {
                    session.set_mode(new_mode);
                    mode_selector.apply(new_mode);
                    log(
                        &events,
                        format!(
                            "EVRT2 (experimental): MODE_SWITCH {:?} → {:?} ({:?})",
                            active_mode, new_mode, reason
                        ),
                    );
                }
                Err(e) => {
                    log(
                        &events,
                        format!("EVRT2 (experimental): MODE_SWITCH send failed: {e}"),
                    );
                }
            }
        }
    }
}

/// ROADMAP.md Phase 5.2 — SDUDP.md § Path Probing: sends the SAME HELLO
/// datagram to every candidate simultaneously from one already-bound
/// socket, then returns whichever address answers with SESSION_ACK first.
/// LAN and WAN/STUN candidates race on equal footing — no preference is
/// given to list order, unlike the old `candidates.first()` behavior this
/// replaces. Packets from an address not in `candidates` are ignored (not a
/// real security boundary — AuthTag isn't active yet at this point in the
/// handshake — just noise rejection).
fn race_hello_candidates(
    socket: &UdpSocket,
    candidates: &[std::net::SocketAddr],
    mode: Mode,
    max_fps: u32,
    max_res: (u32, u32),
    stop: &Arc<AtomicBool>,
    on_status: &mut impl FnMut(String),
) -> Option<std::net::SocketAddr> {
    match race_hello_all_paths(
        socket, candidates, None, mode, max_fps, max_res, stop, on_status,
    ) {
        Some(RaceWinner::Udp(addr)) => Some(addr),
        Some(RaceWinner::Relay) => unreachable!("relay=None can never produce RaceWinner::Relay"),
        None => None,
    }
}

/// ROADMAP.md Phase 5.3: which candidate kind answered first.
#[derive(Debug)]
enum RaceWinner {
    Udp(std::net::SocketAddr),
    Relay,
}

/// Extends `race_hello_candidates`'s UDP-only race (Phase 5.2) with a relay
/// candidate racing alongside it — SDUDP.md § Path Probing names exactly
/// these three candidate kinds: LAN, public/STUN, and relay tunnel; this is
/// the last one joining the race. `relay` is `(outbound, inbound)`: the
/// same HELLO datagram sent to every UDP candidate is also handed to
/// `outbound` for the caller to wrap as `Misc::Evrt2RelayWrap` and send
/// over its TCP relay stream; `inbound` is polled alongside the UDP socket
/// for the matching ACK. `None` means "no relay channel available" and
/// this behaves exactly like the old UDP-only `race_hello_candidates`.
fn race_hello_all_paths(
    socket: &UdpSocket,
    candidates: &[std::net::SocketAddr],
    relay: Option<&(Sender<Vec<u8>>, Receiver<Vec<u8>>)>,
    mode: Mode,
    max_fps: u32,
    max_res: (u32, u32),
    stop: &Arc<AtomicBool>,
    on_status: &mut impl FnMut(String),
) -> Option<RaceWinner> {
    let hello = build_hello(mode, max_fps, max_res, &[]);
    for &addr in candidates {
        let _ = socket.send_to(&hello, addr);
    }
    let relay_send_ok = if let Some((relay_out, _)) = relay {
        relay_out.send(hello.clone()).is_ok()
    } else {
        false
    };
    evrt2log!(
        "[evrt2] race_hello_all_paths: sent HELLO ({} bytes) to {} UDP candidate(s), relay={:?} (send_ok={relay_send_ok})",
        hello.len(), candidates.len(), relay.is_some()
    );
    on_status(format!(
        "EVRT2 (experimental): HELLO разослан {} UDP-кандидат(ам){} — ждём первый ответ…",
        candidates.len(),
        if relay.is_some() { " + relay" } else { "" }
    ));
    let deadline = Instant::now() + RACE_TIMEOUT;
    let mut buf = [0u8; 1500];
    let mut relay_bytes_seen: u64 = 0;
    let mut udp_bytes_seen: u64 = 0;
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            evrt2log!("[evrt2] race_hello_all_paths: stop flag set, aborting race");
            return None;
        }
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                udp_bytes_seen += 1;
                if candidates.contains(&from) {
                    if let Ok((header, _payload)) =
                        crate::evrt2_packet::PacketHeader::decode(&buf[..len])
                    {
                        if header.packet_type == crate::evrt2_packet::PacketType::SessionAck {
                            evrt2log!("[evrt2] race_hello_all_paths: UDP winner {from}");
                            return Some(RaceWinner::Udp(from));
                        }
                    }
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => {}
        }
        if let Some((_, relay_in)) = relay {
            if let Ok(raw) = relay_in.try_recv() {
                relay_bytes_seen += 1;
                evrt2log!("[evrt2] race_hello_all_paths: relay_in received {} bytes (#{relay_bytes_seen})", raw.len());
                if let Ok((header, _payload)) = crate::evrt2_packet::PacketHeader::decode(&raw) {
                    evrt2log!(
                        "[evrt2] race_hello_all_paths: relay packet decoded, type={:?}",
                        header.packet_type
                    );
                    if header.packet_type == crate::evrt2_packet::PacketType::SessionAck {
                        evrt2log!("[evrt2] race_hello_all_paths: relay winner");
                        return Some(RaceWinner::Relay);
                    }
                } else {
                    evrt2log!("[evrt2] race_hello_all_paths: relay bytes failed to decode as a packet header");
                }
            }
        }
    }
    evrt2log!(
        "[evrt2] race_hello_all_paths: timed out after {:?} — udp_bytes_seen={udp_bytes_seen} relay_bytes_seen={relay_bytes_seen}",
        RACE_TIMEOUT
    );
    None
}

/// ROADMAP.md Phase 5.4 — real switch, UDP→relay only. `session.is_relay()`
/// guards against trying this when already relay-backed — there's no
/// further fallback wired up for a relay path that itself degrades (honest
/// gap, see ROADMAP.md). Only attempted when the initial race's relay
/// channel pair is still around (`relay: Some(...)` — the UDP side won the
/// original race but the relay leg itself never got torn down, exactly the
/// guarantee `run_client_experiment`'s own doc on `relay` establishes).
///
/// Shared by both the organic degradation trigger (`RttEstimator::on_sample`
/// returning true) and the `EVRTDESK_EVRT2_FORCE_SWITCH_AFTER` debug hook
/// (see `run_client_experiment`) — same reasoning as Phase 5.3's
/// `EVRTDESK_EVRT2_FORCE_RELAY`: a real network degradation is awkward to
/// reproduce on demand, so a debug trigger exercises the exact same code
/// path live instead of only in a loopback unit test.
fn attempt_switch_to_relay(
    session: &mut Evrt2Session,
    relay: &mut Option<(Sender<Vec<u8>>, Receiver<Vec<u8>>)>,
    rtt_est: &mut crate::evrt2_rtt::RttEstimator,
    on_status: &mut impl FnMut(String),
) {
    if session.is_relay() {
        return;
    }
    let Some((relay_out, relay_in)) = relay.as_ref() else {
        on_status("EVRT2 (experimental): деградация обнаружена, но relay-канал недоступен — переключаться некуда".to_owned());
        return;
    };
    // Only `&self` needed to attempt — ownership is taken (via
    // `relay.take()`) ONLY once an ACK actually confirms the switch, so a
    // failed attempt (transient blip, relay itself briefly unavailable)
    // doesn't burn the fallback for good; the next sustained degradation
    // sample gets to try again.
    let hello = build_hello(Mode::Ar, 60, (1920, 1080), &[]);
    let mut got_ack = false;
    if relay_out.send(hello).is_ok() {
        // Bounded wait — this blocks the main receive loop (no video frames
        // processed) for up to this long. The path is already confirmed
        // degraded at this point, so a brief stall to attempt recovery is
        // an honest trade-off, not a hidden cost.
        let switch_deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < switch_deadline {
            if let Ok(raw) = relay_in.try_recv() {
                if let Ok((h, _)) = crate::evrt2_packet::PacketHeader::decode(&raw) {
                    if h.packet_type == crate::evrt2_packet::PacketType::SessionAck {
                        got_ack = true;
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if got_ack {
        let (relay_out, relay_in) = relay.take().expect("checked Some above");
        let placeholder_peer: std::net::SocketAddr = "0.0.0.0:0".parse().unwrap();
        session.switch_to_relay(relay_out, relay_in, placeholder_peer);
        // A genuinely different path now — the old baseline no longer
        // describes it.
        rtt_est.reset_baseline();
        on_status("EVRT2 (experimental): переключение на relay выполнено — путь UDP деградировал (Phase 5.4)".to_owned());
    } else {
        on_status("EVRT2 (experimental): попытка переключения на relay не удалась (нет ACK за 1.5с) — остаёмся на текущем пути".to_owned());
    }
}

/// Client-side counterpart, symmetric to `run_host_experiment`: races HELLO
/// against every UDP candidate AND (ROADMAP.md Phase 5.3) the relay
/// candidate if the caller supplied one, then receives+reassembles+decodes
/// frames and hands them to the caller via the callback — same shape as
/// `SessionEvent::Frame` so callers can feed the existing render path
/// unchanged. `relay` is `None` for callers with no TCP relay channel wired
/// (pure UDP racing, Phase 5.2 behavior); `Some((outbound, inbound))`
/// otherwise — see `race_hello_all_paths` for exactly what each side does
/// with it.
pub fn run_client_experiment(
    candidates: Vec<std::net::SocketAddr>,
    mut relay: Option<(Sender<Vec<u8>>, Receiver<Vec<u8>>)>,
    auth_key: Option<crate::evrt2_crypto::SessionKey>,
    stop: Arc<AtomicBool>,
    mut on_frame: impl FnMut(u32, u32, Vec<u8>), // (width, height, rgba)
    mut on_status: impl FnMut(String),
    // ROADMAP.md Phase 1.3: a structured DEGRADE_SIGNAL event, separate
    // from `on_status`'s free-text log line — lets a caller render an
    // actual visual indicator (see `evrt2_preview_window` in main.rs)
    // instead of string-matching a log message, which would be fragile
    // (locale-dependent text, format drift). Called IN ADDITION to
    // `on_status`, not instead of it — the text log line stays for callers
    // (e.g. the current Android client) that don't yet consume this.
    mut on_degrade: impl FnMut(std::time::Duration, std::time::Duration, usize), // (measured_age, ceiling, tile_count)
) {
    evrt2log!(
        "[evrt2] run_client_experiment: entry, {} UDP candidate(s), relay={}",
        candidates.len(),
        relay.is_some()
    );
    if candidates.is_empty() && relay.is_none() {
        on_status("EVRT2 (experimental): нет кандидатов для подключения".to_owned());
        return;
    }
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            on_status(format!("EVRT2 (experimental): bind failed: {e}"));
            return;
        }
    };
    if let Err(e) = socket.set_read_timeout(Some(Duration::from_millis(200))) {
        on_status(format!(
            "EVRT2 (experimental): set_read_timeout failed: {e}"
        ));
        return;
    }
    // Live-found (chasing EVRT2's fps gap vs the live EVRT1 pipeline, see
    // `evrt2_experiment.rs`'s `client_downscale_target` doc for the full
    // story): the client's HELLO used to hardcode `(1920, 1080)` here
    // regardless of the phone's actual screen — `evrt_client::max_resolution`
    // reads back the SAME real device size (`dm.widthPixels`/`heightPixels`)
    // `MainActivity.kt` already reports to EVRT1 via `setMaxResolution`
    // before any connection, letting the host's downscale target the
    // client's real screen instead of an arbitrary Full-HD guess. Falls
    // back to the old `(1920, 1080)` default only if that was never called
    // (e.g. non-Android builds/tests).
    let client_max_res = {
        let real = crate::evrt_client::max_resolution();
        if real == (0, 0) {
            (1920, 1080)
        } else {
            real
        }
    };
    // ROADMAP.md Phase 2: must match run_host_experiment's own start mode
    // (Mode::Ar) — the host's first VIDEO_FRAME packet carries that mode in
    // its header regardless of what the client assumes, but starting in
    // sync means the client's initial jitter-buffer profile (below) is
    // correct from frame one instead of one MODE_SWITCH behind.
    let winner = race_hello_all_paths(
        &socket,
        &candidates,
        relay.as_ref(),
        Mode::Ar,
        60,
        client_max_res,
        &stop,
        &mut on_status,
    );
    let mut session = match winner {
        Some(RaceWinner::Udp(host_addr)) => {
            on_status(format!(
                "EVRT2 (experimental): путь выбран — UDP {host_addr}"
            ));
            match Evrt2Session::from_bound_socket(socket, host_addr, Mode::Ar) {
                Ok(s) => s,
                Err(e) => {
                    on_status(format!("EVRT2 (experimental): session setup failed: {e}"));
                    return;
                }
            }
        }
        Some(RaceWinner::Relay) => {
            on_status(
                "EVRT2 (experimental): путь выбран — relay (RELAY_WRAP, Phase 5.3)".to_owned(),
            );
            let (relay_out, relay_in) = relay
                .take()
                .expect("RaceWinner::Relay implies relay channels were provided");
            let placeholder_peer: std::net::SocketAddr = "0.0.0.0:0".parse().unwrap();
            Evrt2Session::from_relay_channels(relay_out, relay_in, placeholder_peer, Mode::Ar)
        }
        None => {
            on_status(format!(
                "EVRT2 (experimental): ни один путь ({} UDP-кандидат(ов){}) не ответил за {}с",
                candidates.len(),
                if relay.is_some() { " + relay" } else { "" },
                RACE_TIMEOUT.as_secs()
            ));
            return;
        }
    };
    // The race's own HELLO already reached this peer and got ACKed — that's
    // what won the race — so the handshake is already complete here. Matches
    // the host's own set-after-ACK timing (ROADMAP.md Phase 4.2/4.3): both
    // sides only apply the AuthTag/encryption key once ACK is confirmed.
    session.set_auth_key(auth_key);
    let mut got_ack = true;
    on_status("EVRT2 (experimental): подключено ✓".to_owned());
    // ROADMAP.md Phase 5.4 — debug-only test hook (same reasoning as Phase
    // 5.3's EVRTDESK_EVRT2_FORCE_RELAY): a real sustained RTT degradation
    // is awkward to reproduce on demand, so this forces a switch attempt
    // through the EXACT same `attempt_switch_to_relay` code path a real
    // detected degradation would use, N seconds after connecting — proves
    // the live wire round trip (client HELLO over relay → host ACK → both
    // sides swap transport → streaming continues) without waiting on a
    // real network to actually degrade. Not meant to stay set for a normal
    // session.
    let force_switch_after: Option<Duration> =
        std::env::var("EVRTDESK_EVRT2_FORCE_SWITCH_AFTER_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
    let connected_at = Instant::now();
    let mut force_switch_fired = false;

    let mut reassembler = FrameReassembler::new();
    let mut decoder = crate::evrtck::EvrtckDecoder::new();
    // ROADMAP.md Phase 6.1 step 4: decodes IS_SILICON-flagged frames
    // (real NVENC-encoded H264, see the host-side step 3 code in this same
    // file). `openh264` is the same software decoder the live EVRT1 client
    // already uses on both PC and Android (`transport.rs`'s
    // `decode_h264_rgba`) — reused here rather than routing through
    // Android's MediaCodec/Surface path, which has no Rust-side pixel
    // buffer to hand back to `on_frame` at all (MediaCodec renders straight
    // to a `Surface`). `openh264::decoder::Decoder::new()` failing (e.g. the
    // shared library genuinely isn't available) leaves this `None` — a
    // silicon frame arriving in that case is simply dropped, same as any
    // other malformed/undecodable input this loop already tolerates.
    let mut h264_decoder = openh264::decoder::Decoder::new().ok();
    // ROADMAP.md Phase 6.2: what this client actually reports as
    // `silicon_ok` in its own FEEDBACK — real decode-side health, not a
    // hardcoded value. Optimistic default (no evidence of trouble yet);
    // flips false on a genuine decode error, true again on the next
    // successful decode. `h264_decoder.is_none()` (the software decoder
    // itself failed to initialize) is handled separately at the send site
    // below — that's a permanent "no" regardless of this flag.
    let mut silicon_decode_healthy = true;
    // ROADMAP.md Phase 3.1: latest decoded Attention Map (raster P_i values)
    // — currently only logged as proof of a genuine round trip (highest and
    // lowest tile, tile count).
    let mut apf_updates_received: u64 = 0;
    // ROADMAP.md Phase 3.2: this client's own baseline for decoding a
    // delta APF against — mirrors the host's `last_sent_apf`. Reset to
    // `None` whenever a full snapshot's dimensions don't match it (a
    // resolution change), same as the host's own reset rule.
    let mut last_apf: Option<Vec<f32>> = None;
    // ROADMAP.md Phase 3.3: the header that came with `last_apf` — needed
    // alongside the map itself to translate an EVRTCK tile index (native
    // 32px grid) into the matching APF cell index, since the APF grid can
    // be coarser (`fit_scale_for_budget` on the host side).
    let mut last_apf_header: Option<crate::evrt2_apf::ApfHeader> = None;
    // ROADMAP.md Phase 3.3 profiling counters (see the decode loop below).
    let mut apf_priority_profile_samples: u64 = 0;
    let mut apf_priority_profile_hits: u64 = 0;
    let mut frames_received: u64 = 0;
    let mut last_activity = Instant::now();
    let mut last_keepalive_sent = Instant::now();

    // Adaptive jitter buffer (SDUDP.md § 2) — delays non-visible-region
    // frames by the measured jitter-derived buffer_depth; frames carrying
    // any VISIBLE_REGION-flagged packet bypass this entirely (Task 01 § 3).
    let mut jitter_est = JitterEstimator::new(
        ModeProfile::from_evrt2_mode(Mode::Ar),
        Duration::from_secs_f32(1.0 / EXPERIMENT_FPS as f32),
    );
    let mut tracking_frame_id: Option<u32> = None;
    let mut frame_has_visible = false;
    let mut last_frame_id_seen: Option<u32> = None;
    let mut dropped_frames: u64 = 0;
    let mut fps_window_start = Instant::now();
    let mut fps_window_count: u32 = 0;
    let mut decoded_fps = 0.0f32;
    // ROADMAP.md Phase 1.4: request a fresh keyframe on decode failure
    // instead of silently limping along until the next scheduled one (up to
    // KEYFRAME_INTERVAL away on the host). Rate-limited so a sustained
    // decode failure doesn't turn into an IDR_REQUEST flood.
    let mut last_idr_request = Instant::now() - Duration::from_secs(10);
    const IDR_REQUEST_COOLDOWN: Duration = Duration::from_millis(500);

    // ROADMAP.md Phase 1.5: first live proxy for EVRT Gain (EVRT2CKMAX.md
    // § Success metric). Not the full Σ P_i × age_i formula — the client
    // only sees two classes (visible / not), not per-tile P_i — but a
    // measurable, honestly-labeled two-class approximation:
    //   uniform_age  = arrival(last packet of frame) - arrival(first packet)
    //   visible_age  = arrival(first VISIBLE_REGION packet) - arrival(first packet)
    //   gain         = uniform_age - visible_age
    let mut frame_start_arrival: Option<Instant> = None;
    let mut visible_arrival: Option<Instant> = None;
    let mut gain_sum_us: u64 = 0;
    let mut gain_samples: u64 = 0;

    // ROADMAP.md Phase 5.4: real RTT measurement (KEEPALIVE ping/pong, see
    // `evrt2_session::build_keepalive_ping`) — replaces the honest gap this
    // comment used to flag (`rtt_us: 0`, "no RTT probe implemented for this
    // experimental path yet"). A tighter cadence than the FEEDBACK keepalive
    // above so `RttEstimator` gets enough samples to react before the
    // picture visibly stalls.
    const RTT_PING_INTERVAL: Duration = Duration::from_millis(500);
    let mut last_rtt_ping_sent = Instant::now() - RTT_PING_INTERVAL;
    let mut rtt_est = crate::evrt2_rtt::RttEstimator::new();
    let mut last_measured_rtt_us: u32 = 0;
    // Edge-triggered log only — `RttEstimator::on_sample` itself already
    // only returns `true` once per degradation event, so no extra
    // debouncing state is needed here beyond what it already does.

    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = session.send_goodbye();
            on_status("EVRT2 (experimental): остановлено пользователем".to_owned());
            return;
        }
        if last_activity.elapsed() > IDLE_TIMEOUT {
            on_status("EVRT2 (experimental): нет данных от хоста — остановлено".to_owned());
            return;
        }
        if last_keepalive_sent.elapsed() > KEEPALIVE_INTERVAL {
            last_keepalive_sent = Instant::now();
            let _ = session.send_feedback(&ReceiverFeedback2 {
                frame_id: frames_received as u32,
                // No backpressure signal is actually measured here (no
                // encode-side queue on the receiving end) — left honest at
                // 0.0 rather than fabricated.
                pressure: 0.0,
                jitter_p95_us: jitter_est.jitter_p95().as_micros() as u32,
                decoded_fps,
                // ROADMAP.md Phase 6.2: real decode-side health now that
                // there's an actual silicon decode path to have an opinion
                // about (Phase 6.1 step 4). ROADMAP.md Phase 6.3′ H265 A/B
                // test: `silicon_decode_healthy` is now shared across both
                // codec branches (H264 via `openh264`, H265 via Android's
                // MediaCodec Surface decode) and is set `false` by EITHER
                // branch's own "decoder unavailable" case, so dropping the
                // old `h264_decoder.is_some()` gate here doesn't lose that
                // signal — it's now folded into the flag itself instead of
                // being H264-specific. Before Phase 6.1 this was honestly
                // hardcoded `false` because there was nothing silicon to
                // report on.
                silicon_ok: silicon_decode_healthy,
                dropped_frames: dropped_frames.min(u32::MAX as u64) as u32,
                rtt_us: last_measured_rtt_us,
            });
        }
        if last_rtt_ping_sent.elapsed() > RTT_PING_INTERVAL {
            last_rtt_ping_sent = Instant::now();
            let _ = session.send_keepalive_ping(crate::evrt2_session::now_us());
        }
        if !force_switch_fired {
            if let Some(delay) = force_switch_after {
                if connected_at.elapsed() >= delay {
                    force_switch_fired = true;
                    on_status(format!(
                        "EVRT2 (experimental): EVRTDESK_EVRT2_FORCE_SWITCH_AFTER_SECS={}с истекло — форсирую попытку переключения (Phase 5.4 тест)",
                        delay.as_secs()
                    ));
                    attempt_switch_to_relay(&mut session, &mut relay, &mut rtt_est, &mut on_status);
                }
            }
        }
        let Ok(Some((header, payload))) = session.recv_one() else {
            continue;
        };
        let arrival = Instant::now();
        last_activity = arrival;
        jitter_est.on_packet_arrival(arrival);
        if tracking_frame_id != Some(header.frame_id) {
            tracking_frame_id = Some(header.frame_id);
            frame_has_visible = false;
            frame_start_arrival = Some(arrival);
            visible_arrival = None;
        }
        frame_has_visible |= header.is_visible_region();
        if header.is_visible_region() && visible_arrival.is_none() {
            visible_arrival = Some(arrival);
        }

        match header.packet_type {
            crate::evrt2_packet::PacketType::SessionAck => {
                if !got_ack {
                    got_ack = true;
                    // ROADMAP.md Phase 4.2/4.3 — set only now, matching the
                    // host side (which sets it right after sending its own
                    // ACK): HELLO/ACK travel unauthenticated/unencrypted on
                    // both sides, everything from here on doesn't.
                    session.set_auth_key(auth_key);
                    on_status("EVRT2 (experimental): подключено ✓".to_owned());
                }
            }
            crate::evrt2_packet::PacketType::VideoFrame
            | crate::evrt2_packet::PacketType::FecRepair => {
                if let IngestResult::FrameComplete { frame_id, bytes } =
                    reassembler.ingest(&header, &payload)
                {
                    // Live-found: tried dropping any frame_id at or behind
                    // `last_frame_id_seen` here (a UDP-reordering theory for
                    // a "teleport back to an older frame" symptom) — live
                    // test showed it did NOT fix the teleport and made
                    // smoothness WORSE (dropping frames the ordering theory
                    // wrongly flagged as stale). Reverted; the real cause of
                    // the teleport symptom is still open — see ROADMAP.md
                    // for the honest current state before trying again.
                    if let Some(last) = last_frame_id_seen {
                        if frame_id > last.wrapping_add(1) {
                            dropped_frames += (frame_id - last - 1) as u64;
                            // A dropped frame isn't just a missed picture —
                            // for any predictive codec (EVRTCK delta, H264/
                            // H265 P-frames) it desyncs the decoder's
                            // reference state from what the host's encoder
                            // now assumes the client has, corrupting every
                            // frame that references the gap until a real
                            // keyframe arrives. Previously this only got
                            // counted, not acted on — the picture would
                            // stay corrupted/black for up to
                            // `KEYFRAME_INTERVAL` (2s on the host) waiting
                            // for the next periodic keyframe. Live-found
                            // under a genuine H265 throughput burst (AIMD
                            // backing off from real ~18% packet loss): a
                            // visible black flash the user reported.
                            // Requesting a fresh IDR immediately closes the
                            // SAME desync-recovery gap Phase 6.1/1.4
                            // already close for explicit decode errors —
                            // this is a missed spot in that same class of
                            // fix, not a new mechanism.
                            if last_idr_request.elapsed() >= IDR_REQUEST_COOLDOWN {
                                last_idr_request = Instant::now();
                                let _ = session.send_idr_request();
                            }
                        }
                    }
                    last_frame_id_seen = Some(frame_id);

                    // Live-found (chasing visible "рывки" — motion judder,
                    // described as the same movement appearing to repeat):
                    // this used to sleep for `buffer_depth_for_packet`
                    // (jitter-derived, Task 01 § 3) on every non-visible-
                    // region frame — which in practice is EVERY frame in an
                    // EVRT2-only test session, since `frame_has_visible`
                    // only ever goes true from real splicing/visible-region
                    // traffic that needs an actual client cursor focus this
                    // harness never has. The sleep ran on THIS SAME thread —
                    // the one loop that also reads the next incoming UDP
                    // packet — so every "smoothing" delay was also a
                    // blackout window where nothing could be received.
                    // Packets that arrived in a burst during that window
                    // (normal on real WiFi) queued up in the OS socket
                    // buffer and all got drained and decoded back-to-back
                    // the instant the sleep ended: exactly a stutter-then-
                    // catch-up pattern, not smoothing. For a product whose
                    // stated priority is low, predictable input latency
                    // (fast typing, 3D work) over deliberately-buffered
                    // smoothness, releasing every frame immediately — the
                    // same fast path VISIBLE_REGION content already used —
                    // is the right call on both counts: lower latency AND
                    // no more self-inflicted burst decoding.
                    let _ = frame_has_visible; // no longer gates a delay here — see the comment above

                    // ROADMAP.md Phase 6.1 step 4: IS_SILICON frames are
                    // real NVENC-encoded H264, not EVRTCK — route to the
                    // H264 decoder instead. Same wire flag the host's step 3
                    // code sets (`send_frame`'s `is_silicon` parameter).
                    // ROADMAP.md Phase 6.4: IS_SILICON now has two possible
                    // shapes on the wire — a plain NVENC bitstream (as
                    // before), or a spliced container (`SPL2` magic) with an
                    // EVRTCK visible-region overlay riding alongside it.
                    // `parse_spliced_payload` tells the two apart from the
                    // first 4 bytes alone; a plain Annex-B stream can never
                    // match the magic (see that function's doc comment).
                    if header.has_flag(crate::evrt2_packet::flags::IS_SILICON) {
                        // ROADMAP.md Phase 6.3′ H265 A/B test: no software
                        // HEVC decoder exists anywhere in this codebase's
                        // dependency tree (`openh264` is a Cisco H.264-only
                        // implementation) — a real H265 comparison needs the
                        // platform's own hardware decoder, same as the live
                        // (non-EVRT2) pipeline already uses for H265. On
                        // Android that's MediaCodec, driven the exact same
                        // way the live pipeline already drives it
                        // (`android_video::decode_frame_to_surface`), which
                        // renders straight into the `Surface` the game-remote
                        // screen's `TextureView` already sets up for this
                        // exact session (`useHardware` is true whenever
                        // `activeCodec != "EVRTCK"`, which includes the
                        // "EVRT2ONLY" session codec this whole function
                        // serves — no Kotlin/UI changes were needed). Unlike
                        // the H264 branch below, there is no CPU-side RGBA
                        // buffer here at all (MediaCodec owns the pixels),
                        // so a Phase 6.4 splicing overlay cannot be
                        // composited onto an H265 frame — acceptable for
                        // this A/B test since splicing is already documented
                        // (Task-01/ROADMAP) as almost never firing in the
                        // EVRT2-only harness (no real cursor input).
                        if header.has_flag(crate::evrt2_packet::flags::IS_H265) {
                            let (h265_bytes, had_overlay) = match parse_spliced_payload(&bytes) {
                                Some((bg, _overlay)) => (bg, true),
                                None => (bytes.as_slice(), false),
                            };
                            #[cfg(all(target_os = "android", feature = "android-client"))]
                            let decoded = crate::android_video::decode_frame_to_surface(
                                "H265",
                                h265_bytes,
                                header.has_flag(crate::evrt2_packet::flags::IS_KEYFRAME),
                                0,
                                0,
                            );
                            #[cfg(not(all(target_os = "android", feature = "android-client")))]
                            let decoded = {
                                let _ = h265_bytes;
                                false
                            };
                            if decoded {
                                silicon_decode_healthy = true;
                                // Counts a successful MediaCodec submission,
                                // not a confirmed on-screen render — MediaCodec
                                // decodes asynchronously, unlike `openh264`'s
                                // synchronous call below. An honestly-labeled
                                // approximation, matching the same "frame
                                // accepted" moment the H264 branch counts at.
                                frames_received += 1;
                                fps_window_count += 1;
                                if fps_window_start.elapsed() >= Duration::from_secs(1) {
                                    decoded_fps = fps_window_count as f32
                                        / fps_window_start.elapsed().as_secs_f32();
                                    fps_window_count = 0;
                                    fps_window_start = Instant::now();
                                }
                                if frames_received % 20 == 0 {
                                    on_status(format!(
                                        "EVRT2 (experimental): {frames_received} кадров получено (NVENC/H265, MediaCodec Surface)"
                                    ));
                                }
                                if had_overlay {
                                    on_status("EVRT2 (experimental): H265-кадр нёс сплайс-оверлей — пропущен (Surface-декод не имеет RGBA-буфера для композитинга)".to_owned());
                                }
                            } else {
                                silicon_decode_healthy = false;
                                on_status("EVRT2 (experimental): H265 Surface-декод недоступен на этом клиенте (не Android, или MediaCodec отказал)".to_owned());
                                if last_idr_request.elapsed() >= IDR_REQUEST_COOLDOWN {
                                    last_idr_request = Instant::now();
                                    let _ = session.send_idr_request();
                                }
                            }
                            continue;
                        }
                        let (h264_bytes, overlay_bytes) = match parse_spliced_payload(&bytes) {
                            Some((bg, overlay)) => (bg, Some(overlay)),
                            None => (bytes.as_slice(), None),
                        };
                        match h264_decoder.as_mut() {
                            Some(dec) => match dec.decode(h264_bytes) {
                                Ok(Some(yuv)) => {
                                    silicon_decode_healthy = true;
                                    use openh264::formats::YUVSource;
                                    let (w, h) = yuv.dimensions();
                                    let mut rgba = vec![0u8; w * h * 4];
                                    yuv.write_rgba8(&mut rgba);
                                    // Bug found during the Phase 6.4 (cross-codec
                                    // splicing) investigation: without this, the
                                    // EVRTCK decoder's own tracked framebuffer goes
                                    // stale the moment a silicon frame is shown,
                                    // and the next MODE_DELTA P-frame XORs against
                                    // that stale buffer instead of what's actually
                                    // on screen — silent pixel corruption until the
                                    // next periodic keyframe. Both buffers share the
                                    // same RGBA layout (evrtck.rs's own tile encode
                                    // already does the BGRA→RGBA swap internally —
                                    // see `sync_from_rgba`'s doc comment), so this
                                    // is a plain buffer copy, not a conversion.
                                    decoder.sync_from_rgba(&rgba, w, h);

                                    // Phase 6.4: this frame also carries an
                                    // exact EVRTCK overlay for the visible
                                    // region — apply it on top of the NVENC
                                    // background just synced in, and show
                                    // the composite instead of the raw
                                    // (lossy-everywhere) background.
                                    let final_rgba = if let Some(overlay) = overlay_bytes {
                                        match decoder.apply_absolute_overlay(overlay) {
                                            Ok(_applied) => decoder.current_frame().to_vec(),
                                            Err(e) => {
                                                on_status(format!(
                                                    "EVRT2 (experimental): Phase 6.4 overlay decode error: {e} — showing NVENC background only"
                                                ));
                                                rgba
                                            }
                                        }
                                    } else {
                                        rgba
                                    };

                                    frames_received += 1;
                                    fps_window_count += 1;
                                    if fps_window_start.elapsed() >= Duration::from_secs(1) {
                                        decoded_fps = fps_window_count as f32
                                            / fps_window_start.elapsed().as_secs_f32();
                                        fps_window_count = 0;
                                        fps_window_start = Instant::now();
                                    }
                                    on_frame(w as u32, h as u32, final_rgba);
                                    if frames_received % 20 == 0 {
                                        on_status(format!(
                                            "EVRT2 (experimental): {frames_received} кадров получено (NVENC/H264, Phase 6.1)"
                                        ));
                                    }
                                }
                                // Decoder buffering internally (needs
                                // SPS/PPS/IDR first) — same non-fatal
                                // "needs more packets" case the live EVRT1
                                // client already tolerates for this exact
                                // decoder.
                                Ok(None) => {}
                                Err(e) => {
                                    silicon_decode_healthy = false;
                                    on_status(format!(
                                        "EVRT2 (experimental): NVENC/H264 decode error: {e}"
                                    ));
                                    if last_idr_request.elapsed() >= IDR_REQUEST_COOLDOWN {
                                        last_idr_request = Instant::now();
                                        let _ = session.send_idr_request();
                                    }
                                }
                            },
                            None => {
                                silicon_decode_healthy = false;
                                on_status(
                                    "EVRT2 (experimental): получен силиконовый кадр, но H264-декодер недоступен на этом клиенте".to_owned(),
                                );
                            }
                        }
                        continue;
                    }

                    // ROADMAP.md Phase 3.3: steer decode/apply order by the
                    // last APF this client received, when one exists at a
                    // matching grid — `decode_wire_prioritized` itself falls
                    // back to plain byte-stream order (identical to
                    // `decode_wire`) whenever `tile_priority` is `None`, so
                    // this is a strict addition, not a behavior change for
                    // a session that never got an APF at all.
                    let decoder_width = decoder.width();
                    let apf_priority_closure = |tile_idx: usize| -> f32 {
                        let (Some(map), Some(hdr)) = (last_apf.as_ref(), last_apf_header.as_ref())
                        else {
                            return 0.0;
                        };
                        if hdr.cols == 0 || hdr.rows == 0 {
                            return 0.0;
                        }
                        let evrtck_tiles_x =
                            decoder_width.div_ceil(crate::evrtck::TILE_SIZE).max(1);
                        let tx = tile_idx % evrtck_tiles_x;
                        let ty = tile_idx / evrtck_tiles_x;
                        // APF cells can be coarser than EVRTCK's native
                        // tile grid (`fit_scale_for_budget` on the host) —
                        // `tile_size` in the header is the actual cell edge
                        // in pixels, so dividing by EVRTCK's fixed
                        // TILE_SIZE recovers that coarsening factor.
                        let scale = (hdr.tile_size as usize / crate::evrtck::TILE_SIZE).max(1);
                        let apf_tx = tx / scale;
                        let apf_ty = ty / scale;
                        if apf_tx < hdr.cols as usize && apf_ty < hdr.rows as usize {
                            map[apf_tx + apf_ty * hdr.cols as usize]
                        } else {
                            0.0
                        }
                    };
                    let tile_priority: Option<&dyn Fn(usize) -> f32> = if last_apf.is_some() {
                        Some(&apf_priority_closure)
                    } else {
                        None
                    };

                    match decoder.decode_wire_prioritized(&bytes, tile_priority) {
                        Ok((rgba, apply_order)) => {
                            let rgba = rgba.to_vec();
                            let (w, h) = (decoder.width() as u32, decoder.height() as u32);
                            frames_received += 1;
                            // Profiling signal for the Phase 3.3 acceptance
                            // criterion ("focus tiles painted first"): among
                            // this frame's actually-dirty tiles, was the one
                            // with the highest APF priority also the FIRST
                            // one applied? Only meaningful once an APF
                            // baseline exists and more than one tile changed.
                            if tile_priority.is_some() && apply_order.len() > 1 {
                                apf_priority_profile_samples += 1;
                                let max_priority_tile =
                                    apply_order.iter().copied().max_by(|&a, &b| {
                                        apf_priority_closure(a)
                                            .partial_cmp(&apf_priority_closure(b))
                                            .unwrap_or(std::cmp::Ordering::Equal)
                                    });
                                if max_priority_tile == apply_order.first().copied() {
                                    apf_priority_profile_hits += 1;
                                }
                                if apf_priority_profile_samples % 60 == 0 {
                                    on_status(format!(
                                        "EVRT2 (experimental): APF-приоритетный порядок отрисовки — focus-тайл первым в {}/{} кадрах",
                                        apf_priority_profile_hits, apf_priority_profile_samples
                                    ));
                                }
                            }
                            fps_window_count += 1;
                            if fps_window_start.elapsed() >= Duration::from_secs(1) {
                                decoded_fps = fps_window_count as f32
                                    / fps_window_start.elapsed().as_secs_f32();
                                fps_window_count = 0;
                                fps_window_start = Instant::now();
                            }
                            on_frame(w, h, rgba);

                            if let (Some(start), Some(visible)) =
                                (frame_start_arrival, visible_arrival)
                            {
                                let uniform_age = arrival.saturating_duration_since(start);
                                let visible_age = visible.saturating_duration_since(start);
                                if let Some(gain) = uniform_age.checked_sub(visible_age) {
                                    gain_sum_us += gain.as_micros() as u64;
                                    gain_samples += 1;
                                }
                            }
                            if frames_received % 20 == 0 {
                                let gain_avg_ms = if gain_samples > 0 {
                                    (gain_sum_us as f64 / gain_samples as f64) / 1000.0
                                } else {
                                    0.0
                                };
                                on_status(format!(
                                    "EVRT2 (experimental): {frames_received} кадров получено, EVRT Gain (proxy) ≈ {gain_avg_ms:.1}мс"
                                ));
                                gain_sum_us = 0;
                                gain_samples = 0;
                            }
                        }
                        Err(e) => {
                            on_status(format!("EVRT2 (experimental): decode error: {e}"));
                            if last_idr_request.elapsed() >= IDR_REQUEST_COOLDOWN {
                                last_idr_request = Instant::now();
                                let _ = session.send_idr_request();
                            }
                        }
                    }
                }
            }
            // ROADMAP.md Phase 1.3: discreet, honest indicator — never
            // hides a breach, never fabricates one either (only forwards
            // what the host actually measured).
            crate::evrt2_packet::PacketType::DegradeSignal => {
                if let Some(info) = parse_degrade_signal(&payload) {
                    on_status(format!(
                        "EVRT2 (experimental): ⚠ Task01 breach — visible region {:?} > потолок {:?} ({} тайлов)",
                        info.measured_age, info.ceiling, info.region_tiles.len()
                    ));
                    on_degrade(info.measured_age, info.ceiling, info.region_tiles.len());
                }
            }
            // ROADMAP.md Phase 3.1/3.2/3.3: real Attention Priority Field
            // round trip (both the full snapshot and delta encodings — the
            // payload's own `encoding` byte says which), kept as this
            // client's `last_apf`/`last_apf_header` baseline — Phase 3.3
            // uses it below to steer `decode_wire_prioritized`'s tile apply
            // order for the NEXT video frame decoded.
            crate::evrt2_packet::PacketType::ApfUpdate => {
                // Byte 6 is `encoding` in both wire formats (see
                // `evrt2_apf`'s header layout) — peek at it before
                // committing to either decoder, since `decode_delta` needs
                // `last_apf` to already be Some at the right size.
                let decoded = match payload.get(6) {
                    Some(&crate::evrt2_apf::APF_ENCODING_DELTA) => last_apf
                        .as_ref()
                        .and_then(|prev| crate::evrt2_apf::decode_delta(&payload, prev)),
                    _ => crate::evrt2_apf::decode_u4(&payload),
                };
                if let Some((apf_header, map)) = decoded {
                    apf_updates_received += 1;
                    if apf_updates_received == 1 || apf_updates_received % 20 == 0 {
                        let max_p = map.iter().cloned().fold(0.0f32, f32::max);
                        let min_p = map.iter().cloned().fold(1.0f32, f32::min);
                        on_status(format!(
                            "EVRT2 (experimental): APF #{} ({}) — {}x{} тайлов, P_i∈[{:.2},{:.2}]",
                            apf_updates_received,
                            if apf_header.encoding == crate::evrt2_apf::APF_ENCODING_DELTA {
                                "delta"
                            } else {
                                "full"
                            },
                            apf_header.cols,
                            apf_header.rows,
                            min_p,
                            max_p
                        ));
                    }
                    last_apf_header = Some(apf_header);
                    last_apf = Some(map);
                }
                // A delta that couldn't decode (no baseline yet, or a
                // dimension mismatch after a resolution change) is dropped
                // silently here — the next full snapshot at the next
                // keyframe re-establishes `last_apf`, matching the host's
                // own "wait for the next keyframe" fallback rather than
                // this client guessing at a corrupted map.
            }
            // ROADMAP.md Phase 2.2: "Client decoder is mode-agnostic —
            // receives any mode transparently" (AR2R47_MODES.md) — but the
            // jitter buffer's depth FORMULA is mode-dependent (SDUDP.md §2:
            // Mode47 uses a more aggressive floor/multiplier), so it must
            // follow the switch even though decoding itself doesn't care.
            crate::evrt2_packet::PacketType::ModeSwitch => {
                let info = crate::evrt2_session::parse_mode_switch(&header, &payload);
                session.set_mode(info.new_mode);
                jitter_est.set_mode(ModeProfile::from_evrt2_mode(info.new_mode));
                on_status(format!(
                    "EVRT2 (experimental): MODE_SWITCH → {:?} ({:?})",
                    info.new_mode, info.reason
                ));
            }
            // ROADMAP.md Phase 5.4: the host's echo of our RTT ping —
            // `evrt2_session::build_keepalive_ping` doc explains why this
            // rides KEEPALIVE. `now_us().saturating_sub` guards against a
            // clock adjustment landing between send and receive (SystemTime
            // is not monotonic) producing a negative/huge bogus sample.
            crate::evrt2_packet::PacketType::Keepalive => {
                if let Some(send_time_us) = crate::evrt2_session::parse_keepalive_ping(&payload) {
                    let rtt_us = crate::evrt2_session::now_us().saturating_sub(send_time_us);
                    last_measured_rtt_us = rtt_us.min(u32::MAX as u64) as u32;
                    let rtt = Duration::from_micros(rtt_us);
                    if rtt_est.on_sample(rtt) {
                        on_status(format!(
                            "EVRT2 (experimental): деградация пути обнаружена — RTT {:.1}мс (базовый {:.1}мс, >{:.0}×)",
                            rtt.as_secs_f32() * 1000.0,
                            rtt_est.baseline().map(|d| d.as_secs_f32() * 1000.0).unwrap_or(0.0),
                            3.0,
                        ));
                        attempt_switch_to_relay(
                            &mut session,
                            &mut relay,
                            &mut rtt_est,
                            &mut on_status,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Target resolution for downscaling to fit the client's declared
/// `max_res` (see `Evrt2Session::client_max_res`'s doc comment for why this
/// exists), preserving aspect ratio and clamped to even dimensions (video
/// codecs generally require even width/height). `None` means no downscale
/// is needed — either the client didn't declare a cap (`(0, 0)`, the
/// "unset" sentinel) or the native resolution already fits within it.
/// Mirrors `video_pipeline.rs`'s own `client_cap_resolution`/
/// `scale_even_to_fit` (the live EVRT1 pipeline's equivalent), kept as a
/// separate small copy here rather than exported cross-module to avoid
/// coupling two otherwise-independent pipelines over a two-function detail.
fn client_downscale_target(src_w: u32, src_h: u32, client_max: (u32, u32)) -> Option<(u32, u32)> {
    let (client_w, client_h) = client_max;
    if src_w == 0 || src_h == 0 || client_w == 0 || client_h == 0 {
        return None;
    }
    if src_w <= client_w && src_h <= client_h {
        return None;
    }
    let scale = (client_w as f32 / src_w as f32)
        .min(client_h as f32 / src_h as f32)
        .min(1.0);
    let dw = ((src_w as f32 * scale) as u32 & !1).max(2);
    let dh = ((src_h as f32 * scale) as u32 & !1).max(2);
    (dw != src_w || dh != src_h).then_some((dw, dh))
}

/// Nearest-neighbor BGRA downscale into `dst` (resized in place). Same
/// fixed-point (16.16) sampling `video_pipeline.rs`'s own `downscale_bgra`
/// uses for the live pipeline's software-encoder downscale path — cheap,
/// no external dependencies, good enough for a screen-share source (no
/// fine detail lost to aliasing that a user would notice at these scale
/// factors).
fn downscale_bgra_box(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst: &mut Vec<u8>,
    dst_w: u32,
    dst_h: u32,
) {
    let (sw, sh, dw, dh) = (
        src_w as usize,
        src_h as usize,
        dst_w as usize,
        dst_h as usize,
    );
    dst.resize(dw * dh * 4, 0);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh * 4 {
        return;
    }
    let x_ratio = ((sw << 16) / dw) as u64;
    let y_ratio = ((sh << 16) / dh) as u64;
    for dy in 0..dh {
        let sy = ((dy as u64 * y_ratio) >> 16) as usize;
        let src_row = sy * sw * 4;
        let dst_row = dy * dw * 4;
        for dx in 0..dw {
            let sx = ((dx as u64 * x_ratio) >> 16) as usize;
            let s = src_row + sx * 4;
            let d = dst_row + dx * 4;
            if s + 3 < src.len() && d + 3 < dst.len() {
                dst[d] = src[s];
                dst[d + 1] = src[s + 1];
                dst[d + 2] = src[s + 2];
                dst[d + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evrt2_session::build_ack;
    use std::net::SocketAddr;

    // ── EVRT1-parity fps investigation: client-max-res downscale ───────────

    #[test]
    fn downscale_target_is_none_when_client_cap_unset() {
        assert_eq!(client_downscale_target(2560, 1440, (0, 0)), None);
    }

    #[test]
    fn downscale_target_is_none_when_native_already_fits() {
        assert_eq!(client_downscale_target(1280, 720, (1920, 1080)), None);
    }

    #[test]
    fn downscale_target_shrinks_to_fit_preserving_aspect_ratio() {
        // Same real MI_8 case that motivated this fix: 2560×1440 native,
        // client declares 1080 width (its own portrait screen width) — the
        // width-limited scale (0.421875) applies to both dimensions, giving
        // 1080×606 (1440×0.421875 = 607.5, truncated then rounded down to
        // even) — essentially the same shape EVRT1's own `SessionConfig`
        // log showed live for this exact phone (1080×608, off by one
        // rounding step from a different, EVRT1-internal formula — the
        // point proven here is "downscale to roughly the client's real
        // width," not bit-for-bit parity with EVRT1's own rounding).
        let target = client_downscale_target(2560, 1440, (1080, 1080));
        assert_eq!(target, Some((1080, 606)));
    }

    #[test]
    fn downscale_target_dimensions_are_always_even() {
        for (src_w, src_h, cap_w, cap_h) in [
            (2561, 1441, 1081, 1081),
            (1920, 1080, 999, 999),
            (2560, 1440, 1, 1),
        ] {
            if let Some((dw, dh)) = client_downscale_target(src_w, src_h, (cap_w, cap_h)) {
                assert_eq!(dw % 2, 0, "width must be even: {dw}");
                assert_eq!(dh % 2, 0, "height must be even: {dh}");
                assert!(
                    dw >= 2 && dh >= 2,
                    "dimensions must never collapse to zero: {dw}x{dh}"
                );
            }
        }
    }

    #[test]
    fn downscale_bgra_box_produces_correctly_sized_output_and_preserves_solid_color() {
        let src_w = 4u32;
        let src_h = 4u32;
        let mut src = vec![0u8; (src_w * src_h * 4) as usize];
        for px in src.chunks_exact_mut(4) {
            px.copy_from_slice(&[10, 20, 30, 255]);
        }
        let mut dst = Vec::new();
        downscale_bgra_box(&src, src_w, src_h, &mut dst, 2, 2);
        assert_eq!(dst.len(), 2 * 2 * 4);
        for px in dst.chunks_exact(4) {
            assert_eq!(px, &[10, 20, 30, 255]);
        }
    }

    // ── ROADMAP.md Phase 2.4′: loss/jitter-based bandwidth-floor estimate ──

    #[test]
    fn clean_link_reports_unconstrained_bandwidth() {
        let mut est = BandwidthEstimator::new();
        let (bps, loss_rate) = est.on_feedback(0, 180, 8_000);
        assert_eq!(bps, u32::MAX);
        assert_eq!(loss_rate, Some(0.0));
    }

    #[test]
    fn sustained_loss_above_2_percent_forces_the_floor_after_two_consecutive_samples() {
        let mut est = BandwidthEstimator::new();
        // 5/180 ≈ 2.8% — above the 2% threshold, but ONE sample must not be
        // enough (hysteresis — see the struct's own doc for why).
        let (bps1, loss_rate) = est.on_feedback(5, 180, 8_000);
        assert_eq!(bps1, u32::MAX, "one bad sample alone must not force AR");
        assert!(loss_rate.unwrap() > 0.02);
        let (bps2, _) = est.on_feedback(5, 180, 8_000);
        assert!(
            bps2 < crate::evrt2_modes::BANDWIDTH_FORCES_AR_BPS,
            "two consecutive bad samples must force AR, got {bps2}"
        );
    }

    #[test]
    fn loss_right_at_2_percent_does_not_force_the_floor() {
        // Exactly 2% — the check is strictly-greater, matching
        // `evrt2_modes::ModeSelector`'s own strict `<` on the floor itself
        // (being exactly at a threshold never counts as crossing it,
        // consistent everywhere in this codebase).
        let mut est = BandwidthEstimator::new();
        for _ in 0..5 {
            let (bps, _) = est.on_feedback(2, 100, 8_000);
            assert_eq!(bps, u32::MAX);
        }
    }

    #[test]
    fn high_jitter_alone_forces_the_floor_after_two_consecutive_samples() {
        let mut est = BandwidthEstimator::new();
        let (bps1, loss_rate) = est.on_feedback(0, 180, 60_000);
        assert_eq!(bps1, u32::MAX, "one bad sample alone must not force AR");
        assert_eq!(
            loss_rate,
            Some(0.0),
            "loss rate must be reported honestly even when jitter alone triggered it"
        );
        let (bps2, _) = est.on_feedback(0, 180, 60_000);
        assert!(bps2 < crate::evrt2_modes::BANDWIDTH_FORCES_AR_BPS);
    }

    #[test]
    fn recovery_requires_two_consecutive_clean_samples_too() {
        let mut est = BandwidthEstimator::new();
        est.on_feedback(0, 180, 60_000);
        let (bps, _) = est.on_feedback(0, 180, 60_000);
        assert!(
            bps < crate::evrt2_modes::BANDWIDTH_FORCES_AR_BPS,
            "must be forced first"
        );
        let (still_forced, _) = est.on_feedback(0, 180, 8_000);
        assert!(
            still_forced < crate::evrt2_modes::BANDWIDTH_FORCES_AR_BPS,
            "one clean sample alone must not release AR"
        );
        let (released, _) = est.on_feedback(0, 180, 8_000);
        assert_eq!(
            released,
            u32::MAX,
            "two consecutive clean samples must release AR"
        );
    }

    /// ROADMAP.md Phase 2.4′ — the exact regression this struct exists to
    /// fix, reproduced from a real live session log: host running under
    /// heavy load sent far fewer than 60 real frames per FEEDBACK window
    /// (observed ~3fps against a 60fps target), so a window like "37
    /// dropped / 9 sent" — a mathematically nonsensical loss rate over
    /// 100% — arrived as a single feedback sample. The OLD stateless
    /// version reacted to this immediately, live, causing a real
    /// MODE_SWITCH AR↔2R flapping loop. The fix must refuse to trust a
    /// rate from a window this small, at all.
    #[test]
    fn tiny_window_with_more_dropped_than_sent_does_not_force_ar_ever() {
        let mut est = BandwidthEstimator::new();
        for _ in 0..10 {
            let (bps, loss_rate) = est.on_feedback(37, 9, 8_000);
            assert_eq!(
                bps,
                u32::MAX,
                "a window with only 9 frames sent must never be trusted for a rate"
            );
            assert_eq!(
                loss_rate, None,
                "must report 'no rate' honestly, not a fabricated >100% number"
            );
        }
    }

    #[test]
    fn estimated_floor_value_is_actually_below_the_mode_selector_threshold() {
        // Not just "some smaller number" — genuinely below the exact
        // constant `ModeSelector::evaluate` compares against, so this
        // integrates correctly with evrt2_modes.rs rather than merely
        // looking plausible in isolation.
        let mut est = BandwidthEstimator::new();
        est.on_feedback(10, 100, 8_000);
        let (bps, _) = est.on_feedback(10, 100, 8_000);
        assert!(bps < crate::evrt2_modes::BANDWIDTH_FORCES_AR_BPS);
    }

    /// End-to-end across the module boundary: feed `BandwidthEstimator`'s
    /// real output straight into a real `ModeSelector::evaluate` (not a
    /// hand-picked stand-in number) and confirm it actually forces AR — the
    /// two modules' own separate unit tests each prove their own half
    /// correct in isolation; this proves the units/thresholds genuinely
    /// agree with each other where they meet.
    #[test]
    fn a_real_degraded_link_estimate_actually_drives_the_mode_selector_to_ar() {
        let mut est = BandwidthEstimator::new();
        est.on_feedback(8, 100, 8_000); // 8% loss, sample 1
        let (bandwidth_bps, _) = est.on_feedback(8, 100, 8_000); // sample 2 — now forced
        let selector = crate::evrt2_modes::ModeSelector::new(crate::evrt2_packet::Mode::R2);
        let signals = crate::evrt2_modes::ModeSignals {
            motion_ratio: 0.5,
            idle_duration: Duration::ZERO,
            game_detected: false,
            silicon_available: false,
            bandwidth_bps,
            user_requested_game_mode: false,
        };
        assert_eq!(
            selector.evaluate(&signals),
            Some((
                crate::evrt2_packet::Mode::Ar,
                crate::evrt2_modes::SwitchReason::BandwidthForcedAr
            ))
        );
    }

    /// ROADMAP.md Phase 5.2, live over loopback: three candidates — one dead
    /// (nothing bound, never answers), one that's a real listener answering
    /// with SESSION_ACK, one that's a real listener that stays silent (to
    /// prove the winner is genuinely picked by "answered first", not just
    /// "only one that could answer"). `race_hello_candidates` must return
    /// exactly the address of the socket that actually sent ACK.
    #[test]
    fn race_hello_candidates_picks_the_socket_that_actually_acks() {
        let dead_addr: SocketAddr = "127.0.0.1:1".parse().unwrap(); // nothing binds port 1

        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        silent
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let silent_addr = silent.local_addr().unwrap();

        let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
        responder
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let responder_addr = responder.local_addr().unwrap();

        let responder_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            // Wait for the raced HELLO, then answer with a real ACK — same
            // wire bytes run_host_experiment's own send_ack would produce.
            if let Ok((len, from)) = responder.recv_from(&mut buf) {
                if let Ok((header, _)) = crate::evrt2_packet::PacketHeader::decode(&buf[..len]) {
                    if header.packet_type == crate::evrt2_packet::PacketType::SessionHello {
                        let ack = build_ack(Mode::Ar);
                        let _ = responder.send_to(&ack, from);
                    }
                }
            }
        });
        // The silent listener drains whatever HELLO arrives but never
        // replies — proves it genuinely loses the race rather than the test
        // accidentally only having one live candidate.
        let silent_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            let _ = silent.recv_from(&mut buf);
        });

        let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        client_socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let candidates = vec![dead_addr, silent_addr, responder_addr];
        let stop = Arc::new(AtomicBool::new(false));
        let mut statuses = Vec::new();
        let winner = race_hello_candidates(
            &client_socket,
            &candidates,
            Mode::Ar,
            60,
            (1920, 1080),
            &stop,
            &mut |s| statuses.push(s),
        );

        assert_eq!(winner, Some(responder_addr));
        responder_thread.join().unwrap();
        silent_thread.join().unwrap();
    }

    /// No candidate answers at all (all dead ports) — must return `None`
    /// rather than hang forever or panic, within `RACE_TIMEOUT`.
    #[test]
    fn race_hello_candidates_returns_none_when_nothing_answers() {
        let dead_a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let dead_b: SocketAddr = "127.0.0.1:2".parse().unwrap();
        let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        client_socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let winner = race_hello_candidates(
            &client_socket,
            &[dead_a, dead_b],
            Mode::Ar,
            60,
            (1920, 1080),
            &stop,
            &mut |_| {},
        );
        assert_eq!(winner, None);
    }

    /// ROADMAP.md Phase 5.3: all UDP candidates are dead ports (simulating
    /// total UDP failure — symmetric NAT), but a relay channel pair is
    /// wired in and something on the other end answers with a real ACK
    /// down that channel. `race_hello_all_paths` must pick `RaceWinner::Relay`
    /// — proves the relay leg genuinely participates in the race rather
    /// than only ever being a documented-but-dead code path.
    #[test]
    fn race_hello_all_paths_picks_relay_when_only_relay_answers() {
        let dead_a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let dead_b: SocketAddr = "127.0.0.1:2".parse().unwrap();
        let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        client_socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();

        let (to_relay_peer_tx, to_relay_peer_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let (from_relay_peer_tx, from_relay_peer_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        // Stand-in for the other side of the TCP relay tunnel: waits for the
        // raced HELLO, answers with a real ACK — same shape
        // `wait_for_relay_hello_and_ack` on the host actually produces.
        let responder_thread = std::thread::spawn(move || {
            if let Ok(raw) = to_relay_peer_rx.recv_timeout(Duration::from_secs(2)) {
                if let Ok((header, _)) = crate::evrt2_packet::PacketHeader::decode(&raw) {
                    if header.packet_type == crate::evrt2_packet::PacketType::SessionHello {
                        let _ = from_relay_peer_tx.send(build_ack(Mode::Ar));
                    }
                }
            }
        });

        let stop = Arc::new(AtomicBool::new(false));
        let relay = (to_relay_peer_tx, from_relay_peer_rx);
        let winner = race_hello_all_paths(
            &client_socket,
            &[dead_a, dead_b],
            Some(&relay),
            Mode::Ar,
            60,
            (1920, 1080),
            &stop,
            &mut |_| {},
        );

        assert!(
            matches!(winner, Some(RaceWinner::Relay)),
            "expected the relay leg to win, got {winner:?}"
        );
        responder_thread.join().unwrap();
    }

    // ── ROADMAP.md Phase 6.3: NvencWorker ───────────────────────────────

    /// Live, real hardware: opens a genuine NVENC session on a dedicated
    /// worker thread, sends it a real (synthetic but correctly-sized) BGRA
    /// frame, and confirms real H264 bytes come back. `#[ignore]`d like the
    /// STUN live test (Phase 5.1) — needs an actual NVIDIA GPU with a free
    /// NVENC session, not something CI can assume. Run explicitly with
    /// `--ignored` to confirm on real hardware.
    #[test]
    #[ignore]
    fn nvenc_worker_opens_a_real_session_and_encodes_a_real_frame() {
        let (w, h) = (640u32, 480u32);
        let worker = NvencWorker::spawn(NvencCodec::H264, w, h, 30, 2_000_000)
            .expect("NVENC session must open on hardware with a free session slot");

        let bgra = vec![128u8; (w * h * 4) as usize];
        worker.send_request(Arc::new(bgra), true); // force_key — first frame must be a real keyframe
        match worker.recv_result(Duration::from_secs(2)) {
            Some(NvencWorkerReply::Encoded {
                result: Ok(Some(pkt)),
                elapsed,
                ..
            }) => {
                assert!(
                    !pkt.bytes.is_empty(),
                    "a real keyframe must produce non-empty Annex-B bytes"
                );
                assert!(
                    pkt.key,
                    "the very first encoded frame, forced, must be reported as a keyframe"
                );
                println!(
                    "NVENC encoded a real {w}x{h} keyframe in {:.2}ms, {} bytes",
                    elapsed.as_secs_f32() * 1000.0,
                    pkt.bytes.len()
                );
            }
            other => panic!("expected a real encoded keyframe, got {other:?}"),
        }
    }

    /// Live, real hardware: two NVENC sessions concurrently — the exact
    /// scenario ROADMAP.md 6.1/6.3 flagged as an unverified risk (this
    /// experimental path's own worker alongside whatever the live EVRT1
    /// pipeline might also be using). Opens two independent `NvencWorker`s
    /// and confirms both can encode without one breaking the other.
    #[test]
    #[ignore]
    fn two_concurrent_nvenc_sessions_do_not_interfere() {
        let (w, h) = (640u32, 480u32);
        let worker_a = NvencWorker::spawn(NvencCodec::H264, w, h, 30, 2_000_000)
            .expect("first NVENC session must open");
        let worker_b = NvencWorker::spawn(NvencCodec::H264, w, h, 30, 2_000_000)
            .expect("a SECOND concurrent NVENC session must also open — if this fails, ROADMAP.md's open concurrency question has a real answer: no");

        let bgra_a = Arc::new(vec![64u8; (w * h * 4) as usize]);
        let bgra_b = Arc::new(vec![192u8; (w * h * 4) as usize]);
        worker_a.send_request(bgra_a, true);
        worker_b.send_request(bgra_b, true);

        for (label, worker) in [("A", &worker_a), ("B", &worker_b)] {
            match worker.recv_result(Duration::from_secs(2)) {
                Some(NvencWorkerReply::Encoded {
                    result: Ok(Some(pkt)),
                    ..
                }) => {
                    assert!(!pkt.bytes.is_empty(), "session {label} must produce real bytes even with a second session concurrently active");
                }
                other => panic!("session {label}: expected a real encoded keyframe, got {other:?}"),
            }
        }
    }

    /// Live, real hardware: the full ROADMAP.md Phase 6.4 splice round trip
    /// — a REAL NVENC-encoded background, decoded through the REAL
    /// `openh264` software decoder the client actually uses (not a
    /// simulated/synthetic stand-in for either codec), spliced with an
    /// EVRTCK absolute overlay, and reassembled client-side. Proves the
    /// design decision this phase's whole point rests on: NVENC's lossy
    /// reconstruction of the background is allowed to be imprecise
    /// (deliberately uses a DIFFERENT solid color than the overlay's true
    /// frame, so any accidental pixel match would be a suspicious
    /// coincidence, not silent success) while the overlaid tile still comes
    /// back byte-exact.
    #[test]
    #[ignore]
    fn splice_round_trips_through_a_real_nvenc_encode_and_real_h264_decode() {
        let (w, h) = (640u32, 480u32);
        let worker = NvencWorker::spawn(NvencCodec::H264, w, h, 30, 2_000_000)
            .expect("NVENC session must open on hardware with a free session slot");

        // Background: a real capture-shaped BGRA buffer, uniform grey —
        // NVENC's OWN reconstruction of this may not be byte-exact (lossy
        // codec), which is fine: nothing asserts on background pixels.
        let background_bgra = vec![100u8; (w * h * 4) as usize];
        worker.send_request(Arc::new(background_bgra), true);
        let nv_pkt = match worker.recv_result(Duration::from_secs(2)) {
            Some(NvencWorkerReply::Encoded {
                result: Ok(Some(pkt)),
                ..
            }) => pkt,
            other => panic!("expected a real encoded keyframe, got {other:?}"),
        };

        // The "true" frame the overlay is built from: a DIFFERENT, distinct
        // color in tile 0 — deliberately far from the background's grey so
        // a pass can't be explained by the two colors coincidentally
        // matching after lossy compression.
        let mut true_frame_bgra = vec![100u8; (w * h * 4) as usize];
        // BGRA byte order: B=10, G=220, R=30. `encode_tile_subset_absolute`
        // does the same BGRA→RGBA swap every other EVRTCK encode path does
        // (see `keyframe_1080p_compresses_better_than_raw` in evrtck.rs), so
        // the decoded output below is checked against [R,G,B,A] =
        // [30, 220, 10, 255], not this array verbatim.
        for px in true_frame_bgra[..(crate::evrtck::TILE_SIZE * crate::evrtck::TILE_SIZE * 4)]
            .chunks_exact_mut(4)
        {
            px.copy_from_slice(&[10, 220, 30, 255]);
        }
        let overlay = crate::evrtck::encode_tile_subset_absolute(
            &true_frame_bgra,
            w as usize,
            h as usize,
            1,
            &[0],
        );

        let (spliced, _overlay_range) = build_spliced_payload(&nv_pkt.bytes, &overlay);
        let (parsed_bg, parsed_overlay) =
            parse_spliced_payload(&spliced).expect("must parse the container it just built");
        assert_eq!(parsed_bg, nv_pkt.bytes.as_slice());
        assert_eq!(parsed_overlay, overlay.as_slice());

        // Real openh264 decode of the REAL NVENC bytes — same decoder the
        // live client uses.
        let mut h264_decoder =
            openh264::decoder::Decoder::new().expect("openh264 decoder must initialize");
        let mut decoded_rgba = None;
        // NVENC's first keyframe may need one extra decode call before
        // producing output on some builds — matches the tolerant
        // Ok(None)-is-not-an-error pattern the live client already uses.
        for _ in 0..2 {
            if let Ok(Some(yuv)) = h264_decoder.decode(parsed_bg) {
                use openh264::formats::YUVSource;
                let (dw, dh) = yuv.dimensions();
                let mut rgba = vec![0u8; dw * dh * 4];
                yuv.write_rgba8(&mut rgba);
                decoded_rgba = Some((rgba, dw, dh));
                break;
            }
        }
        let (bg_rgba, dw, dh) =
            decoded_rgba.expect("real NVENC keyframe must decode to real pixels via openh264");

        let mut decoder = crate::evrtck::EvrtckDecoder::new();
        decoder.sync_from_rgba(&bg_rgba, dw, dh);
        decoder
            .apply_absolute_overlay(parsed_overlay)
            .expect("overlay must apply onto the real NVENC-decoded background");

        // Tile 0 must now be the TRUE overlay color, byte-exact — not
        // NVENC's lossy grey reconstruction of it.
        let final_frame = decoder.current_frame();
        assert_eq!(&final_frame[0..4], &[30, 220, 10, 255], "overlaid tile must be exact (RGBA order) despite sitting on a real lossy NVENC background");

        // A tile OUTSIDE the overlay (bottom-right corner) must still be
        // whatever NVENC actually decoded for the background — proving the
        // overlay didn't clobber unrelated regions.
        let last_px = (final_frame.len() - 4)..final_frame.len();
        assert_ne!(
            &final_frame[last_px],
            &[30, 220, 10, 255],
            "the overlay must not have leaked outside tile 0"
        );
    }
}
