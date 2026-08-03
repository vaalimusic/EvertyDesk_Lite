# RELAY_TUNNEL — EVRT2 over TCP relay (RELAY_WRAP)

**Author:** Arthur Valiev
**Status:** ROADMAP.md Phase 5.3 — implemented, unit/loopback-tested, **live-confirmed end to end**: HELLO → ACK → video frames, all over pure RELAY_WRAP with no UDP path at all (`EVRTDESK_EVRT2_FORCE_RELAY=1`). A real bug was found, root-caused, and fixed along the way — see Testing § below.

---

## Why this exists

SDUDP.md § Path Probing names three candidate kinds a session races: LAN
UDP, public/STUN UDP, and a relay tunnel. The first two exist because a
direct UDP path is *usually* available. It is not always: a symmetric NAT
on either end (common on carrier-grade mobile NAT, some corporate
firewalls) means no UDP candidate — not even the STUN-discovered public
one — will ever be reachable from the other side. The relay tunnel is the
fallback for exactly that case: reuse the TCP relay connection this
codebase's RustDesk-compatible signaling already established (it got this
far, so it works) and carry EVRT2's own packets over it instead of opening
a second transport.

## Wire-level deviation from EVRT2_PACKET.md, stated up front

`EVRT2_PACKET.md` names `0x0C RELAY_WRAP` as an EVRT2 packet type whose
payload is itself an inner EVRT2 packet — i.e. wrapping happens *inside*
the EVRT2 protocol. This implementation does not do that. Instead, the raw
EVRT2 wire bytes (32-byte header + payload, byte-identical to what would
go out over UDP) are carried as the payload of a new field in this
codebase's own RustDesk-compatible `Misc` message —
`Evrt2RelayWrap(bytes)`, tag 126 (`src/rustdesk_proto.rs`) — sent as an
ordinary framed `PeerMessage` over the TCP relay stream.

