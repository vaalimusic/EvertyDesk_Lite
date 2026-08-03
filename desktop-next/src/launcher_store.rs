use crate::protocol::{ConnectionQuality, ViewerScaling};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RECENT_CONNECTIONS: usize = 12;
const MAX_STORE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LauncherStore {
    #[serde(default)]
    pub contacts: Vec<Contact>,
    #[serde(default)]
    pub recent: Vec<RecentConnection>,
    #[serde(default)]
    pub quality: ConnectionQuality,
    #[serde(default)]
    pub scaling: ViewerScaling,
    #[serde(default = "default_true")]
    pub audio_enabled: bool,
    #[serde(default = "default_true")]
    pub start_host_on_launch: bool,
    /// Non-secret account identifier. The access token is kept in the OS
    /// credential vault and is deliberately never serialized here.
    #[serde(default)]
    pub address_book_account: String,
    #[serde(default)]
    pub address_book_guid: String,
    #[serde(default)]
    pub address_book_last_sync_unix: u64,
    #[serde(default = "default_true")]
    pub smart_agent_enabled: bool,
    #[serde(default)]
    pub smart_agent_service_key: String,
    #[serde(default)]
    pub compatibility_settings_expanded: bool,
    #[serde(default)]
    pub vm_settings_expanded: bool,
    #[serde(default)]
    pub vm_bridge_enabled: bool,
    #[serde(default)]
    pub vm_provider: VmProviderPreference,
    #[serde(default)]
    pub vm_target_id: String,
    #[serde(default)]
    pub game_codec: GameCodecPreference,
    #[serde(default)]
    pub game_evrt2_enabled: bool,
}

