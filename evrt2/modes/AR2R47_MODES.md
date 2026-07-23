# AR2R47 — The Three Modes of EVRT2CKMAX

**AR2R47 = ARTUR 47**  
*A tribute to Arthur Valiev, creator of EVRT, EVRTCK, and EvertyDesk.*

---

## The Naming

The three operating modes of EVRT2CKMAX are named **AR**, **2R**, and **47**.
Read in sequence: **AR · 2R · 47** → **ARTUR 47**.

This is the permanent identity of the codec, encoded at the protocol level:
- The Mode byte in every EVRT2 packet carries `0x01` (AR), `0x02` (2R), or `0x47` (47)
- `0x47` = ASCII 'G' = Gaming = Arthur's gaming mode

---

## AR — Static Mode

**Named for:** AR (first two letters of ARTUR)  
**Purpose:** Desktop support, IT remote access, static UI  
**Priority:** Lossless quality > bandwidth efficiency > latency

### When AR is selected
- Session type is "support" or "control"
- Screen content detected as static UI (Windows desktop, document, terminal)
- No motion detected for >2 seconds in an active session
- Network bandwidth < 200KB/s (forced to AR as only viable mode)

### What AR delivers
- **Pixel-perfect lossless** reproduction of any UI element
- Text is always sharp — no DCT artifacts, ever
- Bandwidth floors at 10KB/s on a fully static screen
- Single-tile updates: changing one pixel sends one 5-byte tile packet

### Technical profile
```
Codec:          EVRTCK-SW tile engine (XOR-delta)
Tile size:      32×32 pixels
Keyframe:       On session start + reconnect
Delta method:   XOR vs previous frame → ZRLE or zstd-1
Solid tiles:    5-byte solid-color fast path
FEC:            Enabled (N=6, K=2)
Max FPS:        60 (typically 1–30 for static content)
Latency target: ≤30ms (latency is not critical for support)
Silicon usage:  Optional (keyframe only)
```

---

## 2R — Dynamic Mode

**Named for:** 2R (middle two letters of aRtuR — the two R's)  
**Purpose:** Video playback, animations, mixed dynamic content  
**Priority:** Quality ≥ smooth FPS > bandwidth > latency

### When 2R is selected
- Video or animation detected in foreground
- Motion level 30–70% of pixels changing per frame
- Session type is "presentation" or "video"
- Automatic transition from AR when motion threshold crossed

### What 2R delivers
- Smooth 30–60 FPS video with adaptive quality
- Mixed encoding: silicon for high-motion regions, XOR-delta for static regions
- Bandwidth adapts from 200KB/s to 8MB/s based on content
- Quality adapts via FeedbackLoop2 pressure (never drops below "acceptable")

### Technical profile
```
Codec:          Hybrid (silicon NALs + XOR-delta tiles)
Motion blocks:  8×8 pixel blocks (finer than AR's tiles)
Silicon part:   H264 or H265 NAL slices for high-motion regions
Delta part:     EVRTCK-SW for static/low-motion regions
FEC:            Enabled (N=8, K=2)
Max FPS:        60
Latency target: ≤20ms
Silicon usage:  Preferred, SW fallback available
Bitrate ctrl:   VBR, FeedbackLoop2 driven
```

---

## 47 — Gaming Mode

**Named for:** 47 (last two characters of ARTUR 47)  
**Purpose:** Game streaming, interactive high-FPS sessions  
**Priority:** FPS = Latency = Quality — **no compromises on any**

`0x47` in the Mode byte is ASCII 'G' for Gaming.

### When 47 is selected
- Game process detected in foreground
- Motion level >70% of pixels per frame
- User explicitly selects "Game mode" in client UI
- FPS request ≥ 60fps with latency-sensitive content

### What 47 delivers
- Up to **120 FPS** at full resolution
- Up to **4K (3840×2160)** native output
- **≤8ms** encode latency (silicon mandatory)
- **Zero** XOR-delta tile processing — pure silicon path
- Uncapped bitrate — silicon decides, FeedbackLoop2 guides

### Technical profile
```
Codec:          Silicon-only (NVENC / MediaCodec / VideoToolbox / AMF / QSV)
Tile engine:    DISABLED
B-frames:       0 (forbidden — latency)
Look-ahead:     OFF (forbidden — latency)
Rate control:   VBR with hard latency cap
Low-latency:    Hardware flag ON
Max FPS:        120 (or display refresh rate)
Max resolution: 3840×2160 (4K)
FEC:            DISABLED (forbidden — latency at 120fps)
Latency target: ≤8ms encode+packetize
Silicon:        REQUIRED — no SW fallback in 47 mode
                (session falls back to 2R if no silicon found)
```

### Silicon requirements for mode 47

| Platform | Required | API |
|----------|----------|-----|
| Windows NVIDIA | NVENC | `nvEncodeAPI64.dll` |
| Windows any GPU | Media Foundation HW | `mfplat.dll` |
| Android | MediaCodec HW encoder | NDK AMediaCodec |
| Apple macOS/iOS | VideoToolbox | `VideoToolbox.framework` |
| Linux NVIDIA | NVENC | `libnvidia-encode.so` |
| Linux AMD | VAAPI / AMF | planned |
| Linux Intel | VAAPI / QSV | planned |

If no silicon encoder is found at session start:
- Client is notified: `MODE_SWITCH { target: 2R, reason: NO_SILICON }`
- Session continues in 2R mode with SW fallback
- User sees a status message: "Game mode requires hardware encoder"

---

## Mode Transition Rules

```
          ┌─────────────────────────────────────────────┐
          │                                             │
    ┌─────▼──────┐   motion↑          ┌────────────────▼──────┐
    │     AR     │ ─────────────────► │          2R           │
    │  (static)  │ ◄───────────────── │       (dynamic)       │
    └────────────┘   motion↓ 5s idle  └──────────┬────────────┘
                                                  │        ▲
                                         game↑   │        │  game↓
                                      silicon✓   │        │  or minimize
                                                  ▼        │
                                         ┌────────────────────────┐
                                         │           47           │
                                         │        (gaming)        │
                                         └────────────────────────┘

All transitions are signaled via MODE_SWITCH packet.
Client decoder is mode-agnostic — receives any mode transparently.
```

---

## Summary Table

| Property | AR | 2R | 47 |
|----------|----|----|-----|
| Named for | AR**t**ur | a**r**tu**r** | artur **47** |
| Mode byte | `0x01` | `0x02` | `0x47` |
| Bandwidth min | **10 KB/s** | 200 KB/s | 500 KB/s |
| Bandwidth max | 5 MB/s | 8 MB/s | **uncapped** |
| Max FPS | 60 | 60 | **120** |
| Max resolution | 1080p | 1440p | **4K** |
| Latency target | 30ms | 20ms | **≤8ms** |
| Lossless? | **Yes** | No (adaptive) | No (VBR) |
| Silicon required | No | No | **Yes** |
| FEC | Yes | Yes | No |
| XOR-delta tiles | Yes | Partial | **No** |
| Best for | Support/IT | Video/Demos | **Gaming** |

---

*AR2R47 = ARTUR 47. The codec carries the name of its creator,  
Arthur Valiev, who built EVRT and EVRTCK from the ground up.*
