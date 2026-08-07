# Release engineering (desktop-next)

Covers packaging, auto-update, and CI for the `desktop-next` client
(Windows + macOS — Linux stays on the old egui client per the D1 decision,
and isn't built or packaged here).

## Status

| Piece | State |
|---|---|
| Windows installer (MSI via WiX v5) | Written, **not run** — no WiX toolset in the dev sandbox that built this |
| macOS installer (.app + DMG) | Written, **not run** — no macOS available in the dev sandbox |
| Code signing (Authenticode / notarization) | Not started — no certificate yet |
| Auto-update (check/download/verify) | Implemented in `src/updater.rs`, unit-tested, wired into the launcher's Settings → General panel |
| CI (`.github/workflows/ci.yml`) | Build + test + clippy on every push/PR, windows-latest + macos-latest |
| Release automation (`.github/workflows/release.yml`) | Package + draft GitHub Release on `vX.Y.Z` tag push |

Everything above compiles and the auto-updater's logic is unit-tested, but
the **installer builds themselves have never actually run** — this
environment has neither the WiX toolset nor a macOS machine. Treat the first
real CI run as the actual first test of `wix/main.wxs` and
`scripts/package-macos-dmg.sh`, and expect to iterate.

## Cutting a release

1. Bump `version` in `desktop-next/Cargo.toml`.
2. Commit, tag `vX.Y.Z`, push the tag.
3. `release.yml` builds both installers and opens a **draft** GitHub Release
   with them attached — review and publish it manually (nothing goes out
   automatically).
4. Update your hosted `latest.json` (see below) with the new version and the
   `.sha256` files the workflow produced. Auto-update won't offer the new
   version to existing installs until you do this.

Building locally instead of via CI:
```powershell
# Windows — needs: dotnet tool install --global wix ; wix extension add WixToolset.UI.wixext
.\scripts\package-windows-installer.ps1
```
```bash
# macOS
./scripts/package-macos-dmg.sh
```

## Signing (not done yet)

- **Windows**: needs an Authenticode certificate (EV recommended — avoids
  the SmartScreen reputation ramp-up that a standard cert still gets warned
  on for a while). Once you have one, sign the MSI (or better, sign
  `evertydesk-launcher.exe`/`evertydesk-viewer.exe` themselves *before*
  `wix build` packages them) with `signtool sign /fd sha256 /tr <timestamp-url> /td sha256 ...`.
- **macOS**: needs an Apple Developer ID Application certificate. Sign the
  `.app` (`codesign --deep --sign "Developer ID Application: ..." ...`)
  before `hdiutil create`, then notarize the DMG (`xcrun notarytool submit
  ... --wait`) and staple the ticket (`xcrun stapler staple`).
- Until both exist, `dist/` output is unsigned and the CI workflows don't
  attempt either step — wiring them in is a matter of adding the signing
  commands to the two package scripts once secrets/certs are available.

## Auto-update manifest contract

Host this JSON wherever you like (S3, your own server, a GitHub Release
asset) and point the client at it via the `EVERTYDESK_UPDATE_URL`
environment variable (unset by default — update checks are a no-op until
this is configured, deliberately, so nothing points at a nonexistent
endpoint out of the box):

```json
{
  "version": "0.2.0",
  "notes": "Fixes the thing.",
  "windows": { "url": "https://.../EvertyDeskLite-0.2.0-x64.msi", "sha256": "<hex>" },
  "macos":   { "url": "https://.../EvertyDeskLite-0.2.0.dmg",     "sha256": "<hex>" }
}
```

Both `windows`/`macos` sections are optional — the client only requires the
one matching its own OS. `sha256` is mandatory and enforced: the downloaded
file is hashed and deleted if it doesn't match before the user is ever
offered "Install". Both the manifest URL and every artifact URL inside it
**must** be `https://` — this is enforced unconditionally, with no
local-testing bypass, because a plaintext manifest is fully attacker-
controlled by anyone on-path, including the `sha256` it claims to be
correct.

The client (`src/updater.rs`):
- Checks at most every 6 hours in the background, plus on-demand via the
  "Проверить обновления" button in Settings → General.
- Never applies an update silently. It surfaces "update available", and on
  "Скачать и проверить" downloads to `%LOCALAPPDATA%\EvertyDesk\Updates`
  (Windows) / `~/.cache/evertydesk/updates` (macOS/Linux fallback),
  verifies the hash, and only then offers "Установить", which launches the
  installer/DMG and lets the user finish it — no self-replacement, no
  forced restart.
- Version comparison is numeric-dot-separated (`0.10.0` > `0.9.0`), not
  lexical string comparison.

## CI

- `ci.yml`: on every push/PR touching `desktop-next/` or the shared core —
  `cargo build`, `cargo test`, `cargo clippy -D warnings`, windows-latest +
  macos-latest matrix.
- `release.yml`: on `vX.Y.Z` tag push — builds both installers, uploads them
  as workflow artifacts, and opens a draft GitHub Release with everything
  attached via `softprops/action-gh-release`.

Both are scoped to `desktop-next` only. The old egui client (`src/main.rs`)
is not built by CI, matching the existing "builds locally" convention
documented in the repo's `CLAUDE.md`.