impl Default for LauncherStore {
    fn default() -> Self {
        Self {
            contacts: Vec::new(),
            recent: Vec::new(),
            quality: ConnectionQuality::default(),
            scaling: ViewerScaling::default(),
            audio_enabled: true,
            start_host_on_launch: true,
            address_book_account: String::new(),
            address_book_guid: String::new(),
            address_book_last_sync_unix: 0,
            smart_agent_enabled: true,
            smart_agent_service_key: String::new(),
            compatibility_settings_expanded: false,
            vm_settings_expanded: false,
            vm_bridge_enabled: false,
            vm_provider: VmProviderPreference::default(),
            vm_target_id: String::new(),
            game_codec: GameCodecPreference::default(),
            game_evrt2_enabled: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    pub remote_id: String,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentConnection {
    pub remote_id: String,
    pub last_used_unix: u64,
    #[serde(default)]
    pub direction: ConnectionDirection,
    #[serde(default)]
    pub duration_seconds: u64,
    #[serde(default)]
    pub reconnect_count: u32,
    #[serde(default)]
    pub last_end_reason: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionDirection {
    #[default]
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmProviderPreference {
    #[default]
    Auto,
    HyperV,
    VirtualBox,
}

impl VmProviderPreference {
    pub const ALL: [Self; 3] = [Self::Auto, Self::HyperV, Self::VirtualBox];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::HyperV => "Hyper-V",
            Self::VirtualBox => "VirtualBox",
        }
    }

    pub fn prefix(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::HyperV => Some("hyperv"),
            Self::VirtualBox => Some("vbox"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameCodecPreference {
    #[default]
    Auto,
    H265,
    H264,
    Av1,
}

impl GameCodecPreference {
    pub const ALL: [Self; 4] = [Self::Auto, Self::H265, Self::H264, Self::Av1];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::H265 => "H265 / HEVC",
            Self::H264 => "H264",
            Self::Av1 => "AV1",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Auto => {
                "Auto выбирает конкретный аппаратный codec; для Windows безопасный default — H264."
            }
            Self::H265 => {
                "H265 экономит bitrate, но на Windows HEVC decoder может быть нестабилен."
            }
            Self::H264 => "H264 — самый совместимый аппаратный вариант для Game режима.",
            Self::Av1 => "AV1 имеет смысл только на новом железе с аппаратным encode/decode.",
        }
    }
}

impl LauncherStore {
    pub fn load_default() -> io::Result<Self> {
        Self::load_from(&default_store_path()?)
    }

    pub fn save_default(&self) -> io::Result<()> {
        self.save_to(&default_store_path()?)
    }

    pub fn load_from(path: &Path) -> io::Result<Self> {
        match read_store(path) {
            Ok(store) => Ok(store),
            Err(primary_error)
                if matches!(
                    primary_error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                let backup = backup_path(path);
                match read_store(&backup) {
                    Ok(store) => {
                        if primary_error.kind() == io::ErrorKind::InvalidData {
                            let _ = fs::rename(path, corrupt_path(path));
                        }
                        let _ = fs::copy(&backup, path);
                        Ok(store)
                    }
                    Err(backup_error) if backup_error.kind() == io::ErrorKind::NotFound => {
                        if primary_error.kind() == io::ErrorKind::NotFound {
                            Ok(Self::default())
                        } else {
                            Err(primary_error)
                        }
                    }
                    Err(_) => Err(primary_error),
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let encoded = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        if encoded.len() as u64 > MAX_STORE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "launcher store exceeds 1 MiB",
            ));
        }
        let temporary = temporary_path(path);
        let backup = backup_path(path);
        if temporary.exists() {
            fs::remove_file(&temporary)?;
        }
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        drop(file);

        if path.exists() {
            if backup.exists() {
                fs::remove_file(&backup)?;
            }
            fs::rename(path, &backup)?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            if backup.exists() && !path.exists() {
                let _ = fs::rename(&backup, path);
            }
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    pub fn upsert_contact(&mut self, name: &str, remote_id: &str) -> Result<(), StoreError> {
        self.upsert_contact_details(name, remote_id, "", "")
    }

    pub fn upsert_contact_details(
        &mut self,
        name: &str,
        remote_id: &str,
        group: &str,
        note: &str,
    ) -> Result<(), StoreError> {
        self.upsert_contact_details_with_tags(name, remote_id, group, note, &[])
    }

    pub fn upsert_contact_details_with_tags(
        &mut self,
        name: &str,
        remote_id: &str,
        group: &str,
        note: &str,
        tags: &[String],
    ) -> Result<(), StoreError> {
        let name = name.trim();
        let remote_id = normalize_contact_id(remote_id);
        let group = group.trim();
        let note = note.trim();
        let tags = normalize_contact_tags(tags);
        validate_contact(name, &remote_id, group, note)?;
        validate_contact_tags(&tags)?;

        if let Some(contact) = self
            .contacts
            .iter_mut()
            .find(|contact| normalize_contact_id(&contact.remote_id) == remote_id)
        {
            contact.name = name.to_owned();
            contact.remote_id = remote_id.clone();
            contact.group = group.to_owned();
            contact.tags = tags;
            contact.note = note.to_owned();
        } else {
            self.contacts.push(Contact {
                name: name.to_owned(),
                remote_id: remote_id.clone(),
                favorite: false,
                group: group.to_owned(),
                tags,
                note: note.to_owned(),
            });
        }
        self.contacts.sort_by_key(|contact| {
            (
                contact.group.to_lowercase(),
                contact.name.to_lowercase(),
                contact.remote_id.clone(),
            )
        });
        Ok(())
    }

    pub fn remove_contact(&mut self, remote_id: &str) -> bool {
        let remote_id = normalize_contact_id(remote_id);
        let previous_len = self.contacts.len();
        self.contacts
            .retain(|contact| normalize_contact_id(&contact.remote_id) != remote_id);
        self.contacts.len() != previous_len
    }

    pub fn toggle_favorite(&mut self, remote_id: &str) -> Option<bool> {
        let remote_id = normalize_contact_id(remote_id);
        let contact = self
            .contacts
            .iter_mut()
            .find(|contact| normalize_contact_id(&contact.remote_id) == remote_id)?;
        contact.favorite = !contact.favorite;
        Some(contact.favorite)
    }

    /// Merges the cloud address book into the local offline cache. Local-only
    /// contacts are retained and local grouping/favorite choices survive sync.
    pub fn merge_cloud_contacts<I>(&mut self, contacts: I) -> usize
    where
        I: IntoIterator<Item = (String, String, String, Vec<String>)>,
    {
        let mut added = 0;
        for (name, remote_id, note, tags) in contacts {
            let remote_id = normalize_contact_id(&remote_id);
            if remote_id.is_empty() {
                continue;
            }
            let tags = normalize_contact_tags(&tags);
            if let Some(contact) = self
                .contacts
                .iter_mut()
                .find(|contact| normalize_contact_id(&contact.remote_id) == remote_id)
            {
                if !name.trim().is_empty() {
                    contact.name = name.trim().to_owned();
                }
                if !note.trim().is_empty() {
                    contact.note = note.trim().to_owned();
                }
                if !tags.is_empty() {
                    contact.tags = tags;
                }
                contact.remote_id = remote_id;
            } else {
                let fallback_name = if name.trim().is_empty() {
                    remote_id.as_str()
                } else {
                    name.trim()
                };
                if self
                    .upsert_contact_details_with_tags(
                        fallback_name,
                        &remote_id,
                        "",
                        note.trim(),
                        &tags,
                    )
                    .is_ok()
                {
                    added += 1;
                }
            }
        }
        self.contacts.sort_by_key(|contact| {
            (
                contact.group.to_lowercase(),
                contact.name.to_lowercase(),
                contact.remote_id.clone(),
            )
        });
        self.address_book_last_sync_unix = unix_now();
        added
    }

    pub fn record_recent(&mut self, remote_id: &str) {
        self.record_recent_with_direction(remote_id, ConnectionDirection::Outgoing);
    }

    pub fn record_incoming(&mut self, remote_id: &str) {
        self.record_recent_with_direction(remote_id, ConnectionDirection::Incoming);
    }

    fn record_recent_with_direction(&mut self, remote_id: &str, direction: ConnectionDirection) {
        let remote_id = normalize_contact_id(remote_id);
        if remote_id.is_empty() {
            return;
        }
        self.recent
            .retain(|connection| normalize_contact_id(&connection.remote_id) != remote_id);
        self.recent.insert(
            0,
            RecentConnection {
                remote_id,
                last_used_unix: unix_now(),
                direction,
                duration_seconds: 0,
                reconnect_count: 0,
                last_end_reason: String::new(),
            },
        );
        self.recent.truncate(MAX_RECENT_CONNECTIONS);
    }

    pub fn update_recent_summary(
        &mut self,
        remote_id: &str,
        duration_seconds: u64,
        reconnect_count: u32,
        end_reason: &str,
    ) {
        let remote_id = normalize_contact_id(remote_id);
        if let Some(connection) = self
            .recent
            .iter_mut()
            .find(|connection| normalize_contact_id(&connection.remote_id) == remote_id)
        {
            connection.duration_seconds = duration_seconds;
            connection.reconnect_count = reconnect_count;
            connection.last_end_reason = sanitize_end_reason(end_reason);
        }
    }

    pub fn finish_incoming(&mut self, remote_id: &str, duration_seconds: u64, reason: &str) {
        let remote_id = normalize_contact_id(remote_id);
        if let Some(connection) = self
            .recent
            .iter_mut()
            .find(|connection| normalize_contact_id(&connection.remote_id) == remote_id)
        {
            connection.direction = ConnectionDirection::Incoming;
            connection.duration_seconds = duration_seconds;
            connection.reconnect_count = 0;
            connection.last_end_reason = sanitize_end_reason(reason);
        }
    }

    pub fn remove_recent(&mut self, remote_id: &str) -> bool {
        let remote_id = normalize_contact_id(remote_id);
        let previous_len = self.recent.len();
        self.recent
            .retain(|connection| normalize_contact_id(&connection.remote_id) != remote_id);
        self.recent.len() != previous_len
    }

    pub fn clear_recent(&mut self) -> bool {
        if self.recent.is_empty() {
            return false;
        }
        self.recent.clear();
        true
    }
}

fn sanitize_end_reason(reason: &str) -> String {
    reason
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(120)
        .collect()
}

fn normalize_contact_id(id: &str) -> String {
    id.chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .collect::<String>()
        .to_lowercase()
}

fn read_store(path: &Path) -> io::Result<LauncherStore> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_STORE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("launcher store {} exceeds 1 MiB", path.display()),
        ));
    }
    let encoded = fs::read(path)?;
    if encoded.len() as u64 > MAX_STORE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("launcher store {} exceeds 1 MiB", path.display()),
        ));
    }
    serde_json::from_slice(&encoded).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid launcher store {}: {error}", path.display()),
        )
    })
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

