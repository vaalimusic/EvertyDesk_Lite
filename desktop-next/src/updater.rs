//! Self-hosted auto-update: checks a small JSON manifest, downloads the new
//! installer, and verifies its SHA-256 before handing control to the user.
//!
//! This deliberately never applies an update silently. A remote-desktop
//! client can be mid-session when a check happens; the UI surfaces
//! "update available" and lets the user pick when to download/install.
//!
//! Manifest contract (hosted wherever you like -- S3, GitHub Releases, your
//! own server -- set the URL via `UpdateChannel::url`):
//!
//! ```json
//! {
//!   "version": "2.0.0",
//!   "notes": "Fixes and improvements.",
//!   "windows": { "url": "https://.../EvertyDeskNext-2.0.0-x64.msi", "sha256": "..." },
//!   "macos":   { "url": "https://.../EvertyDeskNext-2.0.0.dmg",     "sha256": "..." }
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
const MAX_VERSION_BYTES: usize = 32;
const MAX_NOTES_BYTES: usize = 8 * 1024;
const GITHUB_API_BASE: &str = "https://api.github.com/repos";
const GITHUB_UPDATE_MANIFEST_ASSETS: [&str; 3] =
    ["latest.json", "evertydesk-update.json", "update.json"];

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

#[derive(Debug, Clone, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
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
pub fn check_for_update(
    manifest_url: &str,
    current_version: &str,
) -> Result<Option<UpdateManifest>, String> {
    require_https(manifest_url)?;

    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
    let response = agent
        .get(manifest_url)
        .call()
        .map_err(|error| format!("failed to check for updates: {error}"))?;

    let mut limited = response.into_reader().take(MAX_MANIFEST_BYTES);
    let mut body = String::new();
    limited
        .read_to_string(&mut body)
        .map_err(|error| format!("failed to read update server response: {error}"))?;

    let manifest: UpdateManifest =
        serde_json::from_str(&body).map_err(|error| format!("invalid update manifest: {error}"))?;
    validate_manifest_metadata(&manifest)?;

    let Some(artifact) = manifest.artifact_for_current_platform() else {
        return Err("manifest does not contain an artifact for this platform".to_owned());
    };
    validate_artifact(artifact)?;

    if is_newer_version(&manifest.version, current_version) {
        Ok(Some(manifest))
    } else {
        Ok(None)
    }
}

/// Checks the latest GitHub Release for a manifest asset (`latest.json`,
/// `evertydesk-update.json`, or `update.json`) and then runs the same manifest
/// validation/download flow as the self-hosted channel. GitHub hosts the
/// manifest, but the manifest still carries platform artifact SHA-256 hashes.
pub fn check_github_release_for_update(
    owner_repo: &str,
    current_version: &str,
) -> Result<Option<UpdateManifest>, String> {
    let owner_repo = validate_github_owner_repo(owner_repo)?;
    let release_url = format!("{GITHUB_API_BASE}/{owner_repo}/releases/latest");
    let body = http_get_string(&release_url, MAX_MANIFEST_BYTES)?;
    let release: GithubRelease =
        serde_json::from_str(&body).map_err(|error| format!("invalid GitHub release: {error}"))?;

    let manifest_url = github_manifest_asset_url(&release)
        .ok_or_else(|| "GitHub release does not contain latest.json manifest asset".to_owned())?;
    let mut update = check_for_update(&manifest_url, current_version)?;
    if let Some(manifest) = &mut update {
        if manifest.notes.trim().is_empty() {
            manifest.notes = release.body.trim().to_owned();
        }
        if manifest.version.trim().is_empty() {
            manifest.version = normalize_github_tag_version(&release.tag_name);
        }
    }
    Ok(update)
}

/// Rejects anything but `https://`. A plaintext manifest is fully
/// attacker-controlled by any on-path network position -- including the
/// `sha256` field it carries, so verifying the download's hash against a
/// hash the same attacker supplied provides no real integrity guarantee.
/// HTTPS is not optional here; there is no local-testing bypass.
fn require_https(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err("update URL must use https://".to_owned())
    }
}

