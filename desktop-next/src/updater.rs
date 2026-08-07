//! Self-hosted auto-update: checks a small JSON manifest, downloads the new
//! installer, and verifies its SHA-256 before handing control to the user.
//!
//! This deliberately never applies an update silently. A remote-desktop
//! client can be mid-session when a check happens; the UI surfaces
//! "update available" and lets the user pick when to download/install.
//!
//! Manifest contract (hosted wherever you like — S3, GitHub Releases, your
//! own server — set the URL via `UpdateChannel::url`):
//!
//! ```json
//! {
//!   "version": "0.2.0",
//!   "notes": "Fixes the thing.",
//!   "windows": { "url": "https://.../evertydesk-lite-0.2.0-x64.msi", "sha256": "..." },
//!   "macos":   { "url": "https://.../evertydesk-lite-0.2.0.dmg",     "sha256": "..." }
//! }
//! ```
//!
//! Only the platform section for the current OS is required; the others may
//! be omitted or left present for other builds fetching the same manifest.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Manifests are tiny hand-written JSON; anything past this is not a manifest.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Installers are tens of MB; guard against a misconfigured URL streaming
/// something unbounded into this process.
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    windows: Option<PlatformArtifact>,
    #[serde(default)]
    macos: Option<PlatformArtifact>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlatformArtifact {
    url: String,
    sha256: String,
}

impl UpdateManifest {
    fn artifact_for_current_platform(&self) -> Option<&PlatformArtifact> {
        if cfg!(target_os = "windows") {
            self.windows.as_ref()
        } else if cfg!(target_os = "macos") {
            self.macos.as_ref()
        } else {
            None
        }
    }
}

/// Result of a successful check: `None` means already up to date.
pub fn check_for_update(manifest_url: &str, current_version: &str) -> Result<Option<UpdateManifest>, String> {
    require_https(manifest_url)?;

    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
    let response = agent
        .get(manifest_url)
        .call()
        .map_err(|error| format!("не удалось проверить обновления: {error}"))?;

    let mut limited = response.into_reader().take(MAX_MANIFEST_BYTES);
    let mut body = String::new();
    limited
        .read_to_string(&mut body)
        .map_err(|error| format!("не удалось прочитать ответ сервера обновлений: {error}"))?;

    let manifest: UpdateManifest =
        serde_json::from_str(&body).map_err(|error| format!("некорректный манифест обновления: {error}"))?;

    let Some(artifact) = manifest.artifact_for_current_platform() else {
        return Err("манифест не содержит сборку для этой платформы".to_owned());
    };
    require_https(&artifact.url)?;

    if is_newer_version(&manifest.version, current_version) {
        Ok(Some(manifest))
    } else {
        Ok(None)
    }
}

/// Rejects anything but `https://`. A plaintext manifest is fully
/// attacker-controlled by any on-path network position — including the
/// `sha256` field it carries, so verifying the download's hash against a
/// hash the same attacker supplied provides no real integrity guarantee.
/// HTTPS is not optional here; there is no local-testing bypass.
fn require_https(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err("адрес обновления должен использовать https://".to_owned())
    }
}

/// Downloads the update artifact for the current platform into `destination_dir`,
/// verifies its SHA-256 against the manifest, and returns the verified file path.
/// The file is removed if verification fails, so a caller never sees a
/// half-downloaded or tampered installer on disk.
pub fn download_and_verify(
    manifest: &UpdateManifest,
    destination_dir: &Path,
) -> Result<PathBuf, String> {
    let artifact = manifest
        .artifact_for_current_platform()
        .ok_or_else(|| "манифест не содержит сборку для этой платформы".to_owned())?;
    require_https(&artifact.url)?;

    std::fs::create_dir_all(destination_dir)
        .map_err(|error| format!("не удалось создать папку загрузки: {error}"))?;

    let file_name = artifact
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("evertydesk-update");
    let destination = destination_dir.join(file_name);

    let agent = ureq::AgentBuilder::new().timeout(DOWNLOAD_TIMEOUT).build();
    let response = agent
        .get(&artifact.url)
        .call()
        .map_err(|error| format!("не удалось загрузить обновление: {error}"))?;

    let mut reader = response.into_reader().take(MAX_DOWNLOAD_BYTES);
    let mut file = std::fs::File::create(&destination)
        .map_err(|error| format!("не удалось создать файл загрузки: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("сбой при загрузке обновления: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if let Err(error) = file.write_all(&buffer[..read]) {
            drop(file);
            let _ = std::fs::remove_file(&destination);
            return Err(format!("не удалось записать файл загрузки: {error}"));
        }
    }
    drop(file);

    let digest = hex_encode(&hasher.finalize());
    if !digest.eq_ignore_ascii_case(&artifact.sha256) {
        let _ = std::fs::remove_file(&destination);
        return Err(format!(
            "контрольная сумма не совпадает (ожидалась {}, получена {digest}) — обновление удалено",
            artifact.sha256
        ));
    }

    Ok(destination)
}

/// Launches the verified installer and lets the user complete it — this
/// process does not attempt to self-replace or elevate silently.
pub fn launch_installer(installer_path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new(installer_path)
            .spawn()
            .map_err(|error| format!("не удалось запустить установщик: {error}"))?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(installer_path)
            .spawn()
            .map_err(|error| format!("не удалось открыть образ диска: {error}"))?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = installer_path;
        Err("автообновление не поддерживается на этой платформе".to_owned())
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Simple semver-ish comparison: numeric dot components, left to right.
/// A malformed/missing component is treated as `0`, so `"0.2"` < `"0.2.1"`.
fn is_newer_version(candidate: &str, current: &str) -> bool {
    let parse = |raw: &str| -> Vec<u64> {
        raw.trim_start_matches('v')
            .split(['.', '-', '+'])
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let candidate_parts = parse(candidate);
    let current_parts = parse(current);
    let len = candidate_parts.len().max(current_parts.len());
    for index in 0..len {
        let candidate_value = candidate_parts.get(index).copied().unwrap_or(0);
        let current_value = current_parts.get(index).copied().unwrap_or(0);
        if candidate_value != current_value {
            return candidate_value > current_value;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_is_numeric_not_lexical() {
        assert!(is_newer_version("0.10.0", "0.9.0"));
        assert!(!is_newer_version("0.9.0", "0.10.0"));
        assert!(is_newer_version("0.2.1", "0.2"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(is_newer_version("v1.0.0", "0.9.9"));
    }

    #[test]
    fn manifest_without_current_platform_artifact_is_rejected() {
        let manifest = UpdateManifest {
            version: "9.9.9".to_owned(),
            notes: String::new(),
            windows: None,
            macos: None,
        };
        assert!(manifest.artifact_for_current_platform().is_none());
    }

    #[test]
    fn hex_encode_matches_known_sha256() {
        // sha256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let digest = Sha256::digest(b"");
        assert_eq!(
            hex_encode(&digest),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn plaintext_update_urls_are_rejected() {
        assert!(require_https("http://example.com/latest.json").is_err());
        assert!(require_https("ftp://example.com/latest.json").is_err());
        assert!(require_https("example.com/latest.json").is_err());
        assert!(require_https("https://example.com/latest.json").is_ok());
    }

    #[test]
    fn check_for_update_rejects_a_plaintext_manifest_url_before_any_request() {
        let result = check_for_update("http://example.com/latest.json", "0.1.0");
        assert!(result.is_err());
    }
}
