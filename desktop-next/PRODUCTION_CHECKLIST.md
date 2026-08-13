# EvertyDesk Next 2 production checklist

Target version: `2.0.1`

This checklist tracks what must be true before publishing EvertyDesk Next 2 as a production build.

## Current verified state

- `cargo test --features viewer-core --bins --lib` passes locally on Windows.
- `cargo check --release --features viewer-core --bins` passes locally on Windows.
- `cargo clippy --features viewer-core --bins -- -D warnings` passes locally for the desktop-next binaries.
- Release binaries exist for:
  - `evertydesk-launcher`
  - `evertydesk-viewer`
  - `evertydesk-rdp-viewer`
- Windows MSI and macOS DMG scripts include all three binaries.
- Windows portable zip packaging is available for tester builds without WiX/MSI.
- `scripts/generate-update-manifest.ps1` generates HTTPS `latest.json` with computed SHA-256 values.
- `scripts/validate-windows-installer-layout.ps1` validates MSI inputs, WiX layout, shortcuts, product icon, upgrade rule, and WiX text encoding before packaging.
- `scripts/validate-release-artifacts.ps1` validates release artifacts, sidecar `.sha256` files, manifest consistency, and portable zip layout before publishing.
- `scripts/package-windows-portable.ps1` runs release-artifact validation after creating the portable zip, skipping manifest checks until `latest.json` is regenerated.
- Windows resources are present: `assets/logo.ico`, tray RGBA assets, viewer icon assets.
- Auto-update logic is implemented and unit-tested, but disabled unless `EVERTYDESK_UPDATE_URL` is configured.
- Update downloads are written to a temporary `.part` file, size-limited, SHA-256 verified, and only then finalized as the installable MSI/DMG.
- Update manifests are validated for HTTPS artifact URLs, platform file extension, SHA-256 shape, bounded version, and bounded notes.
- Launcher/viewer IPC has bounded message sizes, acknowledgements, heartbeat/liveness, and process watchdogs.
- Local launcher store has bounded JSON size, transactional save, backup restore, and corrupt-file quarantine.
- `tools/portable-smoke.ps1 -Configuration release -StopAllExistingLaunchers` passed locally on Windows:
  release launcher starts from the intended directory, window responds, icon resource is present, viewer and RDP viewer are next to launcher, and smoke cleanup stops launcher/host-agent by default.
- `scripts/package-windows-portable.ps1 -StopRunningBuildProcesses` produced
  `dist/EvertyDeskNext-2.0.1-windows-x64-portable.zip` and `.sha256`.
- Extracted portable zip passed `tools/portable-smoke.ps1 -BinaryDirectory <extracted-dir> -StopAllExistingLaunchers`.
- `latest.json` was generated and validated for the 2.0.1 portable artifact.
- Full EVRTCK software-path production benchmark passed on Windows/release:
  `reports/evrtck-prod/full-20260813-after-correctness`.
  Commit `1068c4d37b82926b7246e4482935c4e2bc12f3c0`, AMD Ryzen 5 4600G,
  6 cores / 12 threads, 1920x1080, 300 iterations / 30 warmup.
  The benchmark validates normal and hinted P-frames against the expected
  decoded RGBA output before timing.

## Release blockers

1. Run the Windows MSI package job for real.
   - Required: WiX CLI `5.0.2` and `WixToolset.UI.wixext/5.0.2`.
   - Expected output: `dist/EvertyDeskNext-2.0.1-x64.msi` and `.sha256`.

2. Run the macOS package job on macOS for real.
   - The DMG path is implemented, but cannot be validated on Windows.
   - Verify `.app` launch, icon, permissions prompts, and DMG mounting.

3. Code signing.
   - Windows: sign `evertydesk-launcher.exe`, `evertydesk-viewer.exe`, `evertydesk-rdp-viewer.exe`, then the MSI.
   - macOS: Developer ID signing, notarization, and stapling.
   - Without signing, Windows SmartScreen and macOS Gatekeeper warnings are expected.

