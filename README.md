# EvertyDesk Lite

EvertyDesk Lite is a minimal Rust + egui remote desktop client for the EvertyDesk infrastructure.

The goal is simple: a small, fast, native remote access tool that starts quickly, has no heavy UI stack, and can be built for Windows and Linux, including conservative enterprise Linux environments such as Astra Linux.

This repository contains only the EvertyDesk Lite client. The `rustdesk-master/` directory is used locally as protocol documentation only and is intentionally ignored by Git.

## Why EvertyDesk Lite

Remote desktop tools became too heavy for many everyday support tasks. EvertyDesk Lite takes the opposite path:

- minimal native UI built with Rust + egui;
- direct EvertyDesk server configuration;
- RustDesk-compatible rendezvous/relay protocol work;
- separate remote desktop viewer window;
- screenshot-based remote image pipeline;
- mouse, keyboard, text input, display switching, and connection diagnostics;
- simple codebase that can grow into a controlled support platform.

This is the first public step. Give it a little time, and EvertyDesk will become more than a remote desktop client: it will become a compact automation workstation for support teams, administrators, integrators, and field engineers.

## Current Features

- Native Rust + egui desktop app.
- EvertyDesk server settings built in.
- Connection progress with clear diagnostics.
- Remote desktop viewer in a separate window.
- Working remote image display through an optimized screenshot pipeline.
- Mouse control with coordinate modes for different monitor layouts.
- Keyboard input, text send, Enter, Ctrl+Alt+Del, Lock.
- Multi-display selection.
- Auto refresh with configurable interval.
- Recent remote IDs.
- Clean disconnect and window close handling.
- Background PNG decode and texture reuse for smoother UI.
- Optional `live-h264` build feature prepared for the next video-decoding stage.

## Current Status

EvertyDesk Lite is now a working minimal client. It connects through the EvertyDesk/RustDesk-compatible server path, opens a remote screen window, displays the desktop, and sends mouse/keyboard input.

The current image path is intentionally simple: fast screenshot refresh. This keeps the client small and portable while the live video decoder is being implemented.

## Server Defaults

```text
API URL:     https://desk.everty.ru
ID server:   edesk.server1.everty.ru
Relay server: edesk.server1.everty.ru
Public key:  MrGdbay3g8Qr84YYnxr4qLjw5zLWM1oAOdfehbBnlRs=
```

## Run

```powershell
cargo run
```

## Build

Debug build:

```powershell
cargo build
```

Release build:

```powershell
cargo build --release
```

Release binary:

```text
target\release\evertydesk-lite.exe
```

Experimental H264 build:

```powershell
cargo build --features live-h264
```

The `live-h264` feature is the prepared path for real video decoding. It is not the default because EvertyDesk Lite must stay easy to build on conservative Linux systems.

## Linux Notes

The client is designed to stay lightweight and portable. For Linux builds, install the usual Rust desktop dependencies for `eframe`/`egui` and your distribution graphics stack.

The long-term target is a simple package flow for:

- Windows;
- common Linux distributions;
- Astra Linux;
- portable admin builds.

## Roadmap

Next planned direction:

- H264 live video decoding through the prepared `live-h264` feature.
- VP8/VP9 investigation only if it can be kept portable and simple.
- Better keyboard mapping for mixed Windows/Linux sessions.
- Terminal inside EvertyDesk Lite.
- Script runner for support and administration tasks.
- Automation templates for common service operations.
- Automation Store: a curated marketplace of safe scripts, checks, repairs, deployment steps, and customer-specific workflows.
- Role-based automation access.
- Audit log for remote actions.
- One-click diagnostics bundle.
- File transfer.
- Session recording and support reports.

## Vision

EvertyDesk should not be just another remote desktop window.

The larger idea is a remote workbench:

- connect to a machine;
- see the desktop;
- control it;
- open terminal;
- run approved scripts;
- install or repair software;
- collect diagnostics;
- share automation packages through a store;
- keep everything small, auditable, and understandable.

EvertyDesk Lite is the minimal client that starts this path.

## Repository Policy

- Do not commit `rustdesk-master/`.
- Do not commit `target/`.
- Do not commit local passwords, config files, logs, or `.env` files.
- Keep the client small and practical.
