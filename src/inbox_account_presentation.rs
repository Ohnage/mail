use super::*;
use flectar_mail_core::{
    models::InboxAccount,
    recipient_match::{AddressGroupScope, MailScope},
};

impl AccountPresentationSettings {
    pub(super) fn selected_inbox_accounts(
        &self,
        tab: &flectar_mail_core::models::BusinessTab,
    ) -> Vec<i64> {
        if let Some(ids) = self.business_tab_accounts.get(&tab.id) {
            return ids.clone();
        }
        let profile_ids = tab.profile_ids.iter().cloned().collect::<HashSet<_>>();
        let account_ids = tab
            .account_ids
            .as_ref()
            .map(|ids| ids.iter().copied().collect::<HashSet<_>>());
        let address_view_ids = tab
            .address_view_ids
            .as_ref()
            .map(|ids| ids.iter().copied().collect::<HashSet<_>>());
        self.inbox_accounts
            .iter()
            .filter(|account| {
                account
                    .profile_id
                    .as_ref()
                    .is_some_and(|id| profile_ids.contains(id))
                    || account_ids.as_ref().is_some_and(|ids| {
                        !account.account_ids.is_empty()
                            && account.account_ids.iter().all(|id| ids.contains(id))
                    })
                    || address_view_ids.as_ref().is_some_and(|ids| {
                        !account.address_view_ids.is_empty()
                            && account.address_view_ids.iter().all(|id| ids.contains(id))
                    })
            })
            .map(|account| account.id)
            .collect()
    }

    pub(super) fn visible_inbox_accounts(&self) -> Vec<&InboxAccount> {
        let ids = self
            .active_business()
            .map(|tab| self.selected_inbox_accounts(tab));
        self.inbox_accounts
            .iter()
            .filter(|account| ids.as_ref().is_none_or(|ids| ids.contains(&account.id)))
            .collect()
    }

    pub(super) fn inbox_members(&self, account: &InboxAccount) -> Vec<i64> {
        // The saved id list is authoritative. Scanning every address view to
        // re-derive it is O(views × ids) and hangs sidebar rebuilds when one
        // named account covers tens of thousands of addresses.
        if !account.address_view_ids.is_empty() {
            return account.address_view_ids.clone();
        }
        let Some(profile) = account.automatic_profile.as_ref() else {
            return Vec::new();
        };
        self.address_views
            .iter()
            .filter(|view| view.profile_id.as_ref() == Some(profile))
            .map(|view| view.id)
            .collect()
    }

    pub(super) fn inbox_member_count(&self, account: &InboxAccount) -> usize {
        if !account.address_view_ids.is_empty() {
            return account.address_view_ids.len();
        }
        self.inbox_members(account).len()
    }

    pub(super) fn inbox_scope(&self, account: &InboxAccount) -> MailScope {
        const ALIAS_INBOX_CEILING: usize = 512;
        let mailbox_ids =
            flectar_mail_core::inbox_accounts::mailbox_ids(account, &self.profiles);
        // Prefer the saved count — cloning tens of thousands of address_view_ids
        // just to decide prefer_group was burning tens of ms per tab click.
        let member_count = self.inbox_member_count(account);
        let prefer_group = member_count > ALIAS_INBOX_CEILING
            && !self.address_rules.is_empty()
            && account
                .automatic_profile
                .as_ref()
                .or(account.profile_id.as_ref())
                .is_some();
        let address_inboxes = if prefer_group {
            Vec::new()
        } else {
            let member_ids = self.inbox_members(account);
            let ids = member_ids.into_iter().collect::<HashSet<_>>();
            self.address_views
                .iter()
                .filter(|view| ids.contains(&view.id))
                .map(|view| self.address_filter(view))
                .collect()
        };
        let address_groups = account
            .automatic_profile
            .as_ref()
            .or(account.profile_id.as_ref())
            .filter(|_| prefer_group || account.automatic_profile.is_some())
            .map(|id| {
                vec![AddressGroupScope {
                    profile_ids: vec![id.clone()],
                    rules: self.address_rules.clone(),
                }]
            })
            .unwrap_or_default();
        MailScope {
            match_any: true,
            or_account_ids: mailbox_ids,
            address_inboxes,
            address_groups,
            ..Default::default()
        }
    }

