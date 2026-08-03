# FEEDBACK — ReceiverFeedback2 wire format and what the host actually does with it

**Author:** Arthur Valiev
**Status:** ROADMAP.md Phase 6.6 — written by fact of implementation, July 2026
**Spec:** [`SDUDP.md`](SDUDP.md) § 4 (Pressure System / FeedbackLoop2)

---

## Why this document exists

`SDUDP.md` § 4 describes `ReceiverFeedback2` and a list of host-side
reactions to it. That section is the *design*. This document is the
*as-built* — what's on the wire byte-for-byte, which of those reactions
actually run against a real value today, and which are still an honest
gap. Where the two disagree, this document — not the spec section — is
the accurate one; `SDUDP.md` is left as-is (it's the design record, not a
changelog).

## Wire format (25 bytes, big-endian)

`ReceiverFeedback2::encode`/`::decode` in `src/evrt2_session.rs`:

```text
offset  size  field           type
0       4     frame_id        u32
4       4     pressure        f32
8       4     jitter_p95_us   u32
12      4     decoded_fps     f32
16      1     silicon_ok      u8 (0/1)
17      4     dropped_frames  u32
21      4     rtt_us          u32
                              ---
                              25 bytes total
```

Sent as the payload of a `PacketType::Feedback` (`0x07`) packet, same 32-byte
header as every other EVRT2 packet type. This matches the spec's own field
list exactly — no fields were added or dropped versus `SDUDP.md`'s struct.

## Where each field's VALUE actually comes from (client side)

All seven fields are populated in `run_client_experiment`'s FEEDBACK send
site (`evrt2_experiment.rs`), sent every `KEEPALIVE_INTERVAL` (3s) per the
liveness rule in `SDUDP.md` § 5 — not on the spec's own "every 50–100ms"
cadence (see "Cadence" below for why).

| Field | Source | Honest? |
|---|---|---|
| `frame_id` | `frames_received as u32` — count of frames actually decoded, not the last frame_id seen on the wire | Real |
| `pressure` | **hardcoded `0.0`** | **Gap — see below** |
| `jitter_p95_us` | `JitterEstimator::jitter_p95()` (ROADMAP Phase 0, `evrt2_jitter.rs`) — the real EMA-based estimate, same one that drives this client's own `buffer_depth` decisions | Real |
| `decoded_fps` | measured over a rolling 1-second window of actual `on_frame` calls | Real |
| `silicon_ok` | `h264_decoder.is_some() && h264_decode_healthy` — `h264_decode_healthy` flips to `false` on a real `openh264` decode error and back to `true` on the next successful decode (ROADMAP Phase 6.2) | Real |
| `dropped_frames` | `frame_id` gap detection in the reassembler loop (`frame_id > last.wrapping_add(1)` → gap counted) | Real |
| `rtt_us` | `RttEstimator`'s last KEEPALIVE ping/pong round trip (ROADMAP Phase 5.4, `evrt2_rtt.rs`) | Real |

### `pressure` — the one field that's still fabricated-as-zero, not measured

`SDUDP.md` describes `pressure` as "decode pressure," implying a queue-depth
or backlog signal on the receive side. This client has no such queue to
measure: frames are decoded and handed to `on_frame` synchronously, one at
a time, with no buffering stage whose depth could be sampled. Sending a
fabricated non-zero number here would violate the same principle the rest
of this codebase's honesty comments already follow (see e.g.
`evrt2_scheduler.rs`'s "must not become an excuse to fabricate") — so it's
left at the one value that's actually true: no backpressure is being
measured, full stop, not "backpressure is low." A caller cannot currently
distinguish "genuinely idle" from "we don't measure this" by looking at
`pressure` alone; that ambiguity is itself the honest state of this field
today.

## Host-side reactions: spec vs. actual (`evrt2_experiment.rs`)

`SDUDP.md` § 4 lists five reactions. Here is what the host loop
(`run_experiment_encode_loop`) actually does on receipt of a `Feedback`
packet:

```rust
PacketType::Feedback => {
    if let Some(feedback) = ReceiverFeedback2::decode(&payload) {
        registry.rebalance(&feedback, PROVIDER_NVENC_H264);
    }
}
```

That's the entire handler. One field drives one reaction:

