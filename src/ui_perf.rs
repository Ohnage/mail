//! Lightweight UI-thread timing for tab switches and stall detection.
//!
//! Writes `%APPDATA%/flectar-mail/perf.log` (rotated at 1 MB). Lines never
//! include message bodies or addresses — only tab ids and millisecond timings.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static PERF_LOG: Mutex<Option<PerfLog>> = Mutex::new(None);
const ROTATE_BYTES: u64 = 1024 * 1024;
const STALL_MS: u128 = 100;

struct PerfLog {
    path: PathBuf,
    file: File,
}

fn default_path() -> PathBuf {
    // Match the installed app's data root without pulling in the `dirs` crate.
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    root.join("flectar-mail").join("perf.log")
}

fn with_log(write: impl FnOnce(&mut File)) {
    let mut guard = PERF_LOG.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let path = default_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
            *guard = Some(PerfLog { path, file });
        }
    }
    let Some(log) = guard.as_mut() else {
        return;
    };
    if log.file.metadata().map(|m| m.len()).unwrap_or(0) >= ROTATE_BYTES {
        let rotated = log.path.with_extension("log.1");
        let _ = std::fs::rename(&log.path, &rotated);
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log.path)
        {
            log.file = file;
        }
    }
    write(&mut log.file);
    let _ = log.file.flush();
}

fn stamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn log_tab_switch(
    tab_id: &str,
    ui_callback_ms: u128,
    first_rows_ms: Option<u128>,
    metadata_ms: Option<u128>,
) {
    let tab = tab_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect::<String>();
    with_log(|file| {
        let _ = writeln!(
            file,
            "ts_ms={} tab_switch tab={tab} ui_callback_ms={ui_callback_ms} first_rows_ms={} metadata_ms={}",
            stamp_ms(),
            first_rows_ms
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            metadata_ms
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
        );
    });
    if ui_callback_ms >= STALL_MS {
        log_stall("tab_switch", ui_callback_ms);
    }
}

pub fn log_stall(name: &str, ms: u128) {
    let name = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == ':')
        .take(96)
        .collect::<String>();
    with_log(|file| {
        let _ = writeln!(file, "ts_ms={} stall name={name} ms={ms}", stamp_ms());
    });
}

/// Time a UI-thread section and log when it exceeds 100 ms.
pub fn watch_ui<T>(name: &str, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = work();
    let ms = started.elapsed().as_millis();
    if ms >= STALL_MS {
        log_stall(name, ms);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_threshold_is_one_hundred_ms() {
        const {
            assert!(STALL_MS == 100);
            assert!(STALL_MS < 1_000);
        }
    }
}
