# SD-UDP — Super Dynamic UDP Transport

**Author:** Arthur Valiev  
**Version:** 2026-draft-1

---

## Why "Super Dynamic"

Standard UDP is fire-and-forget. EVRT1 added reliability through
packet reassembly and FeedbackLoop. SD-UDP goes further:
the transport itself **continuously adapts** its behavior based on
network conditions measured in real time — without losing the
zero-overhead properties that make UDP fast.

SD-UDP is not a reliable UDP (that's TCP). It is UDP that **knows**
what it is sending and **schedules** it intelligently.

---

## Core Concepts

### 1. Packet Scheduler

Every frame is broken into slices. The scheduler decides:
- **Transmission order**: IDR slices first (decoder can start sooner)
- **Spacing**: uniform inter-packet gaps to avoid burst loss
- **Priority**: audio packets preempt video mid-frame if needed

```
Frame 1000 (10 slices)
  Send order: IDR_slice_0 (highest priority)
              → slice_1 … slice_9 (evenly spaced)
              → FEC_repair_0, FEC_repair_1 (at end)

Gap between packets: max(1ms, network_jitter_p95 / packet_count)
```

### 2. Jitter Buffer (Adaptive)

Receiver maintains a jitter buffer whose depth adapts to measured
network jitter. Target: **buffer = 1.5× P95 jitter**, updated every
500ms using an exponential moving average.

```
jitter_p95 = EMA(abs(arrival_delta - expected_delta), alpha=0.1)
buffer_depth = max(4ms, min(50ms, jitter_p95 × 1.5))
```

In 47 mode: buffer_depth = max(2ms, jitter_p95 × 1.2) — more aggressive.

### 3. FEC (Forward Error Correction)

FEC is a native SD-UDP feature. Parameters:
- **N**: data packets in a group
- **K**: repair packets (XOR parity of N data packets)
- Recovery: any N packets from (N+K) → full group recovered

Default configuration by mode:
| Mode | N | K | Redundancy | When to use FEC |
|------|---|---|-----------|-----------------|
| AR | 6 | 2 | 25% | Always (WAN) |
| 2R | 8 | 2 | 20% | WAN, >10ms RTT |
| 47 | 0 | 0 | 0% | Never (latency) |

FEC is generated at XOR level — pure Rust, no dependencies.

### 4. Pressure System (FeedbackLoop2)

Receiver measures decode pressure every 50–100ms and sends:

```
struct ReceiverFeedback2 {
    frame_id:       u32,   // last successfully decoded frame
    pressure:       f32,   // 0.0 (idle) … 1.0 (overflow)
    jitter_p95_us:  u32,   // measured network jitter
    decoded_fps:    f32,   // actual decoded FPS at client
    silicon_ok:     bool,  // client silicon decoder healthy
    dropped_frames: u32,   // frames dropped since last feedback
    rtt_us:         u32,   // round-trip time estimate
}
```

Host reacts:
- `pressure > 0.8` → reduce bitrate 20%, drop to lower FPS
- `pressure < 0.2` → allow bitrate increase (10% per 2s)
- `silicon_ok = false` → switch to lower-complexity encoding
- `decoded_fps < target × 0.8` → reduce resolution or FPS cap

### 5. Path Probing (Multi-Candidate)

SD-UDP tries multiple host endpoints simultaneously on connection:
- All LAN IP candidates
- Public IP (via STUN/ipify discovery)
- Relay tunnel endpoint

The **first responding path wins**. All others are closed.
Path switching mid-session if primary path degrades > 3× RTT increase.

---

## Timing Model

```
t=0ms     Frame captured on host
t=1–4ms   Silicon encode (47 mode) / SW encode (AR/2R)
t=4–5ms   EVRT2 packetize + schedule
t=5–6ms   First packet leaves NIC

  [ network: LAN ≤1ms, WAN 10–100ms ]

t=6–10ms  Packets arrive at client (LAN scenario)
t=8–12ms  Jitter buffer releases frame to decoder
t=9–13ms  Silicon decode (1–3ms) or SW decode (5–15ms)
t=10–16ms Frame on display

Total (LAN + silicon both sides): 8–16ms
Total (WAN 50ms RTT + SW decode): 60–90ms
```

---

## Comparison with EVRT1

| Feature | EVRT1 | SD-UDP (EVRT2) |
|---------|-------|----------------|
| Packet format | 24-byte header | 32-byte header |
| FEC | No | Yes (AR/2R modes) |
| Jitter buffer | Static | Adaptive |
| Packet scheduler | Send in order | Priority + spacing |
| Path probing | First available | Multi-candidate race |
| FeedbackLoop | Every 70–150ms | Every 50–100ms, richer |
| Mode awareness | No | Mode in every packet |
| Relay fallback | Separate TCP path | Native RELAY_WRAP packet |
