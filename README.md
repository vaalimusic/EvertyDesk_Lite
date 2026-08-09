# EvertyDesk

Native remote-desktop software written in Rust. Author: Arthur Valiev.
Not a fork of RustDesk — but it speaks RustDesk's rendezvous/relay
protocol, so it can talk to existing RustDesk-compatible infrastructure
if you already have it.

License: [The Valiev License](LICENSE) (MIT-based).

## The current client: EvertyDesk Next

`desktop-next/` is the actively developed client — Windows and macOS.
Fully native Rust, built on [Iced](https://iced.rs) instead of egui,
split into two independent processes:

- **Launcher** (`evertydesk-launcher`) — connection setup, contacts,
  recent connections, host mode, session management.
- **Viewer** (`evertydesk-viewer`) — its own `winit` + WGPU process:
  owns the event loop, the outgoing session, the decoder, input capture,
  and presentation. Crashing or closing one never takes down the other.
- **RDP console** (`evertydesk-rdp-viewer`) — a third, much smaller
  process for connecting straight to a VM's RDP endpoint (currently
  Hyper-V Enhanced Session), completely separate from EvertyDesk's own
  transport.

See [`desktop-next/README.md`](desktop-next/README.md) for the process
architecture and IPC protocol, and
[`desktop-next/RELEASE.md`](desktop-next/RELEASE.md) for how it's
packaged and released.

Linux isn't built by desktop-next yet — see "Archived: EvertyDesk Lite"
below for what still runs there today.

## The codec: EVRTCK

A custom lossless tiled codec — 32×32 tiles, XOR-diff against the
previous frame, ZRLE/zstd on top. Purpose-built for the common case of
a remote desktop session: mostly-static screens with localized changes
(typing, cursor movement, a window redraw), not full-motion video.

Real numbers, 1920×1080, single core, no hardware acceleration
(`cargo bench --bench evrtck_bench`; full scenarios in
`benches/evrtck_bench.rs`):

| Scenario | Payload | Encode |
|---|---|---|
| Static screen | 20 bytes | 1.6 ms |
| Light UI activity (5% dirty) | ~1 KB | 2.9 ms |
| Typing/cursor (15% dirty) | ~2.4 KB | 3.5 ms |
| Heavy redraw (90% dirty, low-entropy) | ~13 KB | 3.4 ms |

Those numbers hold for low-entropy content — text, UI chrome, cursor
motion. Against real photographic/video noise the picture changes
completely (90% dirty with actual noise costs ~6.4 MB, barely better
than raw) — which is exactly why EVRTCK is the default for everyday
desktop/support use, not a general video codec. For full-motion content
(games, video playback), EvertyDesk also supports hardware-accelerated
H.264/H.265/VP8/VP9/AV1 via NVENC, Windows Media Foundation, and
openh264, RustDesk-protocol-compatible.

For publishable EVRTCK numbers, use the production benchmark runner instead
of copying ad-hoc Criterion output:

```powershell
.\scripts\run-evrtck-prod-bench.ps1 -Iterations 300 -Warmup 30
```

It writes metadata, CSV, and JSONL reports with CPU/OS/rustc/commit hash,
payload sizes, p50/p95/p99, and separate encode/decode/roundtrip timings.
See [`docs/EVRTCK_BENCHMARKING.md`](docs/EVRTCK_BENCHMARKING.md).

## Transport: EVRT

A UDP protocol (24-byte header, MTU-safe) with a feedback loop and
adaptive buffering, originally built for a separate game-streaming
project and ported here. Used for both the low-latency game-mode path
and, via a shared transport layer, EVRTCK's own tile updates.

## Repository layout

```
src/                 Shared core (transport, EVRT, codecs, capture, VM backends)
desktop-next/         EvertyDesk Next — the current client
android/               Android client (JNI bridge into the same core)
vendor/                 Forked ironrdp-bulk / ironrdp-session (VirtualBox VRDE fixes)
ironrdp-viewer/          Standalone reference RDP client, used for protocol testing only
benches/                 Criterion benchmarks (EVRTCK)
scripts/                 Build/packaging scripts
```

## Archived: EvertyDesk Lite (egui)

`src/main.rs` — the original egui/eframe-based desktop client — is
**archived**. It's still in the repository and still builds, but it is
not maintained going forward; all new development happens in
`desktop-next/`. See the notice at the top of `src/main.rs` itself.

## Building

Desktop Next (Windows/macOS):

```powershell
cargo build --manifest-path desktop-next/Cargo.toml --bin evertydesk-launcher --bin evertydesk-viewer --features viewer-core
```

The archived egui client (`src/main.rs`, Windows/Linux, unmaintained):

```bash
cargo build --release
```

Android: see `android/`.

No GitHub CI builds the archived client — only `desktop-next/` has CI
(`.github/workflows/`).