**Why:** this codebase's TCP relay connection is not a raw byte pipe —
it's already multiplexing a specific framed protobuf protocol
(`PeerMessage`/`Misc`) that every other piece of control and fallback
traffic uses (`TcpEvrtckFrame`, `TcpAudioFrame`, `Evrt2ExperimentEndpoints`,
…). Nesting an EVRT2-level `RELAY_WRAP` packet inside that would mean
double framing for no benefit — the `Misc` oneof tag already
self-describes "this is a relay-wrapped EVRT2 packet" exactly as well as a
dedicated EVRT2 packet type would, and reuses machinery (length-prefixed
framing, the existing send/recv cipher split) that's already there and
already trusted. Same reasoning this project already applied to `ENCRYPTED`
(NaCl secretbox instead of the spec's AES-GCM, see `EVRT2_SECURITY.md`):
reuse a mechanism the codebase already has and already tests, rather than
implement the spec's letter for its own sake.

`PacketType::RelayWrap = 0x0C` still exists in `evrt2_packet.rs` (matches
the spec's byte value) but nothing constructs a packet with that type —
it's inert, kept for wire-format parity with the spec document, not used
by this implementation.

## Architecture: `Evrt2Session` is transport-agnostic

Before Phase 5.3, `Evrt2Session` (`evrt2_session.rs`) wrapped a real
`UdpSocket` directly. It now wraps an internal `Transport` enum:

```rust
enum Transport {
    Udp(UdpSocket),
    Relay { outbound: Sender<Vec<u8>>, inbound: Receiver<Vec<u8>> },
}
```

Every other piece of the session — FEC, the Task-01 scheduler, AuthTag
(HMAC), ENCRYPTED (NaCl secretbox) — operates on `Vec<u8>` wire bytes and
has no idea which `Transport` variant is underneath. `send_signed`/
`recv_one`'s only transport-specific code is the branch that gets bytes
onto (or off of) either a real socket or a channel pair. This is why the
loopback test `relay_transport_full_lifecycle_with_auth_and_encryption`
(`evrt2_session.rs`) can prove FEC recovery, AuthTag, and encryption all
still work correctly with ZERO UDP sockets involved — the exact same code
path the UDP tests already exercised.

`Evrt2Session::from_relay_channels(outbound, inbound, peer_label, mode)`
constructs a relay-backed session. `peer_label` is display-only (there's no
real `SocketAddr` for a channel pair — the pair itself already scopes
traffic to exactly one peer, so `recv_one`'s "packet came from the wrong
address" check, meaningful for UDP, is simply not needed here).

## Host side: `run_evrt2_only_session` (host.rs)

This is the only call site wired for RELAY_WRAP so far (see Scope below).

1. `start_host_experiment_with_relay_race(events, relay_inbound)` binds the
   UDP socket (STUN included, per Phase 5.1) and spawns
   `run_host_experiment_race`, which races two throwaway threads:
   - `wait_for_udp_hello_and_ack` — the original Phase 1-5.2 raw-socket
     HELLO wait, unchanged.
   - `wait_for_relay_hello_and_ack` — polls the `relay_inbound` channel
     (bytes the caller already unwrapped from incoming `Evrt2RelayWrap`)
     for a HELLO, replies with ACK via the `relay_outbound` channel.

   Whichever produces a session first wins; an internal `race_won` flag
   (distinct from the session's own lifetime `stop` flag) tells the loser
   to give up within one poll interval (≤200ms). Only the winner's session
   proceeds into `run_experiment_encode_loop` — the same capture → EVRTCK
   encode → send loop as before Phase 5.3, now literally unaware whether
   its `Evrt2Session` is UDP- or relay-backed.

2. `run_evrt2_only_session` splits its `StreamCipher` into `SendCipher`/
   `RecvCipher` halves (`crypto::StreamCipher::into_halves`, the same
   pattern the live EVRT1 pipeline's own input-loop/TCP-sender split
   already uses) and spawns a dedicated writer thread — own
   `TcpStream::try_clone()`, drains `relay_outbound_rx`, wraps each item as
   `Misc::Evrt2RelayWrap` and writes it, and independently owns this
   session's TCP keepalive cadence. This thread is the *only* writer to
   this session's TCP stream; the main thread only reads (`recv_peer_rc`)
   and forwards incoming `Evrt2RelayWrap` bytes into `relay_inbound_tx`.
   Splitting reader and writer this way means neither ever calls
   `write_all` on the same underlying socket concurrently — no locking, no
   risk of two frames interleaving on the wire.

## Client side: `run_client_experiment` / `race_hello_all_paths` (evrt2_experiment.rs)

`race_hello_all_paths` extends the Phase 5.2 UDP-only race with an
optional relay leg: the same HELLO datagram sent to every UDP candidate is
also handed to a `relay_out: Sender<Vec<u8>>` (if the caller has a relay
channel), and the loop polls both the UDP socket and a `relay_in:
Receiver<Vec<u8>>` for the winning ACK. Whichever arrives first decides
`RaceWinner::Udp(addr)` or `RaceWinner::Relay`; `run_client_experiment`
then builds the session with `Evrt2Session::from_bound_socket` or
`Evrt2Session::from_relay_channels` accordingly.

`transport.rs`'s `run_session` owns the actual TCP relay connection to the
host. It creates one `(evrt2_relay_out_tx, evrt2_relay_out_rx)` channel
pair per session (not per experiment attempt) and a per-attempt
`evrt2_relay_in_tx` set fresh each time `Misc::Evrt2ExperimentEndpoints`
starts a new race. The session's own tight poll loop (`SESSION_TICK_MS` =
16ms) drains `evrt2_relay_out_rx` and forwards each item as
`Misc::Evrt2RelayWrap` over the relay stream — the same thread that
already owns all other writes to that stream, so again no cross-thread
write races. Incoming `Evrt2RelayWrap` messages are forwarded into
whichever `evrt2_relay_in_tx` is currently set.

## Testing

Live loopback (no network, deterministic, run on every `cargo test`):

- `evrt2_session::tests::relay_transport_full_lifecycle_with_auth_and_encryption`
  — HELLO → ACK → a keyframe with one simulated lost packet (recovered via
  FEC) → FEEDBACK → GOODBYE, entirely over `Transport::Relay` channel
  pairs, with AuthTag and NaCl encryption both active. Proves the
  transport swap changes nothing about correctness.
- `evrt2_session::tests::relay_transport_rejects_a_packet_signed_with_the_wrong_key`
  — AuthTag verification runs identically on the relay path; a forged
  packet is silently dropped, not accepted, exactly like the UDP path.
- `evrt2_experiment::tests::race_hello_all_paths_picks_relay_when_only_relay_answers`
  — all UDP candidates are dead ports; only the relay channel answers;
  the race correctly picks `RaceWinner::Relay`.

Real network — pushed to completion (ROADMAP.md 5.3): a debug env flag
`EVRTDESK_EVRT2_FORCE_RELAY=1` on the host makes `run_evrt2_only_session`
announce zero UDP candidates, forcing a real client to have nothing to race
except the relay leg.

**A real bug was found this way, root-caused, and fixed.** Diagnostic
logging added on both ends (client: `log::info!` via `android_logger`,
gated `#[cfg(target_os = "android")]` through a local `evrt2log!` macro
since `log` isn't a dependency outside Android; host: the same `host_log`/
`log(events, ...)` channel already visible in the app's own UI) showed the
client building, queuing, and successfully writing its HELLO to the TCP
socket (`send_framed result=Ok(())`) every time, while the host's read loop
— provably alive, since it logged receiving *other* Misc messages in the
same window — never saw `Evrt2RelayWrap` arrive at all.

**Root cause**: `rustdesk_proto.rs`'s `Misc` struct declares its oneof
field as `#[prost(oneof = "misc::Union", tags = "5, 7, ..., 125")]` — every
variant's tag is listed except `Evrt2RelayWrap`'s own `tag = "126"`. prost
uses the containing struct's `tags` list only on the DECODE side, to
recognize which incoming field tags belong to the oneof; encoding uses each
variant's own tag directly and doesn't consult that list at all. Net
effect: the client encoded and sent `Evrt2RelayWrap` correctly every time,
and the host's decoder silently treated tag 126 as an unrecognized field on
`Misc` (standard, non-erroring protobuf forward-compatibility behavior) and
dropped it — `Misc.union` decoded to `None`, exactly matching the live
diagnostic output. The existing loopback tests never caught this because
they exercise `Transport::Relay` through raw mpsc channels, entirely below
this protobuf envelope — nothing before this pass ever round-tripped a real
`Evrt2RelayWrap` through `encode_peer_message`/`decode_peer_message`.

**Fix**: one token — added `126` to the `tags` list. New regression test,
`rustdesk_proto::tests::evrt2_relay_wrap_round_trips_through_the_real_peer_message_envelope`,
round-trips a real `PeerMessage`/`Misc`/`Evrt2RelayWrap` through the actual
encode/decode functions; confirmed to fail without the fix (temporarily
reverted the one line, reran, got `panicked: expected Misc(Evrt2RelayWrap),
got Some(Misc(Misc { union: None }))` — the exact live symptom — then
restored the fix).

**Live-confirmed after the fix**, full round trip, `EVRTDESK_EVRT2_FORCE_RELAY=1`
still active (zero UDP candidates, relay-only): `client received
Evrt2RelayWrap: 32 bytes` → `relay packet decoded, type=SessionAck` →
`relay winner` → a sustained stream of `received Evrt2RelayWrap: 1416
bytes` (real video frame data, dozens of packets/sec) → the client's own
UI shows **`260 кадров получено (NVENC/H264, Phase 6.1)`**. The very first
decode attempt logged a transient `OpenH264 ... Native:2` error (expected —
arrived before the first keyframe) and self-recovered on the next one; a
decode-pipeline detail, not a RELAY_WRAP issue, not investigated further
here.

A separate, unrelated observation surfaced during diagnosis: the host
process's PID changes on nearly every connection attempt ("Автозапуск
доступа…" opens every session log) — something respawns the host process
per incoming connection, which meant a manually-launched
`EVRTDESK_EVRT2_FORCE_RELAY=1` instance didn't always survive to the moment
the phone actually connected during earlier runs. Those runs were discarded
from the analysis (not counted as either a pass or a fail) rather than
reported as successes. Not investigated further — out of this pass's scope.

The diagnostic log lines added on both ends are left in the code
permanently, not reverted — real value for whoever continues this
investigation, not one-off scaffolding.

## Scope — what is NOT covered yet

- Only `run_evrt2_only_session` (the "EVRT2-only test" checkbox path) has
  the relay leg wired host-side. The other host call site — the
  experimental "EVRT2" button pressed mid-session inside a live game-mode
  session (`Evrt2ExperimentRequest` handler in `host.rs`) — still uses the
  plain `start_host_experiment` (UDP-only, Phase 5.2 racing only, no relay
  candidate). That call site doesn't own its TCP relay stream directly the
  way `run_evrt2_only_session` does (it runs inside the live pipeline's
  input loop, which has its own writer-thread arrangement via
  `peer_msg_tx`), so wiring it in is a follow-up, not a copy-paste of the
  same change.
- Phase 5.4 (path switching mid-session on RTT degradation) is separate,
  unstarted work — RELAY_WRAP as implemented here only participates in the
  *initial* race, not a live switch away from an already-established UDP
  path.

---

*EVRT2 RELAY_WRAP. Arthur Valiev, 2026.*
