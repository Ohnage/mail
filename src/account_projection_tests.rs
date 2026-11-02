use super::*;
use flectar_mail_core::models::BusinessTab;

fn tab(id: &str, scope: &str, account_ids: Option<Vec<i64>>) -> BusinessTab {
    BusinessTab {
        id: id.into(),
        name: id.into(),
        profile_ids: vec!["legacy".into()],
        recipient_patterns: vec!["tim@example.org".into()],
        scope: scope.into(),
        sender_addresses: vec!["sender@example.org".into()],
        sender_names: vec!["Tim".into()],
        sender_domains: vec!["example.org".into()],
        icon: None,
        account_ids,
    }
}

fn presentation() -> AccountPresentationSettings {
    AccountPresentationSettings {
        profiles: vec![MailProfile {
            id: "legacy".into(),
            name: "Legacy".into(),
            color: "#112233".into(),
            account_ids: vec![1, 2],
            logo: None,
        }],
        ..Default::default()
    }
}

fn image(side: u32) -> slint::Image {
    slint::Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(
        side, side,
    ))
}

#[test]
fn account_icon_override_wins_and_is_independent_of_selected_tab() {
    let mut p = presentation();
    p.profile_logos.insert("legacy".into(), image(2));
    p.account_icon_images.insert("1".into(), image(3));
    p.tab_icons.insert("a".into(), image(4));
    p.business_tabs = vec![
        tab("a", "profiles", Some(vec![1])),
        tab("b", "profiles", Some(vec![1, 2])),
    ];
    for selected in [None, Some("a"), Some("b")] {
        p.active_business_tab = selected.map(str::to_owned);
        assert_eq!(p.account_icon(1).unwrap().size().width, 3);
        assert_eq!(p.account_icon(2).unwrap().size().width, 2);
        assert!(p.account_icon(99).is_none());
    }
    p.account_icon_images.remove("1");
    assert_eq!(p.account_icon(1).unwrap().size().width, 2);
}

#[test]
fn explicit_memberships_support_overlap_deduplication_and_empty_views() {
    let mut p = presentation();
    p.business_tabs = vec![
        tab("a", "profiles", Some(vec![3, 1, 3])),
        tab("b", "profiles", Some(vec![1, 4])),
        tab("empty", "profiles", Some(vec![])),
        tab("legacy", "profiles", None),
    ];
    for (id, expected) in [
        ("a", vec![1, 3]),
        ("b", vec![1, 4]),
        ("empty", vec![]),
        ("legacy", vec![1, 2]),
    ] {
        p.active_business_tab = Some(id.into());
        let (sidebar, query, _) = p.view_scope();
        assert_eq!(sidebar, Some(expected.clone()));
        assert_eq!(query, Some(expected));
    }
}

#[test]
fn explicit_membership_preserves_recipient_any_and_narrow_rules() {
    let mut p = presentation();
    for scope_name in ["recipients", "any", "narrow"] {
        p.business_tabs = vec![tab("rules", scope_name, Some(vec![7, 7]))];
        p.active_business_tab = Some("rules".into());
        let (sidebar, query, scope) = p.view_scope();
        assert_eq!(scope.recipient_patterns, vec!["tim@example.org"]);
        assert_eq!(scope.sender_addresses, vec!["sender@example.org"]);
        assert_eq!(scope.sender_names, vec!["Tim"]);
        assert_eq!(scope.sender_domains, vec!["example.org"]);
        match scope_name {
            "recipients" => {
                assert_eq!(sidebar, Some(vec![7]));
                assert!(query.is_none());
                assert!(!scope.match_any);
            }
            "any" => {
                assert_eq!(sidebar, Some(vec![7]));
                assert!(query.is_none());
                assert!(scope.match_any);
                assert_eq!(scope.or_account_ids, vec![7]);
            }
            _ => {
                assert_eq!(sidebar, Some(vec![7]));
                assert_eq!(query, Some(vec![7]));
                assert!(!scope.match_any);
            }
        }
    }
}

#[test]
fn recipient_tab_sidebar_membership_does_not_limit_recipient_matching() {
    let mut p = presentation();
    for (membership, expected_sidebar) in [
        (None, None),
        (Some(vec![]), Some(vec![])),
        (Some(vec![7, 3, 7]), Some(vec![3, 7])),
    ] {
        p.business_tabs = vec![tab("rules", "recipients", membership)];
        p.active_business_tab = Some("rules".into());
        let (sidebar, query, scope) = p.view_scope();
        assert_eq!(sidebar, expected_sidebar);
        assert!(query.is_none());
        assert_eq!(scope.recipient_patterns, vec!["tim@example.org"]);
        assert_eq!(scope.sender_addresses, vec!["sender@example.org"]);
        assert_eq!(scope.sender_names, vec!["Tim"]);
        assert_eq!(scope.sender_domains, vec!["example.org"]);
        assert!(scope.or_account_ids.is_empty());
        assert!(!scope.match_any);
    }
}

#[test]
fn removed_account_clears_icon_and_all_explicit_memberships() {
    let mut p = presentation();
    p.account_icons
        .insert("1".into(), "account-icons/one.png".into());
    p.account_icon_images.insert("1".into(), image(3));
    p.business_tabs = vec![
        tab("a", "profiles", Some(vec![1])),
        tab("b", "profiles", Some(vec![1, 2])),
    ];
    p.remove_account(1);
    assert!(!p.account_icons.contains_key("1"));
    assert!(p.account_icon(1).is_none());
    assert_eq!(p.business_tabs[0].account_ids, Some(vec![]));
    assert_eq!(p.business_tabs[1].account_ids, Some(vec![2]));
}
