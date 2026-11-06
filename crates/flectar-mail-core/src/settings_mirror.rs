//! Cross-computer copy of settings that are not mail.
//!
//! The database in the application data directory is the copy this computer
//! reads. `mirror/settings.json` beside that database is the same preferences
//! with OAuth client secrets removed, so it can be copied to another computer.
//! On startup a newer file replaces the local preferences and keeps secrets
//! already stored here. Mail is not in the file; each computer downloads it.

use crate::models::Settings;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    static PENDING: RefCell<Option<(PathBuf, String)>> = const { RefCell::new(None) };
}

pub(crate) fn stage(db_file: PathBuf, stored_json: String) {
    PENDING.with(|pending| {
        *pending.borrow_mut() = Some((db_file, stored_json));
    });
}

pub(crate) fn clear_pending() {
    PENDING.with(|pending| {
        pending.borrow_mut().take();
    });
}

/// Write the staged settings once their transaction has committed.
pub(crate) fn flush_pending() {
    let Some((db_file, json)) = PENDING.with(|pending| pending.borrow_mut().take()) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return;
    };
    if let Err(error) = write_document(&db_file, &value) {
        tracing::warn!(%error, "could not write the settings mirror");
    }
}

pub(crate) fn write_document(db_file: &Path, stored: &serde_json::Value) -> std::io::Result<()> {
    let mut value = stored.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "googleClientSecret".into(),
            serde_json::Value::String(String::new()),
        );
        object.insert(
            "msClientSecret".into(),
            serde_json::Value::String(String::new()),
        );
    }
    let path = mirror_path(db_file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(&value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    // Windows does not replace an existing file with rename.
    if std::fs::rename(&tmp, &path).is_err() {
        std::fs::remove_file(&path)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

pub(crate) fn mirror_path(db_file: &Path) -> PathBuf {
    db_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("mirror")
        .join("settings.json")
}

/// A mirror file newer than `local` wins. Blank secrets in the file keep the
/// secrets already stored on this computer.
pub(crate) fn merged_if_newer(db_file: &Path, local: &Settings) -> Option<Settings> {
    let text = std::fs::read_to_string(mirror_path(db_file)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut incoming: Settings = serde_json::from_value(value).ok()?;
    if incoming.mirror_updated_at <= local.mirror_updated_at {
        return None;
    }
    if incoming.google_client_secret.is_empty() {
        incoming.google_client_secret = local.google_client_secret.clone();
    }
    if incoming.ms_client_secret.is_empty() {
        incoming.ms_client_secret = local.ms_client_secret.clone();
    }
    Some(incoming)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_file_omits_client_secrets_and_a_newer_file_keeps_local_ones() {
        let dir = std::env::temp_dir().join(format!(
            "flectar-settings-mirror-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_file = dir.join("flectar-mail.db");
        let mut stored = serde_json::to_value(Settings {
            theme: "carbon".into(),
            mirror_updated_at: 20,
            google_client_secret: "google-secret".into(),
            ms_client_secret: "ms-secret".into(),
            ..Settings::default()
        })
        .unwrap();
        write_document(&db_file, &stored).unwrap();
        let text = std::fs::read_to_string(mirror_path(&db_file)).unwrap();
        assert!(!text.contains("google-secret"));
        assert!(!text.contains("ms-secret"));
        assert!(text.contains("carbon"));

        let local = Settings {
            mirror_updated_at: 10,
            google_client_secret: "kept-google".into(),
            ms_client_secret: "kept-ms".into(),
            ..Settings::default()
        };
        let merged = merged_if_newer(&db_file, &local).unwrap();
        assert_eq!(merged.theme, "carbon");
        assert_eq!(merged.google_client_secret, "kept-google");
        assert_eq!(merged.ms_client_secret, "kept-ms");
        assert_eq!(merged.mirror_updated_at, 20);

        stored["mirrorUpdatedAt"] = serde_json::json!(5);
        write_document(&db_file, &stored).unwrap();
        let local = Settings {
            mirror_updated_at: 20,
            ..local
        };
        assert!(merged_if_newer(&db_file, &local).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