fn corrupt_path(path: &Path) -> PathBuf {
    path.with_extension(format!("corrupt-{}.json", unix_now()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    MissingName,
    MissingRemoteId,
    NameTooLong,
    RemoteIdTooLong,
    InvalidRemoteId,
    GroupTooLong,
    NoteTooLong,
    InvalidMetadata,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingName => formatter.write_str("укажите имя компьютера"),
            Self::MissingRemoteId => formatter.write_str("укажите ID компьютера"),
            Self::NameTooLong => formatter.write_str("имя компьютера слишком длинное"),
            Self::RemoteIdTooLong => formatter.write_str("ID компьютера слишком длинный"),
            Self::InvalidRemoteId => formatter.write_str("ID содержит управляющие символы"),
            Self::GroupTooLong => formatter.write_str("название группы слишком длинное"),
            Self::NoteTooLong => formatter.write_str("заметка слишком длинная"),
            Self::InvalidMetadata => {
                formatter.write_str("группа или заметка содержит управляющие символы")
            }
        }
    }
}

impl std::error::Error for StoreError {}

fn validate_contact(
    name: &str,
    remote_id: &str,
    group: &str,
    note: &str,
) -> Result<(), StoreError> {
    if name.is_empty() {
        return Err(StoreError::MissingName);
    }
    if remote_id.is_empty() {
        return Err(StoreError::MissingRemoteId);
    }
    if name.chars().count() > 80 {
        return Err(StoreError::NameTooLong);
    }
    if remote_id.len() > 128 {
        return Err(StoreError::RemoteIdTooLong);
    }
    if remote_id.chars().any(char::is_control) {
        return Err(StoreError::InvalidRemoteId);
    }
    if group.chars().count() > 60 {
        return Err(StoreError::GroupTooLong);
    }
    if note.chars().count() > 300 {
        return Err(StoreError::NoteTooLong);
    }
    if group.chars().chain(note.chars()).any(char::is_control) {
        return Err(StoreError::InvalidMetadata);
    }
    Ok(())
}