4. Smart Agent production endpoint contract.
   - Confirm `smart-agent-api.md` matches the live `desk.everty.ru` API.
   - Confirm service key handling, heartbeat interval, support request limits, and AB0 behavior.
   - Fix any mojibake/encoding damage in docs before publishing them externally.

5. RustDesk-compatible server settings.
   - Verify custom API URL, ID server, relay server, and public key settings against a non-default test deployment.
   - Confirm default EvertyDesk server values remain hidden in UI and restored when fields are empty.

6. EVRTCK production evidence.
   - Done: full software benchmark without `-Quick`:
     `.\scripts\run-evrtck-prod-bench.ps1 -Iterations 300 -Warmup 30 -OutDir reports\evrtck-prod\full-20260813-after-correctness`.
   - Done: CPU model, thread count, clocks, release profile, commit hash, payload sizes, p50/p95/p99 encode/decode/roundtrip are recorded in the report.
   - Done: software scenarios include static, clustered/scattered dirty tiles, noisy dirty regions, IDE typing, browser scroll, and terminal scroll.
   - Done: attach `reports/evrtck-prod/full-20260813-after-correctness/metadata.json`, `evrtck_prod_bench.csv`, `evrtck_prod_bench.jsonl`, `summary.md`, and `summary.json`.
   - Still required: hardware H.264/H.265 comparison on the same scene set.
   - Scheduler note from the full run: static, invert deltas, and IDE typing are safe EVRTCK paths; scattered noise at 50%/90% should prefer hardware codec; browser scroll still needs hardware comparison or stronger copy-rect/scheduler policy because hinted roundtrip p99 was above the 60 FPS budget.

7. End-to-end session matrix.
   - Outgoing desktop session: LAN, WAN/relay, wrong ID, wrong password, reconnect, multi-monitor.
   - Incoming session: approval, timeout, reject, active-session controls, input lock, clipboard policy.
   - Game mode: own ID/password fields, low-latency profile, concrete codec selection.
   - VM mode: Hyper-V/RDP viewer path, VM inventory, provider filters, action labels.

8. Portable smoke test.
   - Run `tools/portable-smoke.ps1 -Configuration release -StopAllExistingLaunchers` after release build.
   - Verify no white-window startup regression, taskbar icon, tray icon, close-to-tray behavior.
   - Use `-LeaveRunning` only when you intentionally want to inspect the GUI manually after the smoke check.

9. Installer behavior.
   - Fresh install.
   - Upgrade from previous version.
   - Downgrade rejection.
   - Uninstall removes shortcuts and binaries.
   - User data under `%APPDATA%\EvertyDesk` remains intact.

9a. Portable package behavior.
   - Run `scripts/package-windows-portable.ps1 -StopRunningBuildProcesses`.
   - Extract zip into a clean directory.
   - Run `tools/portable-smoke.ps1 -BinaryDirectory <extracted-dir> -StopAllExistingLaunchers`.
   - Verify launcher locates viewer and RDP viewer next to itself.
   - Verify no writes happen inside the extracted portable directory except OS-created metadata.

10. Update manifest.
    - Run `scripts/generate-update-manifest.ps1` or use the release workflow generated `latest.json`.
    - Run `scripts/validate-release-artifacts.ps1` against the final `dist/` directory.
    - Host the generated manifest over HTTPS.
    - Include exact `sha256` values for MSI/DMG.
    - Verify update check, download, hash rejection, `.part` cleanup, and installer launch.

## Recommended release command sequence

```powershell
cd D:\github_project\EvertyDesk_Lite\desktop-next
cargo fmt --check
cargo test --features viewer-core --bins --lib
cargo clippy --features viewer-core --bins -- -D warnings
cargo build --release --features viewer-core --bins
.\scripts\validate-windows-installer-layout.ps1
.\tools\portable-smoke.ps1 -Configuration release -StopAllExistingLaunchers
.\scripts\package-windows-installer.ps1 -StopRunningBuildProcesses
.\scripts\package-windows-portable.ps1 -StopRunningBuildProcesses
```

Then run the macOS package script on macOS:

```bash
cd desktop-next
./scripts/package-macos-dmg.sh
```
