# EvertyDesk Desktop Next

This package is the migration boundary between the low-frequency product UI
and the latency-sensitive remote desktop viewer.

## Processes

- `evertydesk-launcher`: Iced launcher for connection setup, contacts, recent
  connections, live session management and, later, tray and account management.
- `evertydesk-viewer`: independent `winit` + WGPU process (own
  `frame_renderer::FrameRenderer`, not the `pixels` crate — see C1 in
  `RELEASE.md`'s history). It owns its event loop, outgoing
  `TransportClient` session, decoder, input capture, and presentation.
- `evertydesk-rdp-viewer`: a third, much smaller process for connecting
  straight to a VM's RDP endpoint (currently Hyper-V Enhanced Session),
  bypassing EvertyDesk's own transport entirely.

This is the actively developed client (EvertyDesk Next). The original
egui/eframe client (`evertydesk-lite`, `src/main.rs` at the repo root) is
archived — still builds, not maintained — see the notice at the top of that
file and the root README.md.

## Build and run

```powershell
cargo build --manifest-path desktop-next/Cargo.toml --bin evertydesk-launcher --features viewer-core
cargo build --manifest-path desktop-next/Cargo.toml --bin evertydesk-viewer --features viewer-core
cargo run --manifest-path desktop-next/Cargo.toml --bin evertydesk-launcher --features viewer-core
```

The launcher looks for `evertydesk-viewer` next to its own executable. Set
`EVERTYDESK_VIEWER_PATH` to override that path during development.

Only one launcher instance is allowed per machine. The primary process owns a
loopback-only listener with a bounded, versioned request/response handshake and
500 ms I/O timeouts. A later launch asks the primary process to restore and
focus its existing window, then exits before it can create another tray icon,
host registration, or launcher data writer.

Launcher and viewer exchange newline-delimited JSON over redirected standard
streams. Both directions reject malformed UTF-8/JSON and enforce a 64 KiB
per-message limit before allocating or sending an IPC payload. A protocol
violation terminates that viewer session and is surfaced in the launcher
instead of leaving a partially controlled child process running.
Long-lived launcher commands are never written to the child pipe on the Iced UI
thread. Each viewer owns a dedicated stdin writer with a bounded 32-message
queue. Enqueue is non-blocking: saturation returns backpressure immediately,
and a terminated writer reports a broken control channel instead of freezing
the launcher or allowing unbounded memory growth.
Viewer-to-launcher status output follows the same isolation rule: a dedicated
stdout writer drains a bounded 64-message queue, so a launcher that stops
reading cannot block the Winit event loop or transport workers. Routine
telemetry is dropped immediately under saturation. Final session summary and
closed events receive a bounded 250 ms enqueue window followed by a bounded
flush, preserving normal shutdown data without permitting an infinite exit
hang.

Viewer diagnostics use a separate redirected stderr channel. Each line is
limited to 4 KiB, stripped of control characters, and shortened to 320
characters. The launcher keeps only the latest eight lines in memory and shows
the last one when a viewer exits abnormally; diagnostics are never persisted.

## Launcher data

Contacts and recent remote IDs are saved as JSON in
`%APPDATA%\EvertyDesk\launcher.json` on Windows. Passwords and other credentials
are never persisted. Writes are limited to 1 MiB and committed through a
temporary file while the previous valid version is retained as
`launcher.json.bak`. If the primary file is missing or invalid, the launcher
restores that backup automatically; a damaged primary is kept as a timestamped
`launcher.corrupt-*.json` file for diagnostics.

The launcher persists one of three connection profiles under Settings →
Outgoing connections and passes it to the viewer in the bootstrap message.
The Home screen keeps the primary connect flow free of profile controls:

- **Smooth**: 60 FPS with adaptive quality.
- **Balanced**: 45 FPS with adaptive quality.
- **Sharp**: 30 FPS with adaptive quality disabled.

Changing the profile also updates every active viewer over the control IPC.
The transport applies the new frame-rate and adaptive-quality policy without
restarting the session, and the profile remains active after auto-reconnect.
Active-session controls can request a fresh video stream or reconnect the
transport inside the existing viewer window. Viewer telemetry reports input
FPS, bitrate, and accumulated dropped frames back to the launcher.
Each active session can switch to view-only mode, disable clipboard exchange,
cycle to the next remote display, or toggle between smooth aspect-preserving fit
and integer pixel-perfect scaling at runtime. The scaling command updates the
existing WGPU surface without reconnecting, while `pixels::window_pos_to_pixel`
keeps mouse coordinates outside letterbox margins from reaching the remote.
`Ctrl+Alt+M` toggles scaling inside the viewer. Disabling input releases held
keys and mouse buttons before the viewer begins ignoring remote input, while
local viewer shortcuts and clipboard synchronization remain available.
An occluded viewer temporarily reduces the remote stream to 5 FPS and restores
the selected profile plus a fresh-frame request when visible again. A lightweight
frame watchdog requests recovery after 15 seconds without a new frame, but only
while the viewer is visible and connected. Telemetry also includes total session
duration and the number of reconnects.
Before data reaches WGPU or the system clipboard, the viewer validates exact
RGBA lengths and enforces resource limits: 16,384 pixels/256 MiB for a frame,
512 pixels/1 MiB for a cursor, 256 cached cursor shapes, and 1 MiB for clipboard
text in either direction. Rejected video requests a fresh frame and every
rejection is reported to the launcher instead of being silently accepted.
While a session is connected, a dedicated low-frequency watcher detects local
text-clipboard changes and forwards each distinct value automatically. It
pauses when clipboard synchronization is disabled or the transport disconnects,
rescans after reconnect, and exits with the viewer. Content fingerprints plus
the existing sent/received value checks prevent polling duplicates and remote
clipboard echo loops.

Starting a connection to a device that already has an active viewer now restores
and focuses that window instead of opening a duplicate transport. Before final
shutdown, the viewer publishes a session summary; recent-history entries persist
the completed duration and reconnect count with backward-compatible defaults.
Disconnecting from the launcher keeps the process tracked until its summary and
final status arrive. Transport loss remains a reconnecting state rather than a
false process-close event. The launcher waits briefly for the real child exit
status, distinguishes requested, clean, crashed, and unexpectedly lost exits,
and includes bounded stderr diagnostics only for abnormal outcomes. Tray Exit
sends disconnect to every viewer before launcher resources are released.
The launcher allows at most eight concurrent viewer processes. Each new process
must publish its first valid IPC status within eight seconds, while a requested
disconnect has five seconds to finish normally. Tokenized watchdog events cannot
terminate a later process if an operating-system process ID is reused; a viewer
that misses either deadline is removed and force-stopped by the process guard.
Runtime changes for input control, clipboard, quality, and scaling use typed
viewer acknowledgements instead of optimistic launcher state. A setting remains
pending until the exact requested value is confirmed, duplicate commands of the
same kind are blocked, and a three-second acknowledgement timeout reports a
desynchronization without pretending that the setting was applied. This extends
the bootstrap/IPC contract to protocol version 4.
The viewer also posts a lightweight heartbeat through the Winit event queue
every two seconds. Because the event loop itself must handle the pulse before it
is published over IPC, a frozen window cannot appear healthy merely because a
background thread is still alive. The launcher uses a seven-second,
session-token and sequence-guarded liveness deadline; newer heartbeats, stale
timers, and sessions already shutting down are ignored, while an unresponsive
viewer is boundedly terminated.

The "This device" card uses the existing core `AppConfig` identity. It can
start and stop the real `HostService`, copy the local ID or password, regenerate
the password, and approve or reject incoming sessions. Host events are bridged
into the same event-driven Iced subscription as viewer events.
Copied access passwords are removed from the system clipboard after 30 seconds
when the clipboard still contains that password; copying newer content cancels
the pending cleanup. Incoming approval prompts carry generation tokens and are
rejected by the launcher after 40 seconds, before the core host's 45-second
deadline, so a stale prompt cannot approve a newer request.
Once an incoming session starts, the local card shows the connected peer and
can block/unblock that peer's keyboard and mouse events or disconnect it
immediately. The input lock is reset both by the launcher and inside the core
host when a session ends or a new one starts, so an abnormal disconnect cannot
silently leave the next client in a blocked state.
An approval request restores and focuses a launcher that was minimized to the
system tray. The tray's disabled status row and tooltip distinguish stopped,
ready, approval-required, active, and input-blocked states. If another peer asks
for approval while a request is already pending, the old request is explicitly
rejected; new requests are rejected as busy while a session is active.

The header separates the product into Home, Devices, and Settings screens.
Security settings update `AppConfig` and reconfigure a running host immediately:
incoming confirmation, keyboard/mouse control, clipboard synchronization, and
automatic hosting on the next launcher start.

The main layout follows the familiar remote-desktop workflow: a persistent
quick-connect address bar below the application header, a prominent local
workspace ID, compact access controls, and device/session sections below it.
The content expands on wide displays while retaining a bounded readable width.
The Address Book is a dedicated launcher section. Contacts support persistent
favorites, optional groups and notes, in-place editing, grouped display, and
filtering by name, ID, group, or note. Existing launcher JSON remains
compatible because group and note fields default to empty values. Connection
history stays local and passwords are never stored in contact records.

## System tray

On Windows the launcher creates a native tray icon without polling. Its menu
can restore the window, start or stop incoming access, or explicitly exit.
Clicking the window close button minimizes EvertyDesk instead of terminating
the host; only the tray **Exit** action performs a full shutdown.

## Security boundary

The launcher serializes `ViewerBootstrap` as the first JSON line on the viewer's
stdin. Passwords are never placed in command-line arguments. The same stdin
remains open for launcher commands such as graceful disconnect and fullscreen.
The viewer publishes tagged JSON status lines on stdout (`starting`, progress,
connected, latency, reconnecting, failure, and closed). Transient network
failures are retried inside the same viewer window with a bounded 5/10/20/40
second backoff. Authentication failures and unknown device IDs are not retried.

Iced consumes those status lines through an event-driven `Subscription` backed
by an async channel. No process-status timer or polling loop is used. The
launcher displays each live viewer process and can disconnect or toggle
fullscreen independently.

## Rendering migration

The current viewer consumes `SessionEvent::Frame` from `evertydesk-core`, keeps
only the newest RGBA frame, and wakes the Winit event loop with an
`EventLoopProxy`. There is no frame polling and stale decoded frames cannot pile
up in the UI queue. Mouse buttons, pointer movement, wheel input, printable
keyboard input, navigation keys, function keys, and modifier state are forwarded
as `SessionCommand` values.

Remote cursor bitmaps are installed as native Winit cursors instead of forcing a
full-frame redraw on pointer movement. Clipboard updates received from the peer
are applied locally when allowed by the existing security policy; pressing
`Ctrl+V` synchronizes the local clipboard before forwarding the paste shortcut.
Remote input is accepted only while the viewer has focus. Losing focus,
disconnecting, or closing the window releases held modifiers and mouse buttons
so drag operations cannot leave input stuck on the remote computer.

Viewer shortcuts:

- `Alt+Enter`: enter or leave borderless fullscreen.
- `Ctrl+Alt+Left/Right`: switch between remote displays.
- `Ctrl+Alt+S`: save the latest received frame as a PNG under
  `Pictures\EvertyDesk`.

The viewer also renders a centered AnyDesk-style session toolbar over the top
edge of the remote frame. Its seven hit-tested controls provide fullscreen,
display switching, scaling, input enable/disable, clipboard enable/disable,
screenshot, and disconnect actions. Pointer and wheel events inside the toolbar
are consumed locally, and entering it releases held remote buttons/modifiers so
an active drag cannot become stuck. Viewer-originated input, clipboard, and
scaling changes are reported back to the launcher as authoritative control
state under IPC protocol version 5.

`pixels` remains deliberately an initial CPU-RGBA backend. The viewer API does
not depend on it outside the presentation layer. The production path will
replace it with direct WGPU YUV textures and platform-native decoded textures
where available.
