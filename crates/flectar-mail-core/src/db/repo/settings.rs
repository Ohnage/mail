use crate::error::Result;
use crate::models::Settings;
use rusqlite::{Connection, OptionalExtension, params};

pub fn get(conn: &Connection) -> Result<Settings> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'settings'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match json {
        Some(json) => {
            let mut value: serde_json::Value = serde_json::from_str(&json)?;
            if let Some(object) = value.as_object_mut()
                && object.contains_key("businessTabs")
                && !object.contains_key("businessTabsCustomized")
            {
                // Before the customization marker existed, an explicitly saved
                // list could contain local renames, ordering or deletions. Treat
                // it as authoritative; pre-tab settings without a list may seed.
                object.insert("businessTabsCustomized".into(), true.into());
            }
            Ok(serde_json::from_value(value)?)
        }
        None => Ok(Settings::default()),
    }
}

pub fn set(conn: &Connection, settings: &Settings) -> Result<()> {
    let mut value = serde_json::to_value(settings)?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "mirrorUpdatedAt".into(),
            serde_json::json!(chrono::Utc::now().timestamp_millis()),
        );
    }
    let json = serde_json::to_string(&value)?;
    if let Some(path) = database_file(conn) {
        backup_previous(conn, &path)?;
    }
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES ('settings', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![json],
    )?;
    // Stage the committed JSON. An open transaction flushes it from the
    // commit hook; an autocommit statement has already committed, so flush now.
    if let Some(path) = database_file(conn) {
        crate::settings_mirror::stage(path, serde_json::to_string(&value)?);
        if conn.is_autocommit() {
            crate::settings_mirror::flush_pending();
        }
    }
    Ok(())
}

fn backup_previous(conn: &Connection, db_file: &std::path::Path) -> Result<()> {
    let previous: Option<String> = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'settings'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(previous) = previous else {
        return Ok(());
    };
    let path = db_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mirror")
        .join("settings.previous.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, previous)?;
    if std::fs::rename(&temporary, &path).is_err() {
        std::fs::remove_file(&path)?;
        std::fs::rename(&temporary, &path)?;
    }
    Ok(())
}

fn database_file(conn: &Connection) -> Option<std::path::PathBuf> {
    let path: String = conn
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .ok()?;
    if path.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(path))
    }
}