fn http_get_string(url: &str, max_bytes: u64) -> Result<String, String> {
    require_https(url)?;
    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
    let response = agent
        .get(url)
        .set("User-Agent", "EvertyDesk-DesktopNext-Updater")
        .call()
        .map_err(|error| format!("failed to request {url}: {error}"))?;
    let mut limited = response.into_reader().take(max_bytes);
    let mut body = String::new();
    limited
        .read_to_string(&mut body)
        .map_err(|error| format!("failed to read response from {url}: {error}"))?;
    Ok(body)
}

fn validate_github_owner_repo(owner_repo: &str) -> Result<String, String> {
    let value = owner_repo.trim().trim_matches('/');
    let mut parts = value.split('/');
    let Some(owner) = parts.next() else {
        return Err("GitHub repository must be owner/repo".to_owned());
    };
    let Some(repo) = parts.next() else {
        return Err("GitHub repository must be owner/repo".to_owned());
    };
    if parts.next().is_some()
        || !github_path_component_is_safe(owner)
        || !github_path_component_is_safe(repo)
    {
        return Err("GitHub repository must be a safe owner/repo value".to_owned());
    }
    Ok(format!("{owner}/{repo}"))
}

fn github_path_component_is_safe(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn github_manifest_asset_url(release: &GithubRelease) -> Option<String> {
    release.assets.iter().find_map(|asset| {
        let name = asset.name.trim();
        if GITHUB_UPDATE_MANIFEST_ASSETS
            .iter()
            .any(|expected| name.eq_ignore_ascii_case(expected))
            && asset.browser_download_url.starts_with("https://")
        {
            Some(asset.browser_download_url.clone())
        } else {
            None
        }
    })
}

fn normalize_github_tag_version(tag_name: &str) -> String {
    tag_name.trim().trim_start_matches(['v', 'V']).to_owned()
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
        .ok_or_else(|| "manifest does not contain an artifact for this platform".to_owned())?;
    validate_artifact(artifact)?;

    std::fs::create_dir_all(destination_dir)
        .map_err(|error| format!("failed to create update download directory: {error}"))?;

    let file_name = artifact_file_name(&artifact.url)?;
    let destination = destination_dir.join(file_name);
    let partial_destination = destination.with_extension(format!(
        "{}.part",
        destination
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("download")
    ));

    let agent = ureq::AgentBuilder::new().timeout(DOWNLOAD_TIMEOUT).build();
    let response = agent
        .get(&artifact.url)
        .call()
        .map_err(|error| format!("failed to download update: {error}"))?;
    reject_oversized_content_length(&response)?;

    let _ = std::fs::remove_file(&partial_destination);
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&partial_destination)
        .map_err(|error| format!("failed to create update download file: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total_bytes = 0u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("update download failed: {error}"))?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes
            .checked_add(read as u64)
            .ok_or_else(|| "update download size overflow".to_owned())?;
        if total_bytes > MAX_DOWNLOAD_BYTES {
            drop(file);
            let _ = std::fs::remove_file(&partial_destination);
            return Err(format!(
                "update download exceeds the {} byte safety limit",
                MAX_DOWNLOAD_BYTES
            ));
        }
        hasher.update(&buffer[..read]);
        if let Err(error) = file.write_all(&buffer[..read]) {
            drop(file);
            let _ = std::fs::remove_file(&partial_destination);
            return Err(format!("failed to write update download file: {error}"));
        }
    }
    drop(file);

    let digest = hex_encode(&hasher.finalize());
    if !digest.eq_ignore_ascii_case(artifact.sha256.trim()) {
        let _ = std::fs::remove_file(&partial_destination);
        return Err(format!(
            "update checksum mismatch (expected {}, got {digest}); deleted the downloaded file",
            artifact.sha256
        ));
    }

    if destination.exists() {
        std::fs::remove_file(&destination)
            .map_err(|error| format!("failed to replace existing verified update file: {error}"))?;
    }
    std::fs::rename(&partial_destination, &destination)
        .map_err(|error| format!("failed to finalize verified update download: {error}"))?;

    Ok(destination)
}

/// Launches the verified installer and lets the user complete it -- this
/// process does not attempt to self-replace or elevate silently.
pub fn launch_installer(installer_path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new(installer_path)
            .spawn()
            .map_err(|error| format!("failed to launch installer: {error}"))?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(installer_path)
            .spawn()
            .map_err(|error| format!("failed to open disk image: {error}"))?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = installer_path;
        Err("auto-update is not supported on this platform".to_owned())
    }
}

