# EvertyDesk Lite Roadmap

## Stage 1: Working Lite Client

Status: active.

- Minimal Rust + egui UI.
- EvertyDesk server connection.
- Remote screen window.
- Screenshot-based image pipeline.
- Mouse and keyboard input.
- Multi-display selection.
- Clean disconnect and window handling.

## Stage 2: Speed

Status: in progress.

- Background frame decode.
- Texture reuse instead of texture recreation.
- Aggressive screenshot refresh.
- Prepared `live-h264` feature.
- Next: decode H264 frames into RGBA and render them directly.

## Stage 3: Remote Workbench

Planned.

- Built-in terminal.
- Script runner.
- Diagnostics collection.
- File transfer.
- Session action history.

## Stage 4: Automation Store

Planned.

- Curated automation packages.
- Safe script templates.
- Role-based automation access.
- Customer-specific workflow bundles.
- Audit log for every automated action.

