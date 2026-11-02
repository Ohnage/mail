use super::*;
use flectar_mail_core::models::{AddressView, AddressViewSource};

fn address(id: i64, profile: &str, sources: &[&str]) -> AddressView {
    AddressView {
        id,
        address: format!("alias{id}@example.org"),
        profile_id: Some(profile.into()),
        source_account_emails: sources.iter().map(|value| (*value).into()).collect(),
        source: AddressViewSource::Registry,
    }
}

fn presentation(scope: &str) -> AccountPresentationSettings {
    AccountPresentationSettings {
        address_views: vec![
            address(11, "media", &["storage@example.org"]),
            address(12, "cities", &["other@example.org"]),
        ],
        connected_account_emails: HashMap::from([
            ("storage@example.org".into(), 7),
            ("other@example.org".into(), 8),
        ]),
        active_business_tab: Some("group".into()),
        business_tabs: serde_json::from_value(serde_json::json!([{
            "id":"group", "name":"Group", "profileIds":["media"], "accountIds":[3],
            "scope":scope, "recipientPatterns":["legacy@example.org"],
            "senderAddresses":["sender@example.org"]
        }]))
        .unwrap(),
        ..Default::default()
    }
}

#[test]
fn address_sources_resolve_only_real_accounts_and_disconnected_is_empty() {
    let p = presentation("profiles");
    let filter = p.address_filter(&address(
        11,
        "media",
        &["STORAGE@example.org", "storage@example.org"],
    ));
    assert_eq!(filter.account_ids, Some(vec![7]));
    assert_eq!(
        p.address_filter(&address(13, "media", &["missing@example.org"]))
            .account_ids,
        Some(vec![])
    );
    assert_eq!(
        p.address_filter(&address(14, "media", &[])).account_ids,
        None
    );
}

#[test]
fn business_membership_unions_physical_accounts_with_profile_aliases() {
    let p = presentation("profiles");
    let (sidebar, query, scope) = p.view_scope();
    assert_eq!(sidebar, Some(vec![3]));
    assert!(query.is_none());
    assert!(scope.match_any);
    assert_eq!(scope.or_account_ids, vec![3]);
    assert_eq!(scope.address_inboxes.len(), 1);
    assert_eq!(scope.address_inboxes[0].address, "alias11@example.org");
    assert_eq!(scope.address_inboxes[0].account_ids, Some(vec![7]));
}

#[test]
fn automatic_business_views_do_not_include_every_message_in_shared_storage() {
    let mut p = presentation("profiles");
    p.business_tabs[0].account_ids = None;
    p.address_rules = vec![flectar_mail_core::models::AddressRule {
        profile_id: "media".into(),
        addresses: vec![],
        domains: vec!["example.org".into()],
    }];
    p.profiles = vec![MailProfile {
        id: "media".into(),
        name: "Media".into(),
        color: "#123456".into(),
        account_ids: vec![7],
        logo: None,
    }];
    p.address_views[1].source_account_emails = vec!["storage@example.org".into()];
    let (_, query, scope) = p.view_scope();
    assert!(query.is_none());
    assert!(scope.match_any);
    assert!(
        scope.or_account_ids.is_empty(),
        "inherited storage account must not bypass address classification"
    );
    assert_eq!(scope.address_inboxes.len(), 1);
    assert_eq!(scope.address_inboxes[0].address, "alias11@example.org");
    assert_eq!(scope.address_inboxes[0].account_ids, Some(vec![7]));
    assert_eq!(
        scope.address_groups.len(),
        1,
        "automatic rules must cover previously unknown domain aliases"
    );
    assert_eq!(scope.address_groups[0].profile_ids, vec!["media"]);
    p.business_tabs[0].address_view_ids = Some(vec![11]);
    assert!(
        p.view_scope().2.address_groups.is_empty(),
        "explicit saved membership must never auto-widen"
    );
}

#[test]
fn explicit_alias_memberships_override_inheritance_and_allow_overlap() {
    let mut p = presentation("profiles");
    p.business_tabs[0].address_view_ids = Some(vec![12]);
    assert_eq!(
        p.visible_address_views()
            .iter()
            .map(|view| view.id)
            .collect::<Vec<_>>(),
        vec![12]
    );
    p.business_tabs[0].address_view_ids = Some(vec![]);
    assert!(p.visible_address_views().is_empty());
    assert_eq!(p.view_scope().1, Some(vec![3]));
}

#[test]
fn aliases_preserve_legacy_recipient_and_narrow_constraints() {
    for kind in ["recipients", "narrow", "any"] {
        let p = presentation(kind);
        let (_, query, scope) = p.view_scope();
        assert!(query.is_none());
        assert_eq!(scope.recipient_patterns, vec!["legacy@example.org"]);
        assert_eq!(scope.sender_addresses, vec!["sender@example.org"]);
        assert_eq!(scope.address_inboxes.len(), 1);
        assert_eq!(
            scope.base_account_ids,
            if kind == "narrow" {
                Some(vec![3])
            } else {
                None
            }
        );
    }
}

#[test]
fn address_navigation_keys_round_trip_without_becoming_account_scopes() {
    let scope = Scope::AddressInbox(859);
    assert_eq!(scope.to_string().parse::<Scope>().unwrap(), scope);
    assert_eq!(scope.unified(), scope);
    assert!("AddressInbox:0".parse::<Scope>().is_err());
    assert!("AddressInbox:-1".parse::<Scope>().is_err());
}