    pub(super) fn named_tab_scope(
        &self,
        tab: &flectar_mail_core::models::BusinessTab,
    ) -> MailScope {
        let selected = self.selected_inbox_accounts(tab);
        let mut scope = MailScope {
            match_any: true,
            recipient_patterns: tab.recipient_patterns.clone(),
            sender_addresses: tab.sender_addresses.clone(),
            sender_names: tab.sender_names.clone(),
            sender_domains: tab.sender_domains.clone(),
            ..Default::default()
        };
        for account in self
            .inbox_accounts
            .iter()
            .filter(|account| selected.contains(&account.id))
        {
            let account_scope = self.inbox_scope(account);
            scope.or_account_ids.extend(account_scope.or_account_ids);
            scope.address_inboxes.extend(account_scope.address_inboxes);
            scope.address_groups.extend(account_scope.address_groups);
        }
        scope.or_account_ids.sort_unstable();
        scope.or_account_ids.dedup();
        scope
    }

    pub(super) fn inbox_title(&self, id: i64) -> String {
        self.inbox_accounts
            .iter()
            .find(|account| account.id == id)
            .map(|account| account.name.clone())
            .or_else(|| {
                self.address_views
                    .iter()
                    .find(|view| view.id == id)
                    .map(|view| view.address.clone())
            })
            .unwrap_or_else(|| "Account inbox".into())
    }