| Spec reaction | Wired? | What actually happens |
|---|---|---|
| `pressure > 0.8` → reduce bitrate 20%, drop FPS | **No** | `pressure` is always `0.0` (see above) — there is nothing for this reaction to trigger on. No bitrate/FPS-reduction code path exists in the experimental encode loop at all. |
| `pressure < 0.2` → allow bitrate increase | **No** | Same as above — no bitrate ramp-up logic exists. |
| `silicon_ok = false` → switch to lower-complexity encoding | **Yes** | `CapabilityRegistry::rebalance()` (ROADMAP Phase 6.2, `execution_capability.rs`) calls `demote_provider(PROVIDER_NVENC_H264)`, which excludes NVENC from `schedule()` for `DEMOTION_COOLDOWN` (30s). The next frame's `use_nvenc` decision (Phase 6.1 step 3) naturally falls back to `PROVIDER_CPU_EVRTCK` — the same `schedule()` call every frame already makes, just with the silicon candidate temporarily filtered out. No separate "give up" message on the wire. **Live-found-and-fixed bug (2026-07-27)**: the original caller demoted on every repeated stale `silicon_ok=false` report, not just fresh ones — since demotion itself stops the client from ever getting another frame to re-verify health on, this turned the 30s cooldown into a permanent lockout after any single transient decode hiccup. Fixed by edge-triggering the call (only a genuine healthy→unhealthy transition demotes); live-reconfirmed the cooldown now actually expires. See `SILICON_PROBE.md` for the full writeup. |
| `decoded_fps < target × 0.8` → reduce resolution or FPS cap | **No** | Still not acted on — no resolution/FPS-adjustment call site exists. `ReceiverFeedback2::decoded_fps_below_target()` exists as a method (`evrt2_session.rs`) and is exercised by unit tests, but is never called from the live encode loop. A permanent host-side log (`FEEDBACK decoded_fps=...`) was added 2026-07-27 for live diagnosis — it revealed `decoded_fps` capping at ~3-8 under real full-motion 2560×1440 content (never near 60), traced to EVRTCK's own synchronous CPU encode cost, not to anything this feedback loop could fix by itself — see `ROADMAP.md` Phase 6.3. |
| Path switching on RTT degradation | **Yes (UDP→relay only)** | `RttEstimator` (Phase 5.4) flags sustained >3× baseline RTT; the client then attempts a real switch to the relay path kept on standby from the initial race (`Evrt2Session::switch_to_relay`), and the host accepts a late HELLO on that same relay channel mid-session. Implemented, unit-tested, and live-wire-tested (2026-07-27) — including finding and fixing two related live bugs (a premature-switch race and a 3s grace-period fix) — see `ROADMAP.md` Phase 5.4. Scope deliberately narrowed to UDP→relay only (not between UDP candidates, not relay→UDP) after a multi-homed-socket reply-address risk was found in the wider alternative; live confirmation so far covers the mechanism surviving a real session, not an *organically* triggered (non-forced) RTT breach, since this session's LAN never crossed the threshold naturally. |

**Net honest summary (updated 2026-07-27):** of the five reactions
`SDUDP.md` describes, two are now real and live-confirmed — `silicon_ok`
→ demotion (ROADMAP Phase 6.1/6.2, including a live-found-and-fixed
permanent-lockout bug) and RTT degradation → UDP→relay path switching
(ROADMAP Phase 5.4). The other three remain either unmeasured
(`pressure`) or measured-but-unused (`decoded_fps` — now at least
logged live, see above). This is not scattered forgetfulness —
`jitter_p95_us` and `rtt_us` in particular exist specifically to feed
the client's OWN local decisions (buffer depth, degradation detection),
not necessarily a host-side reaction; `silicon_ok` and RTT degradation
were wired to change host behavior specifically because they're the two
signals that already had a real mechanism built to consume them
(Capability Registry's `rebalance`/`demote_provider`, and
`RttEstimator`/`switch_to_relay` respectively).

## Cadence: 3s, not 50–100ms

`SDUDP.md` § 4 says feedback is sent "every 50–100ms." The implementation
sends it every `KEEPALIVE_INTERVAL` = 3 seconds. This is not an oversight —
it's `SDUDP.md` § 5's OWN liveness rule ("client sends FEEDBACK at least
every 3s, even when it has nothing to report") governing the actual send
site, because right now FEEDBACK **is** this client's only upstream
keepalive traffic; there is no separate lightweight keepalive packet type in
use on this path. A 50–100ms cadence would work for the pressure-system
purpose the spec describes, but would also be 30-60× more upstream traffic
than the keepalive purpose needs. Since `pressure` (the field a tighter
cadence would matter most for) is not measured yet (see above), there is no
live reaction that a faster cadence would currently unblock — revisit this
tradeoff if/when real backpressure measurement is added.

## What would need to change to close the remaining gaps

Not committed to, not started — recorded here as the honest "what's
missing" so a future pass has a concrete starting point rather than
re-deriving it:

1. **Real `pressure`**: would require an actual queued-but-not-yet-decoded
   frame count on the client, which requires decode to move off the
   receive thread (currently synchronous) onto a queue a depth can be
   sampled from.
2. **Bitrate/FPS reaction to `pressure`/`decoded_fps`**: needs a
   host-side encode-parameter knob that's actually adjustable mid-session
   — `EvrtckEncoder`/`NvencWorker` both currently take fixed
   resolution/fps/bitrate at construction time (see `NvencWorker::spawn`'s
   signature in `evrt2_experiment.rs`), not a live-adjustable target.
3. **RTT-triggered path switching**: needs the host control loop to accept
   a fresh HELLO mid-session (currently only recognizes
   `GOODBYE`/`IDR_REQUEST`/`KEEPALIVE` post-handshake — see ROADMAP.md
   Phase 5.4's own note on this).

---

*EVRT2 FEEDBACK. Arthur Valiev, 2026.*
