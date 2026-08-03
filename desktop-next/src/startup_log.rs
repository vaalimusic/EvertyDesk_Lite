use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic;
use std::path::PathBuf;
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_FILE_NAME: &str = "desktop-next.log";
const MAX_LOG_BYTES: u64 = 1024 * 1024;

static INSTALL_PANIC_HOOK: Once = Once::new();

pub fn install_process_diagnostics(process_name: &'static str) {
    rotate_log_if_needed();
    append_log_line(process_name, "startup");

    INSTALL_PANIC_HOOK.call_once(move || {
        let previous_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            append_log_line(process_name, &format!("panic: {info}"));
            previous_hook(info);
        }));
    });
}

pub fn append_log_line(process_name: &str, message: &str) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{} [{process_name}] {message}", unix_timestamp_secs());
}

pub fn log_path() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| env::var_os("TMP").map(PathBuf::from))
        .map(|base| base.join("EvertyDesk").join(LOG_FILE_NAME))
}

fn rotate_log_if_needed() {
    let Some(path) = log_path() else {
        return;
    };
    let Ok(metadata) = fs::metadata(&path) else {
        return;
    };
    if metadata.len() <= MAX_LOG_BYTES {
        return;
    }
    let rotated = path.with_extension("log.old");
    let _ = fs::remove_file(&rotated);
    let _ = fs::rename(&path, rotated);
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_uses_a_stable_file_name() {
        let path = log_path().unwrap_or_else(|| PathBuf::from(LOG_FILE_NAME));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(LOG_FILE_NAME)
        );
    }
}
