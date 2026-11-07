//! Cross-computer copy of settings that are not mail.
//!
//! The database in the application data directory is the copy this computer
//! reads. `mirror/settings.json` beside that database is the same preferences
//! with OAuth client secrets removed, so it can be copied to another computer.
//! On startup a newer file overlays the keys it contains. An empty company-tab
//! list does not replace a saved customized list, and blank secrets stay local.
//! Mail is not in the file; each computer downloads it.

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
    if path.is_file() {
        std::fs::copy(&path, path.with_file_name("settings.mirror.previous.json"))?;
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

/// A newer mirror file overlays only the keys it contains. An empty company
/// tab list does not replace a saved customized list unless that file itself
/// says the empty list is the customization. Blank secrets and an empty
/// receiving catalog keep the values already stored on this computer.
pub(crate) fn merged_if_newer(db_file: &Path, local: &Settings) -> Option<Settings> {
    let text = std::fs::read_to_string(mirror_path(db_file)).ok()?;
    // An older file is the common case. Skip deserializing the receiving
    // catalog until the stamp says this file can change local settings.
    if json_stamp(&text).unwrap_or(0) <= local.mirror_updated_at {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    overlay_newer(local, &value)
}

/// When the database copy lost a customized tab list and the mirror file still
/// has one, copy that list back. An older file is enough: the database stamp
/// can be newer after a write that dropped the tabs and then failed to replace
/// the file.
pub(crate) fn restore_dropped_customizations(db_file: &Path, local: &Settings) -> Option<Settings> {
    if local.business_tabs_customized && !local.business_tabs.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(mirror_path(db_file)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let saved: Settings = serde_json::from_value(value).ok()?;
    if !saved.business_tabs_customized || saved.business_tabs.is_empty() {
        return None;
    }
    let mut restored = local.clone();
    restored.business_tabs = saved.business_tabs;
    restored.business_tabs_customized = true;
    restored.active_business_tab = saved.active_business_tab;
    restored.business_tab_accounts = saved.business_tab_accounts;
    restored.business_tab_icons_initialized = saved.business_tab_icons_initialized;
    if restored.mail_profiles.is_empty() {
        restored.mail_profiles = saved.mail_profiles;
        restored.active_mail_profile = saved.active_mail_profile;
    }
    if restored.inbox_accounts.is_empty() {
        restored.inbox_accounts = saved.inbox_accounts;
        restored.inbox_accounts_initialized = saved.inbox_accounts_initialized;
    }
    Some(restored)
}

fn json_stamp(text: &str) -> Option<i64> {
    let key = "\"mirrorUpdatedAt\"";
    let rest = text.get(text.find(key)? + key.len()..)?.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let end = rest
        .find(|character: char| !character.is_ascii_digit() && character != '-')
        .unwrap_or(rest.len());
    rest.get(..end)?.parse().ok()
}

fn overlay_newer(local: &Settings, incoming: &serde_json::Value) -> Option<Settings> {
    let object = incoming.as_object()?;
    let stamp = object
        .get("mirrorUpdatedAt")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if stamp <= local.mirror_updated_at {
        return None;
    }
    let mut merged = serde_json::to_value(local).ok()?;
    let merged_object = merged.as_object_mut()?;
    let keep_tabs = keep_local_tabs(local, object);
    for (key, value) in object {
        if skip_overlay_key(local, key, value, keep_tabs) {
            continue;
        }
        merged_object.insert(key.clone(), value.clone());
    }
    serde_json::from_value(merged).ok()
}

fn keep_local_tabs(
    local: &Settings,
    incoming: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let explicit_empty = incoming
        .get("businessTabsCustomized")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let incoming_tabs = incoming
        .get("businessTabs")
        .and_then(|value| value.as_array());
    local.business_tabs_customized
        && !local.business_tabs.is_empty()
        && !explicit_empty
        && incoming_tabs.is_none_or(|tabs| tabs.is_empty())
}

fn skip_overlay_key(
    local: &Settings,
    key: &str,
    value: &serde_json::Value,
    keep_tabs: bool,
) -> bool {
    if keep_tabs
        && matches!(
            key,
            "businessTabs"
                | "businessTabsCustomized"
                | "activeBusinessTab"
                | "businessTabAccounts"
                | "businessTabIconsInitialized"
        )
    {
        return true;
    }
    if matches!(key, "googleClientSecret" | "msClientSecret")
        && value.as_str().is_none_or(str::is_empty)
    {
        return true;
    }
    let empty_list = value.as_array().is_some_and(Vec::is_empty);
    empty_list
        && match key {
            "mailProfiles" => !local.mail_profiles.is_empty(),
            "inboxAccounts" => !local.inbox_accounts.is_empty(),
            "addressViews" => !local.address_views.is_empty(),
            "addressRules" => !local.address_rules.is_empty(),
            _ => false,
        }
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

    fn company_tabs() -> Vec<crate::models::BusinessTab> {
        vec![
            serde_json::from_value(serde_json::json!({"id": "tab-5", "name": "TM"})).unwrap(),
            serde_json::from_value(serde_json::json!({"id": "media", "name": "Mentink Media"}))
                .unwrap(),
        ]
    }

    fn customized(stamp: i64) -> Settings {
        Settings {
            mirror_updated_at: stamp,
            business_tabs_customized: true,
            active_business_tab: Some("tab-5".into()),
            business_tabs: company_tabs(),
            mail_profiles: vec![crate::models::MailProfile {
                id: "media".into(),
                name: "Mentink Media".into(),
                color: "#111111".into(),
                account_ids: Vec::new(),
                logo: None,
            }],
            address_views: vec![crate::models::AddressView {
                id: 1,
                address: "media@mentinkmedia.com".into(),
                source_account_emails: Vec::new(),
                profile_id: Some("media".into()),
                source: crate::models::AddressViewSource::Registry,
            }],
            ..Settings::default()
        }
    }

    #[test]
    fn a_newer_empty_mirror_keeps_customized_tabs_profiles_and_the_catalog() {
        let dir = std::env::temp_dir().join(format!(
            "flectar-settings-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_file = dir.join("flectar-mail.db");
        let local = customized(10);
        let incoming = Settings {
            theme: "carbon".into(),
            mirror_updated_at: 20,
            ..Settings::default()
        };
        write_document(&db_file, &serde_json::to_value(&incoming).unwrap()).unwrap();
        let merged = merged_if_newer(&db_file, &local).unwrap();
        assert_eq!(merged.theme, "carbon");
        assert_eq!(merged.business_tabs, local.business_tabs);
        assert!(merged.business_tabs_customized);
        assert_eq!(merged.active_business_tab.as_deref(), Some("tab-5"));
        assert_eq!(merged.mail_profiles, local.mail_profiles);
        assert_eq!(merged.address_views, local.address_views);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_explicit_tab_list_replaces_the_saved_one() {
        let dir = std::env::temp_dir().join(format!(
            "flectar-settings-explicit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_file = dir.join("flectar-mail.db");
        let local = customized(10);
        let mut replacement = customized(30);
        replacement.business_tabs = vec![
            serde_json::from_value(serde_json::json!({"id": "only", "name": "Project 47"}))
                .unwrap(),
        ];
        replacement.active_business_tab = Some("only".into());
        write_document(&db_file, &serde_json::to_value(&replacement).unwrap()).unwrap();
        let merged = merged_if_newer(&db_file, &local).unwrap();
        assert_eq!(merged.business_tabs.len(), 1);
        assert_eq!(merged.business_tabs[0].name, "Project 47");

        let mut cleared = customized(40);
        cleared.business_tabs.clear();
        cleared.active_business_tab = None;
        cleared.address_views.clear();
        write_document(&db_file, &serde_json::to_value(&cleared).unwrap()).unwrap();
        let merged = merged_if_newer(&db_file, &local).unwrap();
        assert!(merged.business_tabs.is_empty());
        assert!(merged.business_tabs_customized);
        assert_eq!(merged.address_views, local.address_views);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_json_and_an_older_mirror_leave_local_settings_unchanged() {
        let dir = std::env::temp_dir().join(format!(
            "flectar-settings-invalid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("mirror")).unwrap();
        let db_file = dir.join("flectar-mail.db");
        let local = customized(10);
        std::fs::write(
            mirror_path(&db_file),
            "{ \"mirrorUpdatedAt\": 99, \"businessTabs\": ",
        )
        .unwrap();
        assert!(merged_if_newer(&db_file, &local).is_none());
        assert!(restore_dropped_customizations(&db_file, &local).is_none());
        assert_eq!(local.business_tabs[0].name, "TM");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_older_mirror_restores_tabs_the_database_dropped() {
        let dir = std::env::temp_dir().join(format!(
            "flectar-settings-restore-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_file = dir.join("flectar-mail.db");
        let saved = customized(10);
        write_document(&db_file, &serde_json::to_value(&saved).unwrap()).unwrap();
        let local = Settings {
            mirror_updated_at: 50,
            theme: "carbon".into(),
            ..Settings::default()
        };
        assert!(merged_if_newer(&db_file, &local).is_none());
        let restored = restore_dropped_customizations(&db_file, &local).unwrap();
        assert_eq!(restored.business_tabs, saved.business_tabs);
        assert!(restored.business_tabs_customized);
        assert_eq!(restored.active_business_tab.as_deref(), Some("tab-5"));
        assert_eq!(restored.mail_profiles, saved.mail_profiles);
        assert_eq!(restored.theme, "carbon");
        assert!(restore_dropped_customizations(&db_file, &saved).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
