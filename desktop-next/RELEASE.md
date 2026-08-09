# Release engineering (desktop-next)

This document covers packaging, auto-update, and CI for the `desktop-next`
client.

Scope:

- Windows: launcher, native viewer, RDP/VM viewer, MSI, portable zip.
- macOS: launcher, native viewer, RDP/VM viewer, `.app`, DMG.
- Linux: not packaged here; it stays on the old egui client for now.

## Status

| Piece | State |
|---|---|
| Windows installer (MSI via WiX v5) | Written, not run locally because WiX is not installed in this dev environment |
| Windows portable zip | Implemented, built locally, smoke-tested after extraction |
| macOS installer (.app + DMG) | Written, not run locally because this environment is Windows |
| Code signing (Authenticode / notarization) | Not started; no certificate configured yet |
| Auto-update | Implemented in `src/updater.rs`, unit-tested, wired into Settings -> General |
| CI (`.github/workflows/ci.yml`) | Build + test + clippy on every push/PR, Windows + macOS matrix |
| Release automation (`.github/workflows/release.yml`) | Package + draft GitHub Release on `vX.Y.Z` tag push, generate `latest.json`, plus unattended monthly pre-release build |

The application binaries compile and the updater logic is unit-tested. The MSI
and DMG scripts still need their first real run in CI or on the target OS.
Expect to iterate on the first WiX/macOS packaging run.

## Cutting a release

1. Bump `version` in `desktop-next/Cargo.toml`.
2. Commit, tag `vX.Y.Z`, push the tag.
3. `release.yml` builds Windows/macOS packages and opens a draft GitHub Release.
4. Review the draft release manually before publishing.
5. Publish or copy the generated `latest.json` from the GitHub Release to the
   update endpoint used by `EVERTYDESK_UPDATE_URL`.

Local Windows packaging:

```powershell
cd D:\github_project\EvertyDesk_Lite\desktop-next

# Needs WiX:
#   dotnet tool install --global wix --version 5.0.2
#   wix extension add -g WixToolset.UI.wixext/5.0.2
# (the -g/global flag matters: a non-global extension add is cached
# relative to the current directory, and this script runs wix build from
# desktop-next/, not the repo root)
.\scripts\package-windows-installer.ps1 -StopRunningBuildProcesses

# Does not need WiX; useful for tester builds.
.\scripts\package-windows-portable.ps1 -StopRunningBuildProcesses

# Fast static MSI input/layout check; needs release binaries but not WiX.
.\scripts\validate-windows-installer-layout.ps1
```

Local macOS packaging:

```bash
cd desktop-next
./scripts/package-macos-dmg.sh
```

## Smoke checks

Run this after a release build:

```powershell
.\tools\portable-smoke.ps1 -Configuration release -StopAllExistingLaunchers
```

Run this after extracting the portable zip:

```powershell
.\tools\portable-smoke.ps1 -BinaryDirectory C:\path\to\extracted\EvertyDeskNext -StopAllExistingLaunchers
```

The smoke script verifies:

- `evertydesk-launcher.exe` exists.
- `evertydesk-viewer.exe` exists next to the launcher.
- `evertydesk-rdp-viewer.exe` exists next to the launcher.
- the launcher has an icon resource;
- the window starts and responds;
- exactly one primary launcher process is active for the tested path;
- the host-agent process starts;
- the tested processes are stopped by default after the check.

Use `-LeaveRunning` only when you intentionally want to inspect the GUI after
the smoke check.

## Signing

Windows:

- Sign `evertydesk-launcher.exe`, `evertydesk-viewer.exe`, and
  `evertydesk-rdp-viewer.exe` before MSI packaging.
- Sign the final MSI.
- Use a timestamp URL.
- Without signing, SmartScreen warnings are expected.

macOS:

- Sign the `.app` with Developer ID Application.
- Notarize the DMG.
- Staple the notarization ticket.
- Without signing/notarization, Gatekeeper warnings are expected.

## Auto-update manifest contract

The client reads an HTTPS JSON manifest from `EVERTYDESK_UPDATE_URL`. If the
environment variable is not set, update checks are a no-op by design.

Release automation generates `latest.json` from the packaged artifacts:

```powershell
.\scripts\generate-update-manifest.ps1 `
  -Version 2.0.0 `
  -BaseUrl https://github.com/<owner>/<repo>/releases/download/v2.0.0 `
  -DistDir .\dist `
  -OutFile .\dist\latest.json `
  -Notes v2.0.0
```

Validate release artifacts before publishing:

```powershell
.\scripts\validate-release-artifacts.ps1 `
  -Version 2.0.0 `
  -DistDir .\dist `
  -Manifest .\dist\latest.json `
  -RequireWindows `
  -RequireWindowsPortable
```

For portable archives, validation also opens the zip and checks that
`evertydesk-launcher.exe`, `evertydesk-viewer.exe`, `evertydesk-rdp-viewer.exe`,
and `README.txt` are present at the top level.

`package-windows-portable.ps1` runs that validation automatically after writing
the zip and `.sha256` sidecar, but deliberately skips `latest.json` because the
manifest is generated later by the release job.

Example:

```json
{
  "version": "2.0.0",
  "notes": "EvertyDesk Next 2 release.",
  "windows": {
    "url": "https://example.com/EvertyDeskNext-2.0.0-x64.msi",
    "sha256": "<hex>"
  },
  "macos": {
    "url": "https://example.com/EvertyDeskNext-2.0.0.dmg",
    "sha256": "<hex>"
  }
}
```

Rules enforced by the client:

- manifest URL must be HTTPS;
- artifact URLs must be HTTPS;
- SHA-256 is mandatory;
- downloaded files are deleted if the hash does not match;
- downloads are finalized only after SHA-256 verification;
- updates are never installed silently;
- version comparison is numeric-dot-separated, so `2.10.0` is newer than
  `2.9.0`.

## CI

`ci.yml` runs for `desktop-next/` and shared-core changes:

```powershell
cargo build --features viewer-core --bins
cargo test --features viewer-core --bins --lib
cargo clippy --features viewer-core --bins -- -D warnings
```

`release.yml` has two paths:

- Tag push `vX.Y.Z`: build packages and create a draft GitHub Release.
- Monthly schedule: build `master`, publish a GitHub pre-release versioned by
  date as `vYY.M.0`.

Both release paths pass `EVERTYDESK_RELEASE_VERSION` to the packaging scripts.
When the variable is not set, the scripts read `Cargo.toml`.

The publish job also attaches `latest.json`. The client uses only `windows`
and `macos`; extra fields such as `windows_portable` are for humans/testers and
are ignored by the updater.

## Current local artifact

The local portable package built during production-hardening:

- `dist/EvertyDeskNext-2.0.0-windows-x64-portable.zip`
- SHA-256:
  `9d2de1fc548a0335c61093b027e0dee1f1ca09f3a33971b9f373d493907bea00`

`dist/` is ignored by git and should not be committed.
