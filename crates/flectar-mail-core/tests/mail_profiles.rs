use flectar_mail_core::{
    Core,
    accounts::credentials::{CredentialStore, CredentialStoreHandle, Slot},
    config::Paths,
    error::{CoreError, Result},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct MemoryCredentials(Mutex<HashMap<(i64, Slot), String>>);

impl CredentialStore for MemoryCredentials {
    fn store(&self, account_id: i64, slot: Slot, secret: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert((account_id, slot), secret.into());
        Ok(())
    }

    fn load(&self, account_id: i64, slot: Slot) -> Result<String> {
        self.0
            .lock()
            .unwrap()
            .get(&(account_id, slot))
            .cloned()
            .ok_or_else(|| CoreError::Auth("no stored credential".into()))
    }

    fn delete(&self, account_id: i64, slot: Slot) -> Result<()> {
        self.0.lock().unwrap().remove(&(account_id, slot));
        Ok(())
    }

    fn delete_all(&self, account_id: i64) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .retain(|(id, _), _| *id != account_id);
        Ok(())
    }
}

async fn seeded_core() -> (tempfile::TempDir, Core) {
    let temp = tempfile::tempdir().unwrap();
    let credentials: CredentialStoreHandle = Arc::new(MemoryCredentials::default());
    let core = Core::start_mail_ui_with_credentials(Paths::for_tests(temp.path()), credentials)
        .await
        .unwrap();
    core.db
        .write(|conn| {
            conn.execute_batch(
                "INSERT INTO accounts
                    (id,email,provider,auth_kind,username,incoming_host,incoming_port,
                     smtp_host,smtp_port,created_at,mail_protocol,settings_json)
                 VALUES
                    (1,'alice@example.org','generic','password','alice',
                     'imap.example.org',993,'smtp.example.org',465,0,'imap','{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}'),
                    (2,'bob@example.org','generic','password','bob',
                     'imap.example.org',993,'smtp.example.org',465,0,'imap','{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}')",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    (temp, core)
}

#[tokio::test]
async fn named_accounts_share_aliases_and_tabs_save_atomically_without_catalog_refresh() {
    use flectar_mail_core::models::InboxAccount;
    let (_temp, core) = seeded_core().await;
    let settings = core
        .save_address_view("alias@example.org".into(), vec!["alice@example.org".into()])
        .await
        .unwrap();
    let address = settings.address_views[0].id;
    for name in ["Design", "Support"] {
        core.save_inbox_account(InboxAccount {
            id: 0,
            name: name.into(),
            icon: None,
            color: None,
            account_ids: vec![2],
            address_view_ids: vec![address],
            automatic_profile: None,
            profile_id: None,
        })
        .await
        .unwrap();
    }
    let settings = core.get_settings().await.unwrap();
    assert_eq!(settings.inbox_accounts.len(), 2);
    let members = settings
        .inbox_accounts
        .iter()
        .map(|account| account.id)
        .collect::<Vec<_>>();
    let saved = core
        .save_account_tab(
            None,
            "Business".into(),
            None,
            true,
            Some(members.clone()),
            None,
        )
        .await
        .unwrap();
    let tab = saved.business_tabs[0].clone();
    let unchanged = core
        .save_account_tab(
            Some(tab.id.clone()),
            tab.name.clone(),
            tab.icon.clone(),
            tab.show_name,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(unchanged.business_tabs[0], tab);
    let mut sorted = members.clone();
    sorted.sort_unstable();
    assert_eq!(unchanged.business_tab_accounts[&tab.id], sorted);
    let mut edited = unchanged.inbox_accounts[0].clone();
    edited.name = "Design renamed".into();
    edited.icon = Some("profile-logos/seed.png".into());
    core.save_inbox_account(edited.clone()).await.unwrap();
    core.save_inbox_account(edited).await.unwrap();
    let before = core.get_settings().await.unwrap();
    assert!(
        core.save_account_tab(
            Some(tab.id.clone()),
            "Must not commit".into(),
            None,
            false,
            Some(vec![-999]),
            None,
        )
        .await
        .is_err()
    );
    let after = core.get_settings().await.unwrap();
    assert_eq!(after.business_tabs, before.business_tabs);
    assert_eq!(after.business_tab_accounts, before.business_tab_accounts);
    let deleted = core.delete_inbox_account(members[0]).await.unwrap();
    assert_eq!(deleted.inbox_accounts.len(), 1);
    assert_eq!(deleted.business_tab_accounts[&tab.id], vec![members[1]]);
    assert_eq!(
        core.list_accounts().await.unwrap().len(),
        2,
        "Removing a view never disconnects mailboxes"
    );
}

#[test]
fn tab_membership_distinguishes_legacy_inheritance_from_explicit_empty() {
    use flectar_mail_core::models::{BusinessTab, Settings};
    let legacy: BusinessTab = serde_json::from_value(serde_json::json!({
        "id": "legacy", "name": "Legacy", "profileIds": ["work"]
    }))
    .unwrap();
    assert_eq!(legacy.account_ids, None);
    assert_eq!(legacy.icon, None);
    assert!(legacy.show_name);
    assert_eq!(legacy.address_view_ids, None);
    let mut settings = Settings {
        business_tabs: vec![
            legacy.clone(),
            BusinessTab {
                id: "empty".into(),
                account_ids: Some(Vec::new()),
                address_view_ids: Some(Vec::new()),
                ..legacy
            },
        ],
        ..Settings::default()
    };
    settings
        .account_icons
        .insert("1".into(), "account-icons/account-1.png".into());
    let restored: Settings =
        serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
    assert_eq!(restored.business_tabs[0].account_ids, None);
    assert_eq!(restored.business_tabs[1].account_ids, Some(Vec::new()));
    assert_eq!(restored.business_tabs[0].address_view_ids, None);
    assert_eq!(restored.business_tabs[1].address_view_ids, Some(Vec::new()));
    assert_eq!(restored.account_icons, settings.account_icons);
    assert!(!restored.business_tabs_customized);
}

#[tokio::test]
async fn presentation_only_tab_save_keeps_automatic_membership_for_future_business_aliases() {
    use flectar_mail_core::models::AddressRule;
    let (_temp, core) = seeded_core().await;
    let mut settings = core.get_settings().await.unwrap();
    settings
        .mail_profiles
        .push(flectar_mail_core::models::MailProfile {
            id: "work".into(),
            name: "Work".into(),
            color: "#123456".into(),
            account_ids: vec![1],
            logo: None,
        });
    settings.address_rules = vec![AddressRule {
        profile_id: "work".into(),
        addresses: vec![],
        domains: vec!["work.example".into()],
    }];
    settings.business_tabs = serde_json::from_value(
        serde_json::json!([{"id":"work","name":"Work","scope":"profiles","profileIds":["work"]}]),
    )
    .unwrap();
    core.set_settings(settings).await.unwrap();
    let updated = core
        .save_business_tab_details(
            Some("work".into()),
            "Renamed".into(),
            None,
            None,
            Some("tab-icons/presentation.png".into()),
            false,
        )
        .await
        .unwrap();
    assert_eq!(updated.business_tabs[0].account_ids, None);
    assert_eq!(updated.business_tabs[0].address_view_ids, None);
    assert_eq!(updated.business_tabs[0].profile_ids, vec!["work"]);
    assert!(!updated.business_tabs[0].show_name);
    assert_eq!(
        flectar_mail_core::brand_registry::classify_receiving_address(
            &updated.address_rules,
            "not-yet-in-catalog@work.example"
        )
        .as_deref(),
        Some("work")
    );
    assert!(
        updated
            .address_views
            .iter()
            .all(|view| view.address != "not-yet-in-catalog@work.example")
    );
    let later = core
        .save_address_view("later@work.example".into(), vec![])
        .await
        .unwrap();
    assert_eq!(
        later
            .address_views
            .iter()
            .find(|view| view.address == "later@work.example")
            .unwrap()
            .profile_id
            .as_deref(),
        Some("work")
    );
    assert_eq!(later.business_tabs[0].account_ids, None);
    assert_eq!(later.business_tabs[0].address_view_ids, None);
    let explicit = core
        .save_business_tab_details(
            Some("work".into()),
            "Renamed".into(),
            Some(vec![]),
            Some(vec![]),
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(explicit.business_tabs[0].account_ids, Some(vec![]));
    assert_eq!(explicit.business_tabs[0].address_view_ids, Some(vec![]));
}

#[tokio::test]
async fn receiving_catalog_refresh_reads_pending_sender_rows_without_authorizing_or_ensuring_senders()
 {
    let (_temp, core) = seeded_core().await;
    core.db.write(|conn| {
        conn.execute("INSERT INTO sender_identities(account_id,email,verification_status,last_synced_at) VALUES (1,'pending@alias.example','pending',0)", [])?;
        Ok(())
    }).await.unwrap();
    let before = core
        .db
        .read(|conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM sender_identities", [], |row| {
                    row.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .unwrap();
    let settings = core.refresh_address_views().await.unwrap();
    let alias = settings
        .address_views
        .iter()
        .find(|view| view.address == "pending@alias.example")
        .unwrap();
    assert_eq!(alias.source_account_emails, vec!["alice@example.org"]);
    assert_eq!(
        core.db
            .read(|conn| Ok(conn.query_row(
                "SELECT COUNT(*) FROM sender_identities",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        before
    );
    assert_eq!(core.db.read(|conn| Ok(conn.query_row("SELECT verification_status FROM sender_identities WHERE email='pending@alias.example'", [], |row| row.get::<_, String>(0))?)).await.unwrap(), "pending");
    assert_eq!(
        core.refresh_address_views().await.unwrap().address_views,
        settings.address_views
    );
}

#[tokio::test]
async fn receiving_only_manual_addresses_and_tab_selection_persist_without_account_or_sender_changes()
 {
    let (_temp, core) = seeded_core().await;
    let accounts = core.list_accounts().await.unwrap();
    let sender_count = core
        .db
        .read(|conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM sender_identities", [], |row| {
                    row.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .unwrap();
    let settings = core
        .save_address_view(
            " Tim@memorial.example ".into(),
            vec!["ALICE@example.org".into(), "alice@example.org".into()],
        )
        .await
        .unwrap();
    let view = &settings.address_views[0];
    assert_eq!(view.address, "tim@memorial.example");
    assert_eq!(view.source_account_emails, vec!["alice@example.org"]);
    let id = view.id;
    let settings = core
        .save_business_tab_details(
            None,
            "Memorial".into(),
            Some(vec![]),
            Some(vec![id, id]),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(settings.business_tabs[0].address_view_ids, Some(vec![id]));
    assert!(!settings.business_tabs[0].show_name);
    assert!(
        core.save_business_tab_details(
            None,
            "Bad ID".into(),
            Some(vec![]),
            Some(vec![i64::MAX]),
            None,
            true
        )
        .await
        .is_err()
    );
    assert!(
        core.save_address_view("*@memorial.example".into(), vec![])
            .await
            .is_err()
    );
    assert!(
        core.save_address_view("other@memorial.example".into(), vec!["bad source".into()])
            .await
            .is_err()
    );
    let reloaded = core.get_settings().await.unwrap();
    assert_eq!(reloaded.business_tabs, settings.business_tabs);
    assert_eq!(core.list_accounts().await.unwrap().len(), accounts.len());
    assert_eq!(
        core.db
            .read(|conn| Ok(conn.query_row(
                "SELECT COUNT(*) FROM sender_identities",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        sender_count
    );
    assert_eq!(
        core.save_address_view("TIM@MEMORIAL.EXAMPLE".into(), vec![])
            .await
            .unwrap()
            .address_views[0]
            .id,
        id
    );
}

#[tokio::test]
async fn tab_details_are_many_to_many_and_preserve_profile_assignments() {
    let (_temp, core) = seeded_core().await;
    let (_, profile) = core
        .save_mail_profile(None, "Work".into(), "#123456".into())
        .await
        .unwrap();
    let before = core
        .assign_account_profile(1, Some(profile.id))
        .await
        .unwrap()
        .mail_profiles;
    let settings = core
        .save_business_tab_details(
            None,
            "First".into(),
            Some(vec![2, 1, 2]),
            Some(Vec::new()),
            Some("tab-icons/tab-first.png".into()),
            true,
        )
        .await
        .unwrap();
    let first = settings.business_tabs[0].id.clone();
    let settings = core
        .save_business_tab_details(
            None,
            "Second".into(),
            Some(vec![1]),
            Some(Vec::new()),
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(settings.business_tabs[0].account_ids, Some(vec![1, 2]));
    assert_eq!(settings.business_tabs[1].account_ids, Some(vec![1]));
    assert_eq!(settings.mail_profiles, before);
    assert!(settings.business_tabs_customized);
    assert!(settings.business_tab_icons_initialized);
    let settings = core
        .save_business_tab_details(
            Some(first),
            "Empty".into(),
            Some(vec![]),
            Some(Vec::new()),
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(settings.business_tabs[0].account_ids, Some(vec![]));
    assert_eq!(
        core.get_settings().await.unwrap().business_tabs,
        settings.business_tabs
    );
}

#[tokio::test]
async fn editing_rule_tabs_preserves_tim_and_forty7_predicates() {
    let (_temp, core) = seeded_core().await;
    let mut settings = core.get_settings().await.unwrap();
    settings.business_tabs = serde_json::from_value(serde_json::json!([
        {"id":"tim", "name":"Tim", "scope":"recipients", "profileIds":["personal"], "recipientPatterns":["tim@*.example"]},
        {"id":"forty7", "name":"Forty7", "scope":"any", "profileIds":["work"], "recipientPatterns":["agent@example.org"], "senderAddresses":["agent@example.org"], "senderNames":["Agent"], "senderDomains":["example.org"]}
    ])).unwrap();
    core.set_settings(settings.clone()).await.unwrap();
    for original in settings.business_tabs {
        let updated = core
            .save_business_tab_details(
                Some(original.id.clone()),
                format!("{} renamed", original.name),
                Some(vec![2]),
                Some(Vec::new()),
                Some("tab-icons/tab-custom.png".into()),
                true,
            )
            .await
            .unwrap();
        let tab = updated
            .business_tabs
            .iter()
            .find(|tab| tab.id == original.id)
            .unwrap();
        assert_eq!(tab.scope, original.scope);
        assert_eq!(tab.profile_ids, original.profile_ids);
        assert_eq!(tab.recipient_patterns, original.recipient_patterns);
        assert_eq!(tab.sender_addresses, original.sender_addresses);
        assert_eq!(tab.sender_names, original.sender_names);
        assert_eq!(tab.sender_domains, original.sender_domains);
        assert_eq!(tab.account_ids, Some(vec![2]));
    }
}

#[tokio::test]
async fn account_and_tab_icon_validation_is_atomic() {
    let (_temp, core) = seeded_core().await;
    let (_, profile) = core
        .save_mail_profile(None, "Work".into(), "#123456".into())
        .await
        .unwrap();
    core.assign_account_profile(1, Some(profile.id))
        .await
        .unwrap();
    let before = core
        .set_account_icon(1, Some("account-icons/account-one.png".into()))
        .await
        .unwrap();
    assert_eq!(before.mail_profiles[0].account_ids, vec![1]);
    for path in [
        "../escape.png",
        "C:/escape.png",
        "account-icons/../escape.png",
        "tab-icons/sub/icon.png",
        "account-icons/icon.svg:stream",
    ] {
        assert!(core.set_account_icon(1, Some(path.into())).await.is_err());
        assert!(
            core.save_business_tab_details(
                None,
                "Invalid".into(),
                Some(vec![1]),
                Some(Vec::new()),
                Some(path.into()),
                true
            )
            .await
            .is_err()
        );
    }
    assert!(
        core.set_account_icon(1, Some("tab-icons/wrong.png".into()))
            .await
            .is_err()
    );
    assert!(
        core.save_business_tab_details(
            None,
            "Invalid".into(),
            Some(vec![1]),
            Some(Vec::new()),
            Some("account-icons/wrong.png".into()),
            true
        )
        .await
        .is_err()
    );
    assert!(core.set_account_icon(999, None).await.is_err());
    assert!(
        core.save_business_tab_details(
            None,
            "Invalid".into(),
            Some(vec![999]),
            Some(Vec::new()),
            None,
            true
        )
        .await
        .is_err()
    );
    assert!(
        core.save_business_tab_details(
            None,
            " ".into(),
            Some(vec![]),
            Some(Vec::new()),
            None,
            true
        )
        .await
        .is_err()
    );
    assert!(
        core.save_business_tab_details(
            Some("missing".into()),
            "Missing".into(),
            Some(vec![]),
            Some(Vec::new()),
            None,
            true
        )
        .await
        .is_err()
    );
    assert_eq!(
        core.get_settings().await.unwrap().account_icons,
        before.account_icons
    );
    assert!(!core.get_settings().await.unwrap().business_tabs_customized);
    assert!(
        core.set_account_icon(1, None)
            .await
            .unwrap()
            .account_icons
            .is_empty()
    );
}

#[tokio::test]
async fn every_tab_mutation_marks_the_list_customized() {
    let (_temp, core) = seeded_core().await;
    let settings = core
        .save_business_tab(None, "First".into(), vec![], vec![])
        .await
        .unwrap();
    assert!(settings.business_tabs_customized);
    assert!(settings.business_tab_icons_initialized);
    let first = settings.business_tabs[0].id.clone();
    let mut settings = core
        .save_business_tab(None, "Second".into(), vec![], vec![])
        .await
        .unwrap();
    settings.business_tabs_customized = false;
    core.set_settings(settings).await.unwrap();
    let mut settings = core.move_business_tab(first.clone(), 1).await.unwrap();
    assert!(settings.business_tabs_customized);
    assert!(settings.business_tab_icons_initialized);
    assert_eq!(settings.business_tabs[1].id, first);
    settings.business_tabs_customized = false;
    core.set_settings(settings).await.unwrap();
    assert!(
        core.delete_business_tab(first)
            .await
            .unwrap()
            .business_tabs_customized
    );
}

#[tokio::test]
async fn profiles_are_validated_persisted_and_assign_each_account_once() {
    let (_temp, core) = seeded_core().await;

    let (_, work) = core
        .save_mail_profile(None, " Work ".into(), "#3b82f6".into())
        .await
        .unwrap();
    let (_, personal) = core
        .save_mail_profile(None, "Personal".into(), "#DB2777".into())
        .await
        .unwrap();
    assert_eq!(work.name, "Work");
    assert_eq!(work.color, "#3B82F6");
    assert!(
        core.save_mail_profile(None, "work".into(), "#000000".into())
            .await
            .is_err()
    );
    assert!(
        core.save_mail_profile(None, "Invalid".into(), "blue".into())
            .await
            .is_err()
    );

    core.assign_account_profile(1, Some(work.id.clone()))
        .await
        .unwrap();
    let (_, renamed_work) = core
        .save_mail_profile(Some(work.id.clone()), "Office".into(), "#0891B2".into())
        .await
        .unwrap();
    assert_eq!(renamed_work.account_ids, vec![1]);
    assert_eq!(renamed_work.name, "Office");
    let settings = core
        .assign_account_profile(1, Some(personal.id.clone()))
        .await
        .unwrap();
    assert!(
        settings
            .mail_profiles
            .iter()
            .find(|profile| profile.id == work.id)
            .unwrap()
            .account_ids
            .is_empty()
    );
    assert_eq!(
        settings
            .mail_profiles
            .iter()
            .find(|profile| profile.id == personal.id)
            .unwrap()
            .account_ids,
        vec![1]
    );

    let settings = core
        .set_account_color(1, Some("#f97316".into()))
        .await
        .unwrap();
    assert_eq!(
        settings.account_colors.get("1").map(String::as_str),
        Some("#F97316")
    );
    assert!(
        settings
            .mail_profiles
            .iter()
            .all(|profile| !profile.account_ids.contains(&1))
    );
    let settings = core.set_account_color(1, None).await.unwrap();
    assert!(!settings.account_colors.contains_key("1"));
    assert!(
        core.set_account_color(99, Some("#000000".into()))
            .await
            .is_err()
    );
    assert!(
        core.set_account_color(1, Some("orange".into()))
            .await
            .is_err()
    );

    assert!(
        core.assign_account_profile(2, Some("missing".into()))
            .await
            .is_err()
    );

    let settings = core.delete_mail_profile(personal.id).await.unwrap();
    assert_eq!(settings.mail_profiles.len(), 1);
    assert!(
        core.list_accounts()
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == 1)
    );
}

#[tokio::test]
async fn removing_an_account_cleans_local_profile_membership() {
    let (_temp, core) = seeded_core().await;
    let (_, profile) = core
        .save_mail_profile(None, "Work".into(), "#0D9488".into())
        .await
        .unwrap();
    core.assign_account_profile(1, Some(profile.id.clone()))
        .await
        .unwrap();

    core.remove_account(1).await.unwrap();

    let settings = core.get_settings().await.unwrap();
    assert!(
        settings
            .mail_profiles
            .iter()
            .find(|candidate| candidate.id == profile.id)
            .unwrap()
            .account_ids
            .is_empty()
    );
    assert!(
        core.list_accounts()
            .await
            .unwrap()
            .iter()
            .all(|a| a.id != 1)
    );
}

#[tokio::test]
async fn the_shown_profile_needs_accounts_and_is_dropped_when_it_loses_them() {
    let (_temp, core) = seeded_core().await;
    let (_, work) = core
        .save_mail_profile(None, "Work".into(), "#3B82F6".into())
        .await
        .unwrap();
    let (_, personal) = core
        .save_mail_profile(None, "Personal".into(), "#DB2777".into())
        .await
        .unwrap();
    let shown =
        |settings: &flectar_mail_core::models::Settings| settings.active_mail_profile.clone();

    // A profile without accounts would show an empty mail view.
    assert!(
        core.set_active_mail_profile(Some(work.id.clone()))
            .await
            .is_err()
    );
    assert!(
        core.set_active_mail_profile(Some("missing".into()))
            .await
            .is_err()
    );
    core.assign_account_profile(1, Some(work.id.clone()))
        .await
        .unwrap();
    core.assign_account_profile(2, Some(personal.id.clone()))
        .await
        .unwrap();
    let settings = core
        .set_active_mail_profile(Some(work.id.clone()))
        .await
        .unwrap();
    assert_eq!(shown(&settings), Some(work.id.clone()));
    assert_eq!(
        shown(&core.get_settings().await.unwrap()),
        Some(work.id.clone())
    );
    // Renaming keeps it shown; an empty id shows every account.
    core.save_mail_profile(Some(work.id.clone()), "Office".into(), "#0891B2".into())
        .await
        .unwrap();
    assert_eq!(
        shown(&core.get_settings().await.unwrap()),
        Some(work.id.clone())
    );
    let settings = core
        .set_active_mail_profile(Some(" ".into()))
        .await
        .unwrap();
    assert_eq!(shown(&settings), None);

    // Moving the last account away.
    core.set_active_mail_profile(Some(work.id.clone()))
        .await
        .unwrap();
    let settings = core
        .assign_account_profile(1, Some(personal.id.clone()))
        .await
        .unwrap();
    assert_eq!(shown(&settings), None);

    // Giving the last account its own color.
    let settings = core
        .set_active_mail_profile(Some(personal.id.clone()))
        .await
        .unwrap();
    assert_eq!(shown(&settings), Some(personal.id.clone()));
    core.set_account_color(1, Some("#F97316".into()))
        .await
        .unwrap();
    assert_eq!(
        shown(&core.get_settings().await.unwrap()),
        Some(personal.id.clone()),
        "the profile still has account 2"
    );
    let settings = core
        .set_account_color(2, Some("#F97316".into()))
        .await
        .unwrap();
    assert_eq!(shown(&settings), None);

    // Removing the last account.
    core.assign_account_profile(2, Some(personal.id.clone()))
        .await
        .unwrap();
    core.set_active_mail_profile(Some(personal.id.clone()))
        .await
        .unwrap();
    core.remove_account(2).await.unwrap();
    assert_eq!(shown(&core.get_settings().await.unwrap()), None);

    // Deleting the shown profile.
    core.assign_account_profile(1, Some(work.id.clone()))
        .await
        .unwrap();
    core.set_active_mail_profile(Some(work.id.clone()))
        .await
        .unwrap();
    let settings = core.delete_mail_profile(work.id).await.unwrap();
    assert_eq!(shown(&settings), None);
}
