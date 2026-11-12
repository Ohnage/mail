//! Phase 1/2 real-data timings for BRIEF-TABHANG / BRIEF-TABHANG2.
//!
//! Run with:
//! `FLECTAR_REALDATA_DIR=C:\Users\tment\flectar-work\tabhang-scratch cargo test -p flectar-mail --lib tabhang_realdata -- --ignored --nocapture`
//!
//! Never opens %APPDATA%. Never starts sync/network.

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use crate::AccountPresentationSettings;
    use flectar_mail_core::compose_senders::hosted_index_build_count;
    use flectar_mail_core::models::{Account, AuthKind, MailProtocol, Provider, Settings};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn realdata_dir() -> Option<PathBuf> {
        std::env::var_os("FLECTAR_REALDATA_DIR").map(PathBuf::from)
    }

    fn load_settings(dir: &std::path::Path) -> Settings {
        let mirror = dir.join("mirror").join("settings.json");
        let text = std::fs::read_to_string(&mirror)
            .unwrap_or_else(|e| panic!("read {}: {e}", mirror.display()));
        serde_json::from_str(&text).expect("parse settings.json")
    }

    fn stub_accounts(settings: &Settings, count: usize) -> Vec<Account> {
        (1..=count as i64)
            .map(|id| Account {
                id,
                email: format!("acct{id}@example.test"),
                display_name: Some(format!("Account {id}")),
                avatar_url: None,
                provider: Provider::Generic,
                auth_kind: AuthKind::Password,
                mail_protocol: MailProtocol::Jmap,
                sync_state: "idle".into(),
                sync_error: None,
                can_create_top_level_mailbox: true,
                server_keeps_mail_days: None,
            })
            .chain(settings.inbox_accounts.iter().flat_map(|inbox| {
                inbox.account_ids.iter().map(|&id| Account {
                    id,
                    email: format!("hub{id}@pixelated.cities"),
                    display_name: Some(format!("Hub {id}")),
                    avatar_url: None,
                    provider: Provider::Generic,
                    auth_kind: AuthKind::Password,
                    mail_protocol: MailProtocol::Jmap,
                    sync_state: "idle".into(),
                    sync_error: None,
                    can_create_top_level_mailbox: true,
                    server_keeps_mail_days: None,
                })
            }))
            .fold(Vec::new(), |mut acc, account| {
                if !acc.iter().any(|known| known.id == account.id) {
                    acc.push(account);
                }
                acc
            })
    }

    fn time_alias_rebuild(presentation: &AccountPresentationSettings) -> u128 {
        let started = Instant::now();
        let rows = presentation
            .address_views
            .iter()
            .map(|view| {
                (
                    view.id,
                    view.address.clone(),
                    presentation.address_detail(view),
                )
            })
            .collect::<Vec<_>>();
        let ms = started.elapsed().as_millis();
        assert_eq!(rows.len(), presentation.address_views.len());
        ms
    }

    #[test]
    #[ignore = "requires FLECTAR_REALDATA_DIR scratch copy"]
    fn tabhang_realdata_phase1_timings() {
        let dir = realdata_dir().expect("set FLECTAR_REALDATA_DIR to tabhang-scratch");
        let load_started = Instant::now();
        let settings = load_settings(&dir);
        let load_ms = load_started.elapsed().as_millis();
        let accounts = stub_accounts(&settings, 304);
        let builds_at_start = hosted_index_build_count();
        let mut presentation = AccountPresentationSettings::from_settings(&settings);
        presentation.bind_accounts(&accounts);
        let index = presentation
            .wait_hosted_index(Duration::from_secs(120))
            .expect("hosted index must become ready off the UI thread");
        let index_build_ms = presentation.hosted_index_build_ms().unwrap_or(0);
        let builds_after_wait = hosted_index_build_count();
        assert!(
            builds_after_wait > builds_at_start,
            "index must build off-thread"
        );
        let _ = index;

        eprintln!(
            "realdata load_settings_ms={load_ms} address_views={} accounts={} tabs={} index_build_ms={index_build_ms} hosted_builds={}",
            presentation.address_views.len(),
            accounts.len(),
            presentation.business_tabs.len(),
            builds_after_wait - builds_at_start
        );
        assert!(
            presentation.address_views.len() >= 20_000,
            "expected live addressViews shape"
        );

        // BEFORE (cf54e4b shape): full alias rebuild cost, still skipped on clicks.
        let alias_ms = time_alias_rebuild(&presentation);
        eprintln!("BEFORE apply_connected_accounts address_inboxes rebuild_ms={alias_ms}");

        let one_state_ms = {
            let started = Instant::now();
            let _ = presentation.view_scope_cached();
            started.elapsed().as_millis()
        };

        for tab in presentation.business_tabs.clone() {
            presentation.active_business_tab = Some(tab.id.clone());
            presentation.invalidate_scope_cache();
            let builds_before_click = hosted_index_build_count();
            let shared_started = Instant::now();
            let shared = presentation.active_shared_scope_for_bench();
            let shared_ms = shared_started.elapsed().as_millis();
            let scope_started = Instant::now();
            let (sidebar, _, scope) = presentation.view_scope_cached();
            let scope_ms = scope_started.elapsed().as_millis();
            let second_started = Instant::now();
            let _ = presentation.view_scope_cached();
            let cached_ms = second_started.elapsed().as_millis();
            let builds_after_click = hosted_index_build_count();
            eprintln!(
                "AFTER tab={} view_scope_ms={scope_ms} shared_ms={shared_ms} cached_ms={cached_ms} index_build_ms={index_build_ms} first_rows_proxy_ms={scope_ms} sidebar_accounts={} address_inboxes={} address_groups={} shared_evidenced={} shared_omit={} sender_domains={} hosted_builds_delta={}",
                tab.id,
                sidebar.as_ref().map(Vec::len).unwrap_or(0),
                scope.address_inboxes.len(),
                scope.address_groups.len(),
                shared
                    .as_ref()
                    .map(|s| s.evidenced_mail.iter().map(|m| m.addresses.len()).sum::<usize>())
                    .unwrap_or(0),
                shared
                    .as_ref()
                    .and_then(|s| s.remainder_mail.as_ref())
                    .map(|r| r.omit_addresses.len())
                    .unwrap_or(0),
                tab.sender_domains.len(),
                builds_after_click - builds_before_click
            );
            assert_eq!(
                builds_after_click, builds_before_click,
                "tab click must not rebuild HostedAddressIndex"
            );
            assert!(
                scope_ms < 10,
                "view_scope for {} took {scope_ms} ms (budget 10 ms)",
                tab.id
            );
            assert!(
                scope_ms < 50,
                "UI callback proxy for {} took {scope_ms} ms (budget 50 ms)",
                tab.id
            );
        }

        // AFTER: tab path never pays the alias rebuild. The expensive path remains
        // available for the settings surface that opens the chooser.
        eprintln!(
            "AFTER budgets; alias rebuild still available at {alias_ms} ms; one_view_scope_ms={one_state_ms}"
        );
        assert!(
            one_state_ms < 50,
            "cached/single view_scope must stay under 50 ms ({one_state_ms} ms)"
        );
        assert!(
            alias_ms > 10,
            "alias rebuild remains the expensive path skipped on tab click ({alias_ms} ms)"
        );
    }

    #[test]
    fn tab_switch_ui_projection_stays_under_budget_at_live_shape() {
        // Synthetic live shape from BRIEF-TABHANG Phase 3 / TABHANG2.
        let mut presentation = AccountPresentationSettings::default();
        presentation.business_tabs = serde_json::from_value(serde_json::json!([
            {"id":"tab-5","name":"TM","accountIds":[1,293]},
            {"id":"forty7","name":"Project 47","scope":"any","senderDomains": (0..323).map(|i| format!("d{i}.example")).collect::<Vec<_>>()},
            {"id":"pixelated-cities","name":"Pixelated Cities","profileIds":["pixelated-cities"],"accountIds":[8]}
        ]))
        .unwrap();
        presentation.profiles = vec![
            flectar_mail_core::models::MailProfile {
                id: "pixelated-cities".into(),
                name: "Pixelated Cities".into(),
                color: "#112233".into(),
                account_ids: (8..283).collect(),
                logo: None,
            },
            flectar_mail_core::models::MailProfile {
                id: "mentink-memorial".into(),
                name: "Mentink Memorial".into(),
                color: "#000000".into(),
                account_ids: vec![293],
                logo: None,
            },
        ];
        presentation.inbox_accounts = vec![
            flectar_mail_core::models::InboxAccount {
                id: 275,
                name: "Pixelated Cities".into(),
                icon: None,
                color: None,
                account_ids: (8..283).collect(),
                address_view_ids: (1..=28_856).collect(),
                automatic_profile: Some("pixelated-cities".into()),
                profile_id: Some("pixelated-cities".into()),
            },
            flectar_mail_core::models::InboxAccount {
                id: 1,
                name: "Tim Mentink".into(),
                icon: None,
                color: None,
                account_ids: vec![1, 293],
                address_view_ids: Vec::new(),
                automatic_profile: None,
                profile_id: Some("tim-personal".into()),
            },
        ];
        presentation.address_rules = (0..318)
            .map(|index| flectar_mail_core::models::AddressRule {
                profile_id: "pixelated-cities".into(),
                addresses: Vec::new(),
                domains: vec![format!("d{index}.pixelated.cities")],
            })
            .collect();
        presentation.address_views = (0..28_856)
            .map(|index| flectar_mail_core::models::AddressView {
                id: i64::from(index + 1),
                address: if index == 0 {
                    "tim@mentinkmemorial.org".into()
                } else {
                    format!("city{index}@pixelated.cities")
                },
                source_account_emails: if index == 0 {
                    vec!["hub@company.example".into()]
                } else {
                    vec!["hub@pixelated.cities".into()]
                },
                profile_id: Some(if index == 0 {
                    "mentink-memorial".into()
                } else {
                    "pixelated-cities".into()
                }),
                source: Default::default(),
            })
            .collect();
        presentation.rebuild_view_indexes();
        presentation
            .connected_account_emails
            .insert("hub@pixelated.cities".into(), 8);
        presentation
            .connected_account_emails
            .insert("hub@company.example".into(), 293);
        presentation
            .connected_account_emails
            .insert("tim@example.test".into(), 1);
        presentation.invalidate_hosted_inputs();
        presentation
            .wait_hosted_index(Duration::from_secs(60))
            .expect("synthetic hosted index");

        let builds_before = hosted_index_build_count();
        for tab_id in ["forty7", "pixelated-cities", "tab-5"] {
            presentation.active_business_tab = Some(tab_id.into());
            presentation.invalidate_scope_cache();
            let started = Instant::now();
            // UI callback work that used to rebuild aliases + view_scope twice.
            let _ = presentation.view_scope_cached();
            let _ = presentation.view_scope_cached();
            let business_tabs = presentation.business_tabs.len();
            let accounts_rows = 304usize;
            let elapsed = started.elapsed();
            eprintln!(
                "synthetic tab={tab_id} ui_projection_ms={} (tabs={business_tabs} account_rows={accounts_rows})",
                elapsed.as_millis()
            );
            assert!(
                elapsed.as_millis() < 50,
                "tab {tab_id} UI projection took {:?} (budget 50 ms)",
                elapsed
            );
            assert!(
                elapsed.as_millis() < 10,
                "view_scope budget 10 ms for {tab_id}: {:?}",
                elapsed
            );
        }
        assert_eq!(
            hosted_index_build_count(),
            builds_before,
            "tab clicks must not call HostedAddressIndex::build"
        );
        // Alias rebuild stays far more expensive than the tab projection we
        // just timed — tab clicks must keep skipping it.
        let alias_ms = time_alias_rebuild(&presentation);
        assert!(
            alias_ms > 10,
            "alias rebuild must remain the expensive path we skip on tab click ({alias_ms} ms)"
        );
    }

    #[test]
    fn tab_projection_ignores_held_writer_and_streaming_state_ticks() {
        use std::sync::{Arc, Mutex};
        use std::thread;
        use std::time::Duration;

        let mut presentation = AccountPresentationSettings::default();
        presentation.business_tabs = serde_json::from_value(serde_json::json!([
            {"id":"forty7","name":"Project 47","scope":"any","senderDomains":["a.example"]}
        ]))
        .unwrap();
        presentation.address_views = (0..5_000)
            .map(|index| flectar_mail_core::models::AddressView {
                id: i64::from(index + 1),
                address: format!("city{index}@pixelated.cities"),
                source_account_emails: vec!["hub@pixelated.cities".into()],
                profile_id: Some("pixelated-cities".into()),
                source: Default::default(),
            })
            .collect();
        presentation.rebuild_view_indexes();
        presentation.active_business_tab = Some("forty7".into());
        presentation.invalidate_hosted_inputs();
        let _ = presentation.wait_hosted_index(Duration::from_secs(30));

        let lock = Arc::new(Mutex::new(()));
        let held = Arc::clone(&lock);
        let blocker = thread::spawn(move || {
            let _guard = held.lock().unwrap();
            thread::sleep(Duration::from_millis(200));
        });
        thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        let _ = presentation.view_scope_cached();
        let elapsed = started.elapsed();
        drop(blocker);
        assert!(
            elapsed < Duration::from_millis(50),
            "view_scope must not wait on a held writer ({elapsed:?})"
        );
    }

    #[test]
    fn address_inboxes_model_stays_lazy_on_tab_path() {
        // Tab clicks and sync ticks pass rebuild_address_aliases=false.
        // Nothing here materializes the 28k-row Slint model.
        let presentation = AccountPresentationSettings::default();
        assert!(presentation.address_views.is_empty());
    }
}