pub fn normalize_contact_tags(tags: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        let tag = tag.split_whitespace().collect::<Vec<_>>().join(" ");
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&tag))
        {
            normalized.push(tag);
        }
    }
    normalized.sort_by_key(|tag| tag.to_lowercase());
    normalized
}

fn validate_contact_tags(tags: &[String]) -> Result<(), StoreError> {
    if tags.len() > 24 {
        return Err(StoreError::InvalidMetadata);
    }
    for tag in tags {
        if tag.chars().count() > 40 || tag.chars().any(char::is_control) {
            return Err(StoreError::InvalidMetadata);
        }
    }
    Ok(())
}

fn default_store_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    if let Some(app_data) = std::env::var_os("APPDATA") {
        return Ok(PathBuf::from(app_data)
            .join("EvertyDesk")
            .join("launcher.json"));
    }

    #[cfg(not(windows))]
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config_home)
            .join("evertydesk")
            .join("launcher.json"));
    }

    if let Some(user_profile) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
    {
        return Ok(PathBuf::from(user_profile)
            .join(".config")
            .join("evertydesk")
            .join("launcher.json"));
    }

    Ok(std::env::current_dir()?.join("evertydesk-launcher.json"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "evertydesk-launcher-store-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn store_path(&self) -> PathBuf {
            self.0.join("launcher.json")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn contact_upsert_is_sorted_and_deduplicated_by_id() {
        let mut store = LauncherStore::default();
        store.upsert_contact("Workstation", "222").unwrap();
        store.upsert_contact("Alpha", "111").unwrap();
        store.upsert_contact("Renamed", "222").unwrap();

        assert_eq!(store.contacts.len(), 2);
        assert_eq!(store.contacts[0].name, "Alpha");
        assert_eq!(store.contacts[1].name, "Renamed");
    }

    #[test]
    fn contact_ids_are_normalized_before_deduplication() {
        let mut store = LauncherStore::default();
        store.upsert_contact("Cashbox", "123 456-789").unwrap();
        store.toggle_favorite("123456789");
        store
            .upsert_contact_details("Cashbox renamed", "123-456 789", "Office", "")
            .unwrap();

        assert_eq!(store.contacts.len(), 1);
        assert_eq!(store.contacts[0].remote_id, "123456789");
        assert_eq!(store.contacts[0].name, "Cashbox renamed");
        assert!(store.contacts[0].favorite);
        assert_eq!(store.contacts[0].group, "Office");
    }

    #[test]
    fn contact_actions_match_normalized_ids() {
        let mut store = LauncherStore::default();
        store.upsert_contact("Cashbox", "123 456").unwrap();

        assert_eq!(store.toggle_favorite("123-456"), Some(true));
        assert!(store.contacts[0].favorite);
        assert!(store.remove_contact("123 456"));
        assert!(store.contacts.is_empty());
    }

    #[test]
    fn recent_connections_are_unique_and_bounded() {
        let mut store = LauncherStore::default();
        for index in 0..20 {
            store.record_recent(&index.to_string());
        }
        store.record_recent("15");

        assert_eq!(store.recent.len(), MAX_RECENT_CONNECTIONS);
        assert_eq!(store.recent[0].remote_id, "15");
        assert_eq!(
            store
                .recent
                .iter()
                .filter(|connection| connection.remote_id == "15")
                .count(),
            1
        );
    }

    #[test]
    fn recent_connections_are_normalized_before_deduplication() {
        let mut store = LauncherStore::default();
        store.record_recent("123 456");
        store.record_recent("123-456");

        assert_eq!(store.recent.len(), 1);
        assert_eq!(store.recent[0].remote_id, "123456");
    }

    #[test]
    fn recent_actions_match_normalized_ids() {
        let mut store = LauncherStore::default();
        store.record_recent("123 456");
        store.update_recent_summary("123-456", 42, 2, "done");
        store.finish_incoming("123 456", 43, "incoming");

        assert_eq!(store.recent[0].duration_seconds, 43);
        assert_eq!(store.recent[0].reconnect_count, 0);
        assert_eq!(store.recent[0].direction, ConnectionDirection::Incoming);
        assert_eq!(store.recent[0].last_end_reason, "incoming");
        assert!(store.remove_recent("123-456"));
        assert!(store.recent.is_empty());
    }

    #[test]
    fn store_json_round_trip_preserves_entries() {
        let mut store = LauncherStore::default();
        store.upsert_contact("Office", "123 456 789").unwrap();
        store.toggle_favorite("123 456 789");
        store.record_recent("123 456 789");
        store.start_host_on_launch = true;

        let encoded = serde_json::to_vec(&store).unwrap();
        let decoded: LauncherStore = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, store);
    }

    #[test]
    fn favorite_can_be_toggled_and_old_contacts_default_to_regular() {
        let mut store: LauncherStore = serde_json::from_str(
            r#"{"contacts":[{"name":"Office","remote_id":"123"}],"recent":[]}"#,
        )
        .unwrap();

        assert!(!store.contacts[0].favorite);
        assert!(store.contacts[0].group.is_empty());
        assert!(store.contacts[0].tags.is_empty());
        assert!(store.contacts[0].note.is_empty());
        assert_eq!(store.toggle_favorite("123"), Some(true));
        assert!(store.contacts[0].favorite);
        assert_eq!(store.toggle_favorite("missing"), None);
    }

    #[test]
    fn contact_details_are_updated_and_sorted_by_group_then_name() {
        let mut store = LauncherStore::default();
        store
            .upsert_contact_details("Server", "222", "Production", "Rack 4")
            .unwrap();
        store
            .upsert_contact_details("Laptop", "111", "Personal", "Travel")
            .unwrap();
        store
            .upsert_contact_details("Server renamed", "222", "Office", "Rack 5")
            .unwrap();

        assert_eq!(store.contacts.len(), 2);
        assert_eq!(store.contacts[0].group, "Office");
        assert_eq!(store.contacts[0].name, "Server renamed");
        assert_eq!(store.contacts[0].note, "Rack 5");
        assert_eq!(store.contacts[1].group, "Personal");
    }

    #[test]
    fn contact_tags_are_normalized_deduplicated_and_validated() {
        let mut store = LauncherStore::default();
        store
            .upsert_contact_details_with_tags(
                "PC",
                "123",
                "Office/Cashbox",
                "",
                &[" prod ".to_owned(), "VIP".to_owned(), "prod".to_owned()],
            )
            .unwrap();

        assert_eq!(
            store.contacts[0].tags,
            vec!["prod".to_owned(), "VIP".to_owned()]
        );
        assert_eq!(
            store.upsert_contact_details_with_tags("PC", "123", "", "", &["x".repeat(41)]),
            Err(StoreError::InvalidMetadata)
        );
    }

    #[test]
    fn contact_metadata_is_bounded_and_rejects_control_characters() {
        let mut store = LauncherStore::default();
        assert_eq!(
            store.upsert_contact_details("PC", "123", &"g".repeat(61), ""),
            Err(StoreError::GroupTooLong)
        );
        assert_eq!(
            store.upsert_contact_details("PC", "123", "", &"n".repeat(301)),
            Err(StoreError::NoteTooLong)
        );
        assert_eq!(
            store.upsert_contact_details("PC", "123", "Off\nice", ""),
            Err(StoreError::InvalidMetadata)
        );
    }

    #[test]
    fn recent_entries_can_be_removed_individually_or_cleared() {
        let mut store = LauncherStore::default();
        store.record_recent("111");
        store.record_recent("222");

        assert!(store.remove_recent("111"));
        assert!(!store.remove_recent("missing"));
        assert_eq!(store.recent.len(), 1);
        assert!(store.clear_recent());
        assert!(!store.clear_recent());
    }

    #[test]
    fn session_summary_updates_recent_entry_and_old_json_stays_compatible() {
        let mut store: LauncherStore = serde_json::from_str(
            r#"{"contacts":[],"recent":[{"remote_id":"123","last_used_unix":1}]}"#,
        )
        .unwrap();
        assert_eq!(store.recent[0].duration_seconds, 0);
        assert_eq!(store.recent[0].reconnect_count, 0);
        assert_eq!(store.recent[0].direction, ConnectionDirection::Outgoing);
        assert!(store.recent[0].last_end_reason.is_empty());

        store.update_recent_summary("123", 125, 2, "Отключено пользователем");
        assert_eq!(store.recent[0].duration_seconds, 125);
        assert_eq!(store.recent[0].reconnect_count, 2);
        assert_eq!(store.recent[0].last_end_reason, "Отключено пользователем");
    }

    #[test]
    fn incoming_session_records_direction_duration_and_bounded_reason() {
        let mut store = LauncherStore::default();
        store.record_incoming(" 456 ");
        store.finish_incoming("456", 75, &format!("  disconnected\n{}", "x".repeat(200)));

        let connection = &store.recent[0];
        assert_eq!(connection.remote_id, "456");
        assert_eq!(connection.direction, ConnectionDirection::Incoming);
        assert_eq!(connection.duration_seconds, 75);
        assert!(!connection.last_end_reason.contains('\n'));
        assert!(connection.last_end_reason.chars().count() <= 120);
    }

    #[test]
    fn old_store_without_quality_uses_balanced_profile() {
        let decoded: LauncherStore =
            serde_json::from_str(r#"{"contacts":[],"recent":[]}"#).unwrap();
        assert_eq!(decoded.quality, ConnectionQuality::Balanced);
        assert_eq!(decoded.scaling, ViewerScaling::SmoothFit);
        assert!(decoded.audio_enabled);
        assert!(decoded.start_host_on_launch);
        assert!(decoded.address_book_account.is_empty());
        assert!(decoded.address_book_guid.is_empty());
        assert_eq!(decoded.address_book_last_sync_unix, 0);
        assert!(!decoded.vm_settings_expanded);
        assert!(!decoded.vm_bridge_enabled);
        assert_eq!(decoded.vm_provider, VmProviderPreference::Auto);
        assert!(decoded.vm_target_id.is_empty());
        assert_eq!(decoded.game_codec, GameCodecPreference::Auto);
        assert!(!decoded.game_evrt2_enabled);
    }

    #[test]
    fn vm_settings_round_trip_and_provider_labels_are_stable() {
        let store = LauncherStore {
            vm_settings_expanded: true,
            vm_bridge_enabled: true,
            vm_provider: VmProviderPreference::VirtualBox,
            vm_target_id: "vbox:demo".to_owned(),
            ..LauncherStore::default()
        };

        let decoded: LauncherStore =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        assert_eq!(decoded.vm_provider.label(), "VirtualBox");
        assert_eq!(decoded.vm_provider.prefix(), Some("vbox"));
        assert_eq!(decoded, store);
    }

    #[test]
    fn game_settings_round_trip_and_labels_are_stable() {
        let store = LauncherStore {
            game_codec: GameCodecPreference::Av1,
            game_evrt2_enabled: true,
            ..LauncherStore::default()
        };

        let decoded: LauncherStore =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        assert_eq!(decoded.game_codec.label(), "AV1");
        assert!(decoded.game_codec.hint().contains("AV1"));
        assert!(decoded.game_evrt2_enabled);
        assert_eq!(decoded, store);
    }

    #[test]
    fn cloud_merge_preserves_local_group_and_favorite() {
        let mut store = LauncherStore::default();
        store
            .upsert_contact_details("Old name", "123-456", "Офис", "Old note")
            .unwrap();
        store.toggle_favorite("123-456");

        let added = store.merge_cloud_contacts([
            (
                "Cloud name".to_owned(),
                "123 456".to_owned(),
                "Cloud host".to_owned(),
                vec!["prod".to_owned()],
            ),
            ("New PC".to_owned(), "789".to_owned(), String::new(), vec![]),
        ]);

        assert_eq!(added, 1);
        assert_eq!(store.contacts.len(), 2);
        let existing = store
            .contacts
            .iter()
            .find(|contact| contact.remote_id == "123456")
            .unwrap();
        assert_eq!(existing.name, "Cloud name");
        assert_eq!(existing.group, "Офис");
        assert_eq!(existing.tags, vec!["prod".to_owned()]);
        assert_eq!(existing.note, "Cloud host");
        assert!(existing.favorite);
        assert!(store.address_book_last_sync_unix > 0);
    }

    #[test]
    fn serialized_account_metadata_contains_no_secret_fields() {
        let store = LauncherStore {
            address_book_account: "user@example.com".to_owned(),
            address_book_guid: "book-guid".to_owned(),
            ..LauncherStore::default()
        };
        let json = serde_json::to_string(&store).unwrap();
        assert!(json.contains("user@example.com"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("password"));
    }

    #[test]
    fn transactional_save_keeps_the_previous_store_as_backup() {
        let directory = TestDirectory::new("backup");
        let path = directory.store_path();
        let mut first = LauncherStore {
            start_host_on_launch: true,
            ..LauncherStore::default()
        };
        first.upsert_contact("Office", "111").unwrap();
        first.save_to(&path).unwrap();

        let mut second = first.clone();
        second.upsert_contact("Home", "222").unwrap();
        second.save_to(&path).unwrap();

        assert_eq!(LauncherStore::load_from(&path).unwrap(), second);
        assert_eq!(read_store(&backup_path(&path)).unwrap(), first);
        assert!(!temporary_path(&path).exists());
    }

    #[test]
    fn corrupt_primary_store_is_quarantined_and_restored_from_backup() {
        let directory = TestDirectory::new("recovery");
        let path = directory.store_path();
        let mut recoverable = LauncherStore::default();
        recoverable.upsert_contact("Safe copy", "111").unwrap();
        recoverable.save_to(&path).unwrap();

        let mut latest = recoverable.clone();
        latest.upsert_contact("Latest", "222").unwrap();
        latest.save_to(&path).unwrap();
        fs::write(&path, b"{broken json").unwrap();

        assert_eq!(LauncherStore::load_from(&path).unwrap(), recoverable);
        assert_eq!(read_store(&path).unwrap(), recoverable);
        assert!(fs::read_dir(&directory.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("launcher.corrupt-")
        }));
    }

    #[test]
    fn oversized_store_is_rejected_before_deserialization() {
        let directory = TestDirectory::new("oversized");
        let path = directory.store_path();
        fs::write(&path, vec![b' '; MAX_STORE_BYTES as usize + 1]).unwrap();

        let error = LauncherStore::load_from(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds 1 MiB"));
    }
}