    pub(super) fn inbox_choice(&self, account: &InboxAccount) -> InboxViewChoiceRow {
        let image = self
            .account_icon_images
            .get(&format!("inbox:{}", account.id));
        InboxViewChoiceRow {
            key: format!("inbox:{}", account.id).into(),
            kind: InboxViewKind::NamedAccount,
            name: account.name.clone().into(),
            detail: format!(
                "{} mailboxes · {} addresses",
                flectar_mail_core::inbox_accounts::mailbox_ids(account, &self.profiles).len(),
                self.inbox_member_count(account)
            )
            .into(),
            initials: avatar_initials(&account.name).into(),
            avatar: image.cloned().unwrap_or_default(),
            has_avatar: image.is_some(),
            checked: false,
            color: self
                .saved_inbox_color(account)
                .unwrap_or_else(|| fallback_account_color(account.id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn account(id: i64, addresses: Vec<i64>) -> InboxAccount {
        InboxAccount {
            id,
            name: format!("Account {}", id.unsigned_abs()),
            icon: None,
            color: None,
            account_ids: vec![],
            address_view_ids: addresses,
            automatic_profile: None,
            profile_id: None,
        }
    }
    fn presentation() -> AccountPresentationSettings {
        let mut p = AccountPresentationSettings { address_views: serde_json::from_value(serde_json::json!([
            {"id":11,"address":"design@example.org","sourceAccountEmails":["storage@example.org"]},
            {"id":12,"address":"support@example.org","sourceAccountEmails":["missing@example.org"]}
        ]))
        .unwrap(), ..Default::default() };
        p.connected_account_emails
            .insert("storage@example.org".into(), 7);
        p.inbox_accounts = vec![account(-1, vec![11]), account(-2, vec![11, 12])];
        p.business_tabs = serde_json::from_value(serde_json::json!([
            {"id":"one","name":"One","profileIds":[]},
            {"id":"two","name":"Two","profileIds":[]}
        ]))
        .unwrap();
        p.business_tab_accounts.insert("one".into(), vec![-1, -2]);
        p.business_tab_accounts.insert("two".into(), vec![-1]);
        p.active_business_tab = Some("one".into());
        p
    }
    #[test]
    fn named_account_and_tab_filters_share_exact_alias_sources() {
        let p = presentation();
        let one = p.inbox_scope(&p.inbox_accounts[0]);
        assert_eq!(one.address_inboxes[0].account_ids, Some(vec![7]));
        assert!(one.or_account_ids.is_empty());
        let two = p.inbox_scope(&p.inbox_accounts[1]);
        assert_eq!(two.address_inboxes[1].account_ids, Some(vec![]));
        let (sidebar, query, combined) = p.view_scope();
        assert_eq!(sidebar, Some(vec![]));
        assert_eq!(query, Some(vec![]));
        assert!(combined.match_any);
        assert_eq!(combined.address_inboxes.len(), 3);
        assert!(combined.or_account_ids.is_empty());
    }
    #[test]
    fn shared_accounts_follow_edits_in_every_tab_and_empty_tabs_stay_empty() {
        let mut p = presentation();
        assert_eq!(p.visible_inbox_accounts().len(), 2);
        p.inbox_accounts[0].address_view_ids = vec![12];
        p.active_business_tab = Some("two".into());
        assert_eq!(p.visible_inbox_accounts().len(), 1);
        assert_eq!(
            p.view_scope().2.address_inboxes[0].address,
            "support@example.org"
        );
        p.business_tab_accounts.insert("two".into(), vec![]);
        assert!(p.visible_inbox_accounts().is_empty());
        let scope = p.view_scope().2;
        assert!(scope.match_any);
        assert!(scope.address_inboxes.is_empty());
        assert!(scope.or_account_ids.is_empty());
    }
    #[test]
    fn named_account_navigation_has_a_distinct_persistent_key() {
        let scope = Scope::AddressInbox(-1234);
        assert_eq!(scope.to_string(), "InboxAccount:1234");
        assert_eq!(scope.to_string().parse::<Scope>().unwrap(), scope);
        assert!("InboxAccount:0".parse::<Scope>().is_err());
        assert!("InboxAccount:-1".parse::<Scope>().is_err());
    }
    #[test]
    fn a_linked_profile_adds_mailboxes_without_rewriting_the_account() {
        use flectar_mail_core::models::MailProfile;
        let mut p = presentation();
        p.inbox_accounts[0].profile_id = Some("work".into());
        p.inbox_accounts[0].account_ids = vec![1];
        p.profiles = vec![MailProfile {
            id: "work".into(),
            name: "Work".into(),
            color: "#112233".into(),
            account_ids: vec![1, 8],
            logo: None,
        }];
        assert_eq!(p.inbox_scope(&p.inbox_accounts[0]).or_account_ids, vec![1, 8]);
        p.profiles[0].account_ids.push(9);
        assert_eq!(
            p.inbox_scope(&p.inbox_accounts[0]).or_account_ids,
            vec![1, 8, 9]
        );
        assert!(p.inbox_choice(&p.inbox_accounts[0]).detail.starts_with("3 mailboxes"));
    }

    #[test]
    fn company_inbox_keeps_a_selected_accounts_mailbox_and_skips_other_accounts() {
        use flectar_mail_core::models::{Account, AuthKind, MailProfile, MailProtocol, Provider};
        let mut p = presentation();
        p.inbox_accounts[0].name = "Company".into();
        p.inbox_accounts[0].profile_id = Some("work".into());
        p.inbox_accounts[0].account_ids = vec![4];
        p.inbox_accounts[1].name = "Personal".into();
        p.inbox_accounts[1].account_ids = vec![8];
        p.profiles = vec![MailProfile {
            id: "work".into(),
            name: "Work".into(),
            color: "#112233".into(),
            account_ids: vec![4, 8],
            logo: None,
        }];
        assert_eq!(
            p.inbox_scope(&p.inbox_accounts[0]).or_account_ids,
            vec![4, 8],
            "a selected account keeps a mailbox another account also lists"
        );
        p.inbox_accounts[0].account_ids = vec![4, 8];
        assert_eq!(
            p.inbox_scope(&p.inbox_accounts[0]).or_account_ids,
            vec![4, 8]
        );
        p.inbox_accounts[0].account_ids = vec![4];
        p.active_business_tab = Some("one".into());
        let (sidebar, query, scope) = p.view_scope();
        assert_eq!(sidebar, Some(vec![4, 8]));
        assert_eq!(query, Some(vec![4, 8]));
        assert_eq!(scope.or_account_ids, vec![4, 8]);
        p.active_business_tab = Some("two".into());
        let (sidebar, query, scope) = p.view_scope();
        assert_eq!(sidebar, Some(vec![4, 8]));
        assert_eq!(query, Some(vec![4, 8]));
        assert_eq!(scope.or_account_ids, vec![4, 8]);
        let accounts = [4, 8, 99]
            .into_iter()
            .map(|id| Account {
                id,
                email: format!("box{id}@example.test"),
                display_name: None,
                avatar_url: None,
                provider: Provider::Generic,
                auth_kind: AuthKind::Password,
                mail_protocol: MailProtocol::Imap,
                sync_state: "idle".into(),
                sync_error: None,
                can_create_top_level_mailbox: true,
                server_keeps_mail_days: None,
            })
            .collect::<Vec<_>>();
        let company = p.company_account_ids(&accounts);
        assert_eq!(company, vec![4, 8]);
        assert_ne!(
            company,
            accounts.iter().map(|account| account.id).collect::<Vec<_>>()
        );
    }

    /// Kept from 8fb1508. It only times `view_scope` and caps address_inboxes,
    /// so it missed the UI-thread rebuild of every AddressView into
    /// `address_inboxes` plus sync-state floods calling that rebuild. See
    /// `tabhang_bench` for the regression that covers that path.
    #[test]
    fn company_tab_switch_stays_cheap_with_tens_of_thousands_of_address_views() {
        let mut p = presentation();
        p.business_tabs = serde_json::from_value(serde_json::json!([
            {"id":"tab-5","name":"TM","accountIds":[7]},
            {"id":"pixelated-cities","name":"Pixelated Cities","profileIds":["pixelated-cities"],"accountIds":[8]}
        ]))
        .unwrap();
        p.profiles = vec![flectar_mail_core::models::MailProfile {
            id: "pixelated-cities".into(),
            name: "Pixelated Cities".into(),
            color: "#112233".into(),
            account_ids: vec![8],
            logo: None,
        }];
        p.address_rules = vec![flectar_mail_core::models::AddressRule {
            profile_id: "pixelated-cities".into(),
            addresses: Vec::new(),
            domains: vec!["pixelated.cities".into()],
        }];
        p.address_views = (0..20_000)
            .map(|index| flectar_mail_core::models::AddressView {
                id: i64::from(index + 1),
                address: format!("city{index}@pixelated.cities"),
                source_account_emails: vec!["hub@pixelated.cities".into()],
                profile_id: Some("pixelated-cities".into()),
                source: Default::default(),
            })
            .collect();
        p.connected_account_emails
            .insert("hub@pixelated.cities".into(), 8);
        p.rebuild_view_indexes();
        p.active_business_tab = Some("pixelated-cities".into());
        let started = std::time::Instant::now();
        let (sidebar, _, scope) = p.view_scope();
        let elapsed = started.elapsed();
        assert_eq!(sidebar, Some(vec![8]));
        assert!(
            scope.address_inboxes.len() <= 512,
            "tab switch must not explode address_inboxes ({})",
            scope.address_inboxes.len()
        );
        assert!(
            !scope.address_groups.is_empty() || scope.or_account_ids == vec![8],
            "large catalogs fall back to mailbox/group scope"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "view_scope took {elapsed:?} with {} address views",
            p.address_views.len()
        );
        let account = InboxAccount {
            id: -9,
            name: "Pixelated Cities".into(),
            icon: None,
            color: None,
            account_ids: vec![8],
            address_view_ids: (1..=20_000).collect(),
            automatic_profile: Some("pixelated-cities".into()),
            profile_id: Some("pixelated-cities".into()),
        };
        let started = std::time::Instant::now();
        let detail = p.inbox_choice(&account).detail.to_string();
        let elapsed = started.elapsed();
        assert!(detail.contains("20000 addresses"));
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "inbox_choice took {elapsed:?}"
        );
    }
}