fn validate_artifact(artifact: &PlatformArtifact) -> Result<(), String> {
    require_https(&artifact.url)?;
    validate_sha256(&artifact.sha256)?;
    let file_name = artifact_file_name(&artifact.url)?;
    if cfg!(target_os = "windows") && !file_name.to_ascii_lowercase().ends_with(".msi") {
        return Err("windows update artifact must be an MSI file".to_owned());
    }
    if cfg!(target_os = "macos") && !file_name.to_ascii_lowercase().ends_with(".dmg") {
        return Err("macOS update artifact must be a DMG file".to_owned());
    }
    Ok(())
}

fn validate_manifest_metadata(manifest: &UpdateManifest) -> Result<(), String> {
    let version = manifest.version.trim();
    if version.is_empty() || version.len() > MAX_VERSION_BYTES {
        return Err(format!(
            "manifest version must be 1..={MAX_VERSION_BYTES} bytes"
        ));
    }
    if version
        .bytes()
        .any(|byte| byte < 0x20 || byte == b'/' || byte == b'\\')
    {
        return Err("manifest version contains unsafe characters".to_owned());
    }
    if manifest.notes.len() > MAX_NOTES_BYTES {
        return Err(format!("manifest notes exceed {MAX_NOTES_BYTES} bytes"));
    }
    Ok(())
}

fn reject_oversized_content_length(response: &ureq::Response) -> Result<(), String> {
    let Some(raw) = response.header("Content-Length") else {
        return Ok(());
    };
    let length = raw
        .trim()
        .parse::<u64>()
        .map_err(|_| "update server returned an invalid Content-Length".to_owned())?;
    if length > MAX_DOWNLOAD_BYTES {
        Err(format!(
            "update server reports a {} byte file, above the {} byte safety limit",
            length, MAX_DOWNLOAD_BYTES
        ))
    } else {
        Ok(())
    }
}

fn validate_sha256(raw: &str) -> Result<(), String> {
    let value = raw.trim();
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("manifest SHA-256 must be exactly 64 hex characters".to_owned())
    }
}