/// Record a settings mirror when a mail transaction commits, and drop it
/// when that transaction rolls back.
pub(crate) fn install_mirror_hooks(conn: &Connection) -> Result<()> {
    conn.commit_hook(Some(|| {
        crate::settings_mirror::flush_pending();
        false
    }))?;
    conn.rollback_hook(Some(|| {
        crate::settings_mirror::clear_pending();
    }))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil;

    fn registry_for_migration_test() -> crate::brand_registry::BrandRegistry {
        crate::brand_registry::parse_brand_registry(
            br##"{
            "profiles":[{"id":"work","name":"Refreshed Work","color":"#123456"}],
            "tabs":[{"id":"work","name":"Seeded","profiles":["work"]}]
        }"##,
        )
        .unwrap()
    }

    #[test]
    fn pre_feature_saved_tab_lists_survive_import_including_empty_deletions() {
        for tabs in [
            serde_json::json!([
                {"id":"second","name":"Renamed second","profileIds":[]},
                {"id":"first","name":"Renamed first","profileIds":[]}
            ]),
            serde_json::json!([]),
        ] {
            let conn = testutil::conn();
            let mut raw = serde_json::to_value(Settings::default()).unwrap();
            let object = raw.as_object_mut().unwrap();
            object.remove("businessTabsCustomized");
            object.remove("businessTabIconsInitialized");
            object.insert("businessTabs".into(), tabs);
            conn.execute(
                "INSERT INTO app_settings (key,value) VALUES ('settings',?1)",
                [raw.to_string()],
            )
            .unwrap();
            let legacy = get(&conn).unwrap();
            assert!(legacy.business_tabs_customized);
            assert!(!legacy.business_tab_icons_initialized);
            let refreshed = crate::brand_registry::apply_brand_registry(
                &legacy,
                &[],
                &registry_for_migration_test(),
                |_| Err("unused".into()),
            )
            .unwrap()
            .settings;
            assert_eq!(refreshed.business_tabs, legacy.business_tabs);
            assert_eq!(refreshed.mail_profiles[0].name, "Refreshed Work");
            set(&conn, &refreshed).unwrap();
            assert!(get(&conn).unwrap().business_tabs_customized);
        }
    }

    #[test]
    fn pre_tab_settings_without_a_list_remain_eligible_for_registry_seed() {
        let conn = testutil::conn();
        conn.execute(
            "INSERT INTO app_settings (key,value) VALUES ('settings',?1)",
            [r#"{"theme":"snow","undoSendSeconds":20,"loadRemoteImages":false}"#],
        )
        .unwrap();
        let old = get(&conn).unwrap();
        assert!(!old.business_tabs_customized);
        let seeded = crate::brand_registry::apply_brand_registry(
            &old,
            &[],
            &registry_for_migration_test(),
            |_| Err("unused".into()),
        )
        .unwrap()
        .settings;
        assert_eq!(seeded.business_tabs[0].name, "Seeded");
        assert!(!seeded.business_tabs_customized);
        set(&conn, &Settings::default()).unwrap();
        assert!(
            !get(&conn).unwrap().business_tabs_customized,
            "explicit modern false stays false even with an empty list"
        );
    }

    #[test]
    fn defaults_when_unset_and_roundtrip() {
        let c = testutil::conn();
        let d = get(&c).unwrap();
        assert!(d.notifications_enabled);
        assert_eq!(d.sync_interval_minutes, 5);
        assert!(d.auto_advance);
        assert!(d.auto_labels_enabled);
        assert!(d.mark_read_on_open);
        assert!(!d.load_remote_images);
        assert!(!d.monochrome_sidebar_icons);
        assert_eq!(d.app_icon, crate::models::AppIcon::Orange);
        assert!(d.show_avatars);
        assert!(d.group_mail_by_date);
        assert!(d.show_tabs);
        assert!(d.conversation_view);
        assert!(d.collect_outgoing_contacts);
        assert!(!d.collect_incoming_contacts);
        assert!(!d.contact_suggest_all_accounts);
        assert!(d.suggest_learned_contacts);
        assert_eq!(d.workspace_layout, "default");
        assert_eq!(d.calendar_week_start, "monday");
        assert_eq!(d.theme_preset, "default");
        assert_eq!(
            d.message_appearance,
            crate::models::MessageAppearance::System
        );
        assert_eq!(d.custom_theme.light_primary, "#0969DA");
        assert!(d.mail_profiles.is_empty());
        assert!(d.active_mail_profile.is_none());
        assert!(!d.show_account_badges);
        assert!(d.download_directory.is_empty());

        let mut s = d.clone();
        s.theme = "carbon".into();
        s.message_appearance = crate::models::MessageAppearance::Light;
        s.calendar_week_start = "sunday".into();
        s.notifications_enabled = false;
        s.monochrome_sidebar_icons = true;
        s.app_icon = crate::models::AppIcon::Graphite;
        s.show_avatars = false;
        s.group_mail_by_date = false;
        s.show_tabs = false;
        s.conversation_view = false;
        s.workspace_layout = "minimal".into();
        s.contact_suggest_all_accounts = true;
        s.theme_preset = "teal".into();
        s.download_directory = "/srv/downloads".into();
        s.custom_theme.light_primary = "#006B7A".into();
        s.signature_list.push(crate::models::Signature {
            id: "a".into(),
            account_id: 1,
            name: "Work".into(),
            html: "<b>Dean</b>".into(),
        });
        s.signature_defaults.insert(
            "1".into(),
            crate::models::SignatureDefaults {
                new_id: Some("a".into()),
                reply_id: None,
            },
        );
        s.ai_automation_rules.push(crate::models::AiAutomationRule {
            id: "invoice".into(),
            name: "Invoices".into(),
            source_prompt: "For vendor invoices, move to Important".into(),
            instruction: "It is a vendor invoice".into(),
            enabled: true,
            actions: vec![crate::models::AiAutomationAction {
                kind: "route_to".into(),
                value: "important".into(),
            }],
        });
        s.mail_profiles.push(crate::models::MailProfile {
            id: "work".into(),
            name: "Work".into(),
            color: "#3B82F6".into(),
            account_ids: vec![1, 2],
            logo: None,
        });
        s.active_mail_profile = Some("work".into());
        set(&c, &s).unwrap();

        let back = get(&c).unwrap();
        assert_eq!(back.theme, "carbon");
        assert_eq!(
            back.message_appearance,
            crate::models::MessageAppearance::Light
        );
        assert_eq!(back.calendar_week_start, "sunday");
        assert!(!back.notifications_enabled);
        assert!(back.monochrome_sidebar_icons);
        assert_eq!(back.app_icon, crate::models::AppIcon::Graphite);
        assert!(!back.show_avatars);
        assert!(!back.group_mail_by_date);
        assert!(!back.show_tabs);
        assert!(!back.conversation_view);
        assert!(back.contact_suggest_all_accounts);
        assert_eq!(back.workspace_layout, "minimal");
        assert_eq!(back.theme_preset, "teal");
        assert_eq!(back.download_directory, "/srv/downloads");
        assert_eq!(back.custom_theme.light_primary, "#006B7A");
        assert_eq!(back.signature_list.len(), 1);
        assert_eq!(back.signature_list[0].html, "<b>Dean</b>");
        assert_eq!(back.ai_automation_rules, s.ai_automation_rules);
        assert_eq!(back.mail_profiles, s.mail_profiles);
        assert_eq!(back.active_mail_profile.as_deref(), Some("work"));
        assert_eq!(
            back.signature_defaults
                .get("1")
                .and_then(|d| d.new_id.as_deref()),
            Some("a")
        );
    }

    /// Blobs written before new fields existed must deserialize with defaults.
    #[test]
    fn old_blob_gets_field_defaults() {
        let c = testutil::conn();
        c.execute(
            "INSERT INTO app_settings (key, value) VALUES ('settings',
             '{\"theme\":\"snow\",\"undoSendSeconds\":20,\"loadRemoteImages\":true}')",
            [],
        )
        .unwrap();
        let s = get(&c).unwrap();
        assert_eq!(s.theme, "snow");
        assert_eq!(s.undo_send_seconds, 20);
        assert!(s.load_remote_images);
        assert!(s.mark_read_on_open);
        assert!(!s.monochrome_sidebar_icons);
        assert_eq!(s.app_icon, crate::models::AppIcon::Orange);
        assert!(s.show_avatars);
        assert!(s.group_mail_by_date);
        assert!(s.show_tabs);
        assert!(s.conversation_view);
        assert!(s.collect_outgoing_contacts);
        assert!(!s.collect_incoming_contacts);
        assert!(!s.contact_suggest_all_accounts);
        assert!(s.suggest_learned_contacts);
        assert_eq!(s.workspace_layout, "default");
        assert_eq!(s.calendar_week_start, "monday");
        assert_eq!(s.theme_preset, "default");
        assert_eq!(s.custom_theme, crate::models::CustomTheme::default());
        assert!(s.notifications_enabled);
        assert_eq!(s.sync_interval_minutes, 5);
        assert!(s.auto_advance);
        assert!(s.auto_labels_enabled);
        assert!(s.ai_automation_rules.is_empty());
        assert!(s.mail_profiles.is_empty());
        assert!(s.active_mail_profile.is_none());
        assert!(!s.show_account_badges);
        assert!(s.download_directory.is_empty());
        assert!(!s.ai_model_instant.is_empty());
        assert!(!s.ai_model_cheap.is_empty());
        assert!(!s.ai_model_intelligent.is_empty());
    }

    #[test]
    fn malformed_settings_are_reported_instead_of_silently_reset() {
        let c = testutil::conn();
        c.execute(
            "INSERT INTO app_settings (key, value) VALUES ('settings', '{not-json')",
            [],
        )
        .unwrap();

        assert!(get(&c).is_err());
    }

    #[test]
    fn saving_the_open_tab_keeps_the_catalog_and_the_tab_list() {
        let conn = testutil::conn();
        let settings = Settings {
            business_tabs_customized: true,
            business_tabs: vec![
                serde_json::from_value(serde_json::json!({"id": "tab-5", "name": "TM"})).unwrap(),
                serde_json::from_value(serde_json::json!({"id": "media", "name": "Mentink Media"}))
                    .unwrap(),
            ],
            active_business_tab: Some("tab-5".into()),
            address_views: vec![
                serde_json::from_value(serde_json::json!({
                    "id": 1,
                    "address": "tim@mentinkmemorial.org"
                }))
                .unwrap(),
            ],
            ..Settings::default()
        };
        set(&conn, &settings).unwrap();
        let mut stored = get(&conn).unwrap();
        stored.active_business_tab = Some("media".into());
        stored.active_mail_profile = None;
        let views = stored.address_views.clone();
        let tabs = stored.business_tabs.clone();
        set(&conn, &stored).unwrap();
        let again = get(&conn).unwrap();
        assert_eq!(again.business_tabs, tabs);
        assert_eq!(again.address_views, views);
        assert_eq!(again.active_business_tab.as_deref(), Some("media"));
    }

    #[test]
    fn a_settings_write_keeps_the_previous_document() {
        let dir = std::env::temp_dir().join(format!(
            "flectar-settings-backup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_file = dir.join("flectar-mail.db");
        let mut conn = rusqlite::Connection::open(&db_file).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        crate::db::migrations::run(&mut conn).unwrap();
        let first = Settings {
            business_tabs_customized: true,
            business_tabs: vec![
                serde_json::from_value(serde_json::json!({"id": "tab-5", "name": "TM"})).unwrap(),
            ],
            ..Settings::default()
        };
        set(&conn, &first).unwrap();
        set(&conn, &Settings::default()).unwrap();
        let previous =
            std::fs::read_to_string(dir.join("mirror").join("settings.previous.json")).unwrap();
        assert!(previous.contains("TM"));
        let mirror_previous =
            std::fs::read_to_string(dir.join("mirror").join("settings.mirror.previous.json"))
                .unwrap();
        assert!(mirror_previous.contains("TM"));
        assert!(get(&conn).unwrap().business_tabs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
