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
        self.inbox_accounts
            .iter()
            .filter(|account| {
                account
                    .profile_id
                    .as_ref()
                    .is_some_and(|id| tab.profile_ids.contains(id))
                    || tab.account_ids.as_ref().is_some_and(|ids| {
                        !account.account_ids.is_empty()
                            && account.account_ids.iter().all(|id| ids.contains(id))
                    })
                    || tab.address_view_ids.as_ref().is_some_and(|ids| {
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
        self.address_views
            .iter()
            .filter(|view| {
                account.address_view_ids.contains(&view.id)
                    || account
                        .automatic_profile
                        .as_ref()
                        .is_some_and(|id| view.profile_id.as_ref() == Some(id))
            })
            .map(|view| view.id)
            .collect()
    }

    pub(super) fn inbox_scope(&self, account: &InboxAccount) -> MailScope {
        let ids = self
            .inbox_members(account)
            .into_iter()
            .collect::<HashSet<_>>();
        MailScope {
            match_any: true,
            or_account_ids: account.account_ids.clone(),
            address_inboxes: self
                .address_views
                .iter()
                .filter(|view| ids.contains(&view.id))
                .map(|view| self.address_filter(view))
                .collect(),
            address_groups: account
                .automatic_profile
                .as_ref()
                .map(|id| {
                    vec![AddressGroupScope {
                        profile_ids: vec![id.clone()],
                        rules: self.address_rules.clone(),
                    }]
                })
                .unwrap_or_default(),
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
                account.account_ids.len(),
                self.inbox_members(account).len()
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
        assert!(query.is_none());
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
}