fn artifact_file_name(url: &str) -> Result<&str, String> {
    let path = url
        .split_once(['?', '#'])
        .map(|(path, _)| path)
        .unwrap_or(url);
    let Some(name) = path.rsplit('/').next().filter(|name| !name.is_empty()) else {
        return Err("update URL does not contain a file name".to_owned());
    };
    if name == "." || name == ".." || name.contains('\\') || name.bytes().any(|byte| byte < 0x20) {
        return Err("update URL contains an unsafe file name".to_owned());
    }
    Ok(name)
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

    const VALID_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

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
    fn generated_manifest_allows_extra_human_fields() {
        let manifest: UpdateManifest = serde_json::from_str(&format!(
            r#"{{
                "version": "2.0.0",
                "notes": "release",
                "windows": {{
                    "url": "https://example.com/EvertyDeskNext-2.0.0-x64.msi",
                    "sha256": "{VALID_SHA256}",
                    "size_bytes": 123
                }},
                "windows_portable": {{
                    "url": "https://example.com/EvertyDeskNext-2.0.0-windows-x64-portable.zip",
                    "sha256": "{VALID_SHA256}",
                    "size_bytes": 456
                }}
            }}"#
        ))
        .unwrap();
        assert_eq!(manifest.version, "2.0.0");
        assert_eq!(manifest.notes, "release");
        if cfg!(target_os = "windows") {
            assert!(manifest.artifact_for_current_platform().is_some());
            assert!(validate_artifact(manifest.artifact_for_current_platform().unwrap()).is_ok());
        }
    }

    #[test]
    fn github_release_channel_uses_manifest_asset_not_raw_installer() {
        let release: GithubRelease = serde_json::from_str(
            r#"{
                "tag_name": "v2.0.1",
                "body": "release notes",
                "assets": [
                    {
                        "name": "EvertyDeskNext-2.0.1-x64.msi",
                        "browser_download_url": "https://github.com/everty/desk/releases/download/v2.0.1/EvertyDeskNext-2.0.1-x64.msi"
                    },
                    {
                        "name": "latest.json",
                        "browser_download_url": "https://github.com/everty/desk/releases/download/v2.0.1/latest.json"
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(normalize_github_tag_version(&release.tag_name), "2.0.1");
        assert_eq!(
            github_manifest_asset_url(&release).as_deref(),
            Some("https://github.com/everty/desk/releases/download/v2.0.1/latest.json")
        );
    }

    #[test]
    fn github_owner_repo_validation_rejects_paths_and_urls() {
        assert_eq!(
            validate_github_owner_repo(" EvertyDesk/EvertyDesk_Lite ").as_deref(),
            Ok("EvertyDesk/EvertyDesk_Lite")
        );
        assert!(validate_github_owner_repo("https://github.com/a/b").is_err());
        assert!(validate_github_owner_repo("a/b/c").is_err());
        assert!(validate_github_owner_repo("a/../b").is_err());
        assert!(validate_github_owner_repo("a/").is_err());
    }

    #[test]
    fn artifact_validation_rejects_bad_sha_and_unsafe_names() {
        assert!(validate_sha256(VALID_SHA256).is_ok());
        assert!(validate_sha256("abc").is_err());
        assert_eq!(
            artifact_file_name("https://example.com/releases/EvertyDeskNext-2.0.0-x64.msi?token=1")
                .unwrap(),
            "EvertyDeskNext-2.0.0-x64.msi"
        );
        assert!(artifact_file_name("https://example.com/releases/").is_err());
        assert!(artifact_file_name("https://example.com/releases/..").is_err());
        assert!(artifact_file_name("https://example.com/releases/bad\\name.msi").is_err());
    }

    #[test]
    fn manifest_metadata_rejects_empty_unsafe_and_oversized_fields() {
        let valid = UpdateManifest {
            version: "2.0.0".to_owned(),
            notes: "release".to_owned(),
            windows: None,
            macos: None,
        };
        assert!(validate_manifest_metadata(&valid).is_ok());

        let empty_version = UpdateManifest {
            version: " ".to_owned(),
            ..valid.clone()
        };
        assert!(validate_manifest_metadata(&empty_version).is_err());

        let unsafe_version = UpdateManifest {
            version: "2.0.0/evil".to_owned(),
            ..valid.clone()
        };
        assert!(validate_manifest_metadata(&unsafe_version).is_err());

        let oversized_version = UpdateManifest {
            version: "1".repeat(MAX_VERSION_BYTES + 1),
            ..valid.clone()
        };
        assert!(validate_manifest_metadata(&oversized_version).is_err());

        let oversized_notes = UpdateManifest {
            notes: "n".repeat(MAX_NOTES_BYTES + 1),
            ..valid
        };
        assert!(validate_manifest_metadata(&oversized_notes).is_err());
    }

    #[test]
    fn hex_encode_matches_known_sha256() {
        let digest = Sha256::digest(b"");
        assert_eq!(hex_encode(&digest), VALID_SHA256);
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
