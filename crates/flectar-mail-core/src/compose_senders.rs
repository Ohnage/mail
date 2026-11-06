//! Compose groups authorized From addresses by business tab.
//!
//! A shared mailbox can authorize addresses for more than one company. The
//! mailbox's profile decides where that connection is listed, not which of
//! its authorized addresses may be sent. Tim-named addresses stay on the
//! existing Tim tab. Other addresses stay on the company tab whose profiles
//! include them. Nothing here invents an address the provider did not
//! authorize, and a wildcard is an authorization, not a From address.

use crate::brand_registry::classify_receiving_address;
use crate::models::{AddressRule, AddressView, BusinessTab, MailProfile, SenderIdentity};
use crate::recipient_match::email_matches;
use std::collections::{BTreeMap, BTreeSet};

/// One provider-authorized address and the account that may submit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeSender {
    pub email: String,
    pub display_name: Option<String>,
    pub account_id: i64,
}

/// A business tab's sendable addresses. The id and name are the tab's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeGroup {
    pub id: String,
    pub name: String,
    pub senders: Vec<ComposeSender>,
}

/// A connected mailbox plus the identities its provider has authorized.
#[derive(Debug, Clone)]
pub struct ComposeAccount {
    pub id: i64,
    pub email: String,
    pub profile_id: Option<String>,
    pub identities: Vec<SenderIdentity>,
}

/// The Tim group the user already has. A customized tab that lists Tim's
/// mailboxes wins over a name, so "TM" is kept and a second Tim tab is not
/// created. Name is only the fallback when no tab lists those mailboxes.
pub fn existing_tim_tab<'a>(
    tabs: &'a [BusinessTab],
    tim_mailbox_ids: &[i64],
) -> Option<&'a BusinessTab> {
    let overlap = |tab: &BusinessTab| {
        tab.account_ids.as_ref().map_or(0, |ids| {
            ids.iter().filter(|id| tim_mailbox_ids.contains(id)).count()
        })
    };
    tabs.iter()
        .filter(|tab| overlap(tab) > 0)
        .max_by_key(|tab| overlap(tab))
        .or_else(|| {
            tabs.iter().find(|tab| {
                tab.name.eq_ignore_ascii_case("Tim Mentink") || tab.id.eq_ignore_ascii_case("tim")
            })
        })
        .or_else(|| {
            tabs.iter().find(|tab| {
                tab.name.eq_ignore_ascii_case("Tim Business")
                    || tab.name.eq_ignore_ascii_case("TM")
                    || tab.id.eq_ignore_ascii_case("tim-personal")
            })
        })
}

fn is_tim_tab(tab: &BusinessTab, tim: Option<&BusinessTab>) -> bool {
    tim.is_some_and(|tim| tim.id == tab.id)
}

pub fn is_tim_address(tim: Option<&BusinessTab>, email: &str) -> bool {
    if tim.is_some_and(|tab| {
        tab.recipient_patterns
            .iter()
            .any(|pattern| email_matches(pattern, email))
    }) {
        return true;
    }
    let Some((local, _)) = email.trim().rsplit_once('@') else {
        return false;
    };
    let local = local
        .split('+')
        .next()
        .unwrap_or(local)
        .to_ascii_lowercase();
    local == "tim"
        || local == "timothy"
        || local == "tmentink"
        || local.starts_with("tim.")
        || local.starts_with("tmentink")
}

/// Group every authorized address. Tabs with no authorized address are omitted.
pub fn compose_groups(
    tabs: &[BusinessTab],
    address_views: &[AddressView],
    address_rules: &[AddressRule],
    accounts: &[ComposeAccount],
    profiles: &[MailProfile],
    tim_mailbox_ids: &[i64],
) -> Vec<ComposeGroup> {
    let tim = existing_tim_tab(tabs, tim_mailbox_ids);
    let mut chosen: BTreeMap<String, Candidate> = BTreeMap::new();
    for account in accounts {
        for identity in &account.identities {
            if !identity.is_verified() {
                continue;
            }
            if identity.is_selectable() {
                consider(
                    &mut chosen,
                    Candidate::exact(account, identity, identity.email.clone()),
                );
            } else if identity.is_wildcard() {
                // A wildcard authorizes a domain. Only addresses already tied
                // to this mailbox are offered, not the rest of the registry.
                for view in address_views {
                    if view_hosted_by(view, account, profiles) && identity.authorizes(&view.address)
                    {
                        consider(
                            &mut chosen,
                            Candidate::expanded(account, identity, view.address.clone()),
                        );
                    }
                }
            }
        }
    }

    if tabs.is_empty() {
        return account_groups(accounts, &chosen);
    }

    let mut grouped: BTreeMap<String, Vec<ComposeSender>> = BTreeMap::new();
    let mut unmatched: Vec<Candidate> = Vec::new();
    for candidate in chosen.into_values() {
        if let Some(tab) = group_tab(tabs, tim, address_views, address_rules, &candidate) {
            grouped
                .entry(tab.id.clone())
                .or_default()
                .push(candidate.sender());
        } else {
            unmatched.push(candidate);
        }
    }
    for candidate in unmatched {
        grouped
            .entry(format!("mailbox-{}", candidate.account_id))
            .or_default()
            .push(candidate.sender());
    }

    let mut groups = Vec::new();
    let mut used = BTreeSet::new();
    for tab in tabs {
        if is_tim_tab(tab, tim) && tim.is_some_and(|tim| tim.id != tab.id) {
            continue;
        }
        let Some(mut senders) = grouped.remove(&tab.id) else {
            continue;
        };
        senders.sort_by(|left, right| left.email.cmp(&right.email));
        used.insert(tab.id.clone());
        groups.push(ComposeGroup {
            id: tab.id.clone(),
            name: tab.name.clone(),
            senders,
        });
    }
    let mut leftovers = grouped.into_iter().collect::<Vec<_>>();
    leftovers.sort_by(|left, right| left.0.cmp(&right.0));
    for (id, mut senders) in leftovers {
        if used.contains(&id) {
            continue;
        }
        senders.sort_by(|left, right| left.email.cmp(&right.email));
        let name = senders
            .first()
            .and_then(|sender| {
                accounts
                    .iter()
                    .find(|account| account.id == sender.account_id)
                    .map(|account| account.email.clone())
            })
            .unwrap_or_else(|| id.clone());
        groups.push(ComposeGroup { id, name, senders });
    }
    groups
}

/// The account that may submit `email`, when a group offers it.
pub fn transport_account(groups: &[ComposeGroup], email: &str) -> Option<i64> {
    let email = email.trim();
    groups.iter().find_map(|group| {
        group
            .senders
            .iter()
            .find(|sender| sender.email.eq_ignore_ascii_case(email))
            .map(|sender| sender.account_id)
    })
}

/// The group that should be selected when compose opens. A requested address
/// wins, then the tab the user is already looking at.
pub fn compose_group_index(
    groups: &[ComposeGroup],
    requested_email: &str,
    active_tab_id: Option<&str>,
) -> usize {
    if !requested_email.is_empty()
        && let Some(index) = groups.iter().position(|group| {
            group
                .senders
                .iter()
                .any(|sender| sender.email.eq_ignore_ascii_case(requested_email))
        })
    {
        return index;
    }
    active_tab_id
        .and_then(|id| groups.iter().position(|group| group.id == id))
        .unwrap_or(0)
}

/// One tab and the mailboxes its inbox currently lists, before address evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabMailboxSet {
    pub id: String,
    pub mailbox_ids: Vec<i64>,
}

/// How the open tab lists a mailbox that also belongs to the Tim tab.
///
/// `None` means the tab does not share storage with that Tim tab, or no owned
/// Tim address is hosted there, so the caller must not hide mail by guessing.
pub fn shared_tab_scope(
    tabs: &[BusinessTab],
    tab_mailboxes: &[TabMailboxSet],
    tim_mailbox_ids: &[i64],
    active_tab_id: &str,
    address_views: &[AddressView],
    profiles: &[MailProfile],
    account_emails: &[(i64, String)],
) -> Option<crate::recipient_match::MailScope> {
    use crate::recipient_match::{EvidencedMail, MailScope, RemainderMail};
    let tim = existing_tim_tab(tabs, tim_mailbox_ids)?;
    let active = tab_mailboxes.iter().find(|tab| tab.id == active_tab_id)?;
    let tim_set = tab_mailboxes.iter().find(|tab| tab.id == tim.id)?;
    let mut catalog = tim_set.mailbox_ids.clone();
    for tab in tab_mailboxes {
        catalog.extend(tab.mailbox_ids.iter().copied());
    }
    catalog.sort_unstable();
    catalog.dedup();
    let tim_hosts = catalog
        .iter()
        .copied()
        .filter(|id| {
            hosted_addresses(address_views, profiles, account_emails, &[*id])
                .iter()
                .any(|address| is_tim_address(Some(tim), address))
        })
        .collect::<Vec<_>>();
    // A mailbox both tabs list is shared storage. A company mailbox that
    // hosts a Tim address is split too, even when the Tim tab does not list
    // that mailbox yet, so Tim mail is not left on the company tab.
    let mut split = if active.id == tim.id {
        active
            .mailbox_ids
            .iter()
            .copied()
            .filter(|id| other_tab_has(tab_mailboxes, &tim.id, *id))
            .chain(tim_hosts.iter().copied().filter(|id| {
                other_tab_has(tab_mailboxes, &tim.id, *id) && !active.mailbox_ids.contains(id)
            }))
            .collect::<Vec<_>>()
    } else {
        active
            .mailbox_ids
            .iter()
            .copied()
            .filter(|id| tim_set.mailbox_ids.contains(id) || tim_hosts.contains(id))
            .collect::<Vec<_>>()
    };
    split.sort_unstable();
    split.dedup();
    if split.is_empty() {
        return None;
    }
    let tim_addresses = hosted_addresses(address_views, profiles, account_emails, &split)
        .into_iter()
        .filter(|address| is_tim_address(Some(tim), address))
        .collect::<Vec<_>>();
    if tim_addresses.is_empty() {
        return None;
    }
    let mut exclusive = active.mailbox_ids.clone();
    exclusive.retain(|id| !split.contains(id));
    exclusive.sort_unstable();
    exclusive.dedup();
    let mut scope = MailScope {
        match_any: true,
        or_account_ids: exclusive,
        ..MailScope::default()
    };
    if active.id == tim.id {
        scope.evidenced_mail.push(EvidencedMail {
            account_ids: split,
            addresses: tim_addresses,
        });
    } else {
        let keep = hosted_addresses(address_views, profiles, account_emails, &split)
            .into_iter()
            .filter(|address| !is_tim_address(Some(tim), address))
            .collect::<Vec<_>>();
        scope.remainder_mail = Some(RemainderMail {
            account_ids: split,
            omit_addresses: tim_addresses,
            keep_addresses: keep,
        });
    }
    Some(scope)
}

fn other_tab_has(tabs: &[TabMailboxSet], tim_id: &str, mailbox_id: i64) -> bool {
    tabs.iter()
        .any(|tab| tab.id != tim_id && tab.mailbox_ids.contains(&mailbox_id))
}

fn hosted_addresses(
    views: &[AddressView],
    profiles: &[MailProfile],
    account_emails: &[(i64, String)],
    account_ids: &[i64],
) -> Vec<String> {
    let mut addresses = Vec::new();
    for id in account_ids {
        let email = account_emails
            .iter()
            .find(|(account_id, _)| account_id == id)
            .map(|(_, email)| email.as_str())
            .unwrap_or("");
        for view in views {
            if !view_hosted_by_mailbox(view, *id, email, profiles) {
                continue;
            }
            let address = view.address.trim().to_ascii_lowercase();
            if address.is_empty() || addresses.iter().any(|kept: &String| kept == &address) {
                continue;
            }
            addresses.push(address);
        }
    }
    addresses.sort();
    addresses
}

fn view_hosted_by_mailbox(
    view: &AddressView,
    account_id: i64,
    account_email: &str,
    profiles: &[MailProfile],
) -> bool {
    if !account_email.is_empty()
        && view
            .source_account_emails
            .iter()
            .any(|email| email.eq_ignore_ascii_case(account_email))
    {
        return true;
    }
    view.profile_id.as_ref().is_some_and(|profile_id| {
        profiles
            .iter()
            .any(|profile| profile.id == *profile_id && profile.account_ids.contains(&account_id))
    })
}

pub fn sender_account_id(group: &ComposeGroup, email: &str) -> Option<i64> {
    group
        .senders
        .iter()
        .find(|sender| sender.email.eq_ignore_ascii_case(email))
        .map(|sender| sender.account_id)
}

fn view_hosted_by(view: &AddressView, account: &ComposeAccount, profiles: &[MailProfile]) -> bool {
    if view
        .source_account_emails
        .iter()
        .any(|email| email.eq_ignore_ascii_case(&account.email))
    {
        return true;
    }
    view.profile_id.as_ref().is_some_and(|profile_id| {
        profiles
            .iter()
            .any(|profile| profile.id == *profile_id && profile.account_ids.contains(&account.id))
    })
}

#[derive(Clone)]
struct Candidate {
    email: String,
    display_name: Option<String>,
    account_id: i64,
    account_email: String,
    exact: bool,
}

impl Candidate {
    fn exact(account: &ComposeAccount, identity: &SenderIdentity, email: String) -> Self {
        Self {
            email,
            display_name: identity.display_name.clone(),
            account_id: account.id,
            account_email: account.email.clone(),
            exact: true,
        }
    }

    fn expanded(account: &ComposeAccount, identity: &SenderIdentity, email: String) -> Self {
        Self {
            email,
            display_name: identity.display_name.clone(),
            account_id: account.id,
            account_email: account.email.clone(),
            exact: false,
        }
    }

    fn sender(self) -> ComposeSender {
        ComposeSender {
            email: self.email,
            display_name: self.display_name,
            account_id: self.account_id,
        }
    }

    fn prefer(&self, other: &Candidate) -> bool {
        match (self.exact, other.exact) {
            (true, false) => true,
            (false, true) => false,
            _ => {
                let own = self.account_email.eq_ignore_ascii_case(&self.email);
                let other_own = other.account_email.eq_ignore_ascii_case(&other.email);
                if own != other_own {
                    own
                } else {
                    self.account_id < other.account_id
                }
            }
        }
    }
}

fn consider(chosen: &mut BTreeMap<String, Candidate>, candidate: Candidate) {
    let key = candidate.email.to_ascii_lowercase();
    match chosen.get(&key) {
        Some(current) if !candidate.prefer(current) => {}
        _ => {
            chosen.insert(key, candidate);
        }
    }
}

fn group_tab<'a>(
    tabs: &'a [BusinessTab],
    tim: Option<&'a BusinessTab>,
    views: &[AddressView],
    rules: &[AddressRule],
    candidate: &Candidate,
) -> Option<&'a BusinessTab> {
    if let Some(tim) = tim
        && is_tim_address(Some(tim), &candidate.email)
    {
        return Some(tim);
    }
    let profile = views
        .iter()
        .find(|view| view.address.eq_ignore_ascii_case(&candidate.email))
        .and_then(|view| view.profile_id.clone())
        .or_else(|| classify_receiving_address(rules, &candidate.email));
    profile.and_then(|profile| {
        tabs.iter()
            .find(|tab| !is_tim_tab(tab, tim) && tab.profile_ids.iter().any(|id| id == &profile))
    })
}

fn account_groups(
    accounts: &[ComposeAccount],
    chosen: &BTreeMap<String, Candidate>,
) -> Vec<ComposeGroup> {
    let mut groups = Vec::new();
    for account in accounts {
        let mut senders = chosen
            .values()
            .filter(|candidate| candidate.account_id == account.id)
            .cloned()
            .map(|candidate| candidate.sender())
            .collect::<Vec<_>>();
        if senders.is_empty() {
            continue;
        }
        senders.sort_by(|left, right| left.email.cmp(&right.email));
        groups.push(ComposeGroup {
            id: format!("mailbox-{}", account.id),
            name: account.email.clone(),
            senders,
        });
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SenderDeliveryStatus, SenderIdentitySource};

    fn identity(account: i64, email: &str, status: &str) -> SenderIdentity {
        SenderIdentity {
            account_id: account,
            email: email.into(),
            display_name: None,
            reply_to_email: None,
            is_primary: false,
            is_provider_default: false,
            verification_status: status.into(),
            last_synced_at: 0,
            source: SenderIdentitySource::Provider,
            delivery_status: SenderDeliveryStatus::Unknown,
            delivery_error: None,
        }
    }

    fn view(address: &str, profile: &str) -> AddressView {
        AddressView {
            id: 1,
            address: address.into(),
            source_account_emails: vec!["inbox@mentinkmedia.com".into()],
            profile_id: Some(profile.into()),
            source: Default::default(),
        }
    }

    fn tab(id: &str, name: &str, profiles: &[&str], patterns: &[&str]) -> BusinessTab {
        BusinessTab {
            id: id.into(),
            name: name.into(),
            show_name: true,
            icon: None,
            color: None,
            account_ids: None,
            address_view_ids: None,
            profile_ids: profiles.iter().map(|id| (*id).to_owned()).collect(),
            recipient_patterns: patterns
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
            scope: if patterns.is_empty() {
                "profiles".into()
            } else {
                "recipients".into()
            },
            sender_addresses: Vec::new(),
            sender_names: Vec::new(),
            sender_domains: Vec::new(),
        }
    }

    fn media_account() -> ComposeAccount {
        ComposeAccount {
            id: 7,
            email: "inbox@mentinkmedia.com".into(),
            profile_id: Some("mentink-media".into()),
            identities: vec![
                identity(7, "inbox@mentinkmedia.com", "accepted"),
                identity(7, "*@mentinkmedia.com", "accepted"),
                identity(7, "*@mentinkmemorial.org", "accepted"),
                identity(7, "sales@mentinkmemorial.org", "accepted"),
                identity(7, "pending@mentinkmemorial.org", "pending"),
            ],
        }
    }

    fn fixture() -> (
        Vec<BusinessTab>,
        Vec<AddressView>,
        Vec<AddressRule>,
        Vec<ComposeAccount>,
    ) {
        let tabs = vec![
            tab(
                "tim",
                "Tim Mentink",
                &["tim-personal"],
                &["tim@*", "tim.mentink@*", "tmentink@*"],
            ),
            tab(
                "mentink-media",
                "Mentink Media",
                &["mentink-media", "mentink-memorial"],
                &[],
            ),
        ];
        let views = vec![
            view("tim@mentinkmemorial.org", "mentink-memorial"),
            view("contact@mentinkmemorial.org", "mentink-memorial"),
            view("hello@mentinkmedia.com", "mentink-media"),
            view("stranger@mentinkmemorial.org", "mentink-memorial"),
        ];
        let rules = vec![
            AddressRule {
                profile_id: "mentink-memorial".into(),
                addresses: Vec::new(),
                domains: vec!["mentinkmemorial.org".into()],
            },
            AddressRule {
                profile_id: "mentink-media".into(),
                addresses: Vec::new(),
                domains: vec!["mentinkmedia.com".into()],
            },
        ];
        let mut gmail = identity(3, "tmentink@gmail.com", "accepted");
        gmail.is_primary = true;
        let accounts = vec![
            media_account(),
            ComposeAccount {
                id: 3,
                email: "tmentink@gmail.com".into(),
                profile_id: Some("tim-personal".into()),
                identities: vec![gmail],
            },
        ];
        (tabs, views, rules, accounts)
    }

    fn group<'a>(groups: &'a [ComposeGroup], id: &str) -> &'a ComposeGroup {
        groups.iter().find(|group| group.id == id).expect(id)
    }

    fn emails(group: &ComposeGroup) -> Vec<&str> {
        group
            .senders
            .iter()
            .map(|sender| sender.email.as_str())
            .collect()
    }

    #[test]
    fn tim_addresses_send_from_the_authorizing_mailbox_under_the_tim_tab() {
        let (tabs, views, rules, accounts) = fixture();
        let groups = compose_groups(&tabs, &views, &rules, &accounts, &[], &[]);
        let tim = group(&groups, "tim");
        let media = group(&groups, "mentink-media");
        assert_eq!(tim.name, "Tim Mentink");
        assert_eq!(
            emails(tim),
            vec!["tim@mentinkmemorial.org", "tmentink@gmail.com"]
        );
        assert_eq!(
            transport_account(&groups, "tim@mentinkmemorial.org"),
            Some(7),
            "the Memorial alias is authorized by the shared Media mailbox"
        );
        assert_eq!(
            transport_account(&groups, "tmentink@gmail.com"),
            Some(3),
            "the Gmail address keeps its own account"
        );
        assert!(
            !emails(media).contains(&"tim@mentinkmemorial.org"),
            "Tim sends under the Tim tab, not the company group"
        );
        assert_eq!(
            emails(media),
            vec![
                "contact@mentinkmemorial.org",
                "hello@mentinkmedia.com",
                "inbox@mentinkmedia.com",
                "sales@mentinkmemorial.org",
                "stranger@mentinkmemorial.org",
            ]
        );
        assert_eq!(
            transport_account(&groups, "contact@mentinkmemorial.org"),
            Some(7)
        );
        assert_eq!(
            transport_account(&groups, "sales@mentinkmemorial.org"),
            Some(7)
        );
        assert_eq!(
            transport_account(&groups, "stranger@mentinkmemorial.org"),
            Some(7)
        );
        assert_eq!(
            groups
                .iter()
                .filter(|group| group.name == "Tim Mentink")
                .count(),
            1
        );
        assert!(
            transport_account(&groups, "pending@mentinkmemorial.org").is_none(),
            "an unverified identity is not a From address"
        );
        assert!(
            transport_account(&groups, "invented@mentinkmemorial.org").is_none(),
            "a wildcard does not offer an address the catalog has never seen"
        );
        assert!(transport_account(&groups, "*@mentinkmemorial.org").is_none());
    }

    #[test]
    fn a_customized_tm_tab_keeps_its_name_and_ignores_unrelated_catalog_rows() {
        let (mut tabs, mut views, rules, accounts) = fixture();
        tabs[0].id = "tab-5".into();
        tabs[0].name = "TM".into();
        tabs[0].account_ids = Some(vec![7, 293]);
        tabs[0].recipient_patterns.clear();
        views.push(AddressView {
            id: 99,
            address: "other@elsewhere.example".into(),
            source_account_emails: vec!["storage@elsewhere.example".into()],
            profile_id: Some("pixelated-cities".into()),
            source: Default::default(),
        });
        let profiles = vec![MailProfile {
            id: "mentink-memorial".into(),
            name: "Mentink Memorial".into(),
            color: "#000".into(),
            account_ids: vec![293],
            logo: None,
        }];
        let groups = compose_groups(&tabs, &views, &rules, &accounts, &profiles, &[7, 293]);
        let tim = group(&groups, "tab-5");
        assert_eq!(tim.name, "TM");
        assert!(emails(tim).contains(&"tim@mentinkmemorial.org"));
        assert!(!emails(tim).contains(&"contact@mentinkmemorial.org"));
        assert!(transport_account(&groups, "other@elsewhere.example").is_none());
        assert_eq!(groups.iter().filter(|group| group.id == "tab-5").count(), 1);
        assert!(groups.iter().all(|group| group.name != "Tim Mentink"));
    }

    #[test]
    fn shared_storage_lists_tim_mail_on_tm_and_leaves_other_mail_on_media() {
        let tabs: Vec<BusinessTab> = serde_json::from_value(serde_json::json!([
            {"id":"tab-5","name":"TM","accountIds":[2,293]},
            {"id":"mentink-media","name":"Mentink Media","profileIds":["mentink-media","mentink-memorial"]}
        ]))
        .unwrap();
        let sets = vec![
            TabMailboxSet {
                id: "tab-5".into(),
                mailbox_ids: vec![2, 293],
            },
            TabMailboxSet {
                id: "mentink-media".into(),
                mailbox_ids: vec![4, 12, 293],
            },
        ];
        let views = vec![
            AddressView {
                id: 1,
                address: "tim@mentinkmemorial.org".into(),
                source_account_emails: vec!["tim@mentinkmemorial.org".into()],
                profile_id: Some("mentink-memorial".into()),
                source: Default::default(),
            },
            AddressView {
                id: 2,
                address: "contact@mentinkmemorial.org".into(),
                source_account_emails: vec!["contact@mentinkmemorial.org".into()],
                profile_id: Some("mentink-memorial".into()),
                source: Default::default(),
            },
            AddressView {
                id: 3,
                address: "other@elsewhere.example".into(),
                source_account_emails: vec!["storage@elsewhere.example".into()],
                profile_id: Some("pixelated-cities".into()),
                source: Default::default(),
            },
        ];
        let profiles = vec![
            MailProfile {
                id: "mentink-media".into(),
                name: "Mentink Media".into(),
                color: "#000".into(),
                account_ids: vec![4],
                logo: None,
            },
            MailProfile {
                id: "mentink-memorial".into(),
                name: "Mentink Memorial".into(),
                color: "#000".into(),
                account_ids: vec![12, 293],
                logo: None,
            },
        ];
        let emails = [(293_i64, "storage@mentinkmemorial.org".into())];
        let tim =
            shared_tab_scope(&tabs, &sets, &[2, 293], "tab-5", &views, &profiles, &emails).unwrap();
        assert_eq!(tim.or_account_ids, vec![2]);
        assert_eq!(tim.evidenced_mail[0].account_ids, vec![12, 293]);
        assert_eq!(
            tim.evidenced_mail[0].addresses,
            vec!["tim@mentinkmemorial.org"]
        );
        assert!(tim.remainder_mail.is_none());
        let media = shared_tab_scope(
            &tabs,
            &sets,
            &[2, 293],
            "mentink-media",
            &views,
            &profiles,
            &emails,
        )
        .unwrap();
        assert_eq!(media.or_account_ids, vec![4]);
        let remainder = media.remainder_mail.unwrap();
        assert_eq!(remainder.account_ids, vec![12, 293]);
        assert_eq!(remainder.omit_addresses, vec!["tim@mentinkmemorial.org"]);
        assert_eq!(
            remainder.keep_addresses,
            vec!["contact@mentinkmemorial.org"]
        );
        assert!(tabs.iter().any(|tab| tab.id == "tab-5" && tab.name == "TM"));
        assert_eq!(tabs.len(), 2);
        assert!(
            shared_tab_scope(&tabs, &sets, &[2, 293], "tab-5", &[], &profiles, &emails).is_none(),
            "without a hosted Tim address, shared mail is not hidden"
        );
    }

    #[test]
    fn an_existing_tim_business_name_is_reused_and_not_duplicated() {
        let (mut tabs, views, rules, accounts) = fixture();
        tabs[0].name = "Tim Business".into();
        tabs.insert(
            0,
            tab("tim-copy", "Tim Mentink", &["tim-personal"], &["tim@*"]),
        );
        let groups = compose_groups(&tabs, &views, &rules, &accounts, &[], &[]);
        let tim_groups = groups
            .iter()
            .filter(|group| {
                group.name.eq_ignore_ascii_case("Tim Mentink")
                    || group.name.eq_ignore_ascii_case("Tim Business")
            })
            .map(|group| group.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(tim_groups, vec!["tim-copy"]);
        assert_eq!(
            transport_account(&groups, "tim@mentinkmemorial.org"),
            Some(7)
        );
    }

    #[test]
    fn an_exact_identity_beats_another_accounts_wildcard() {
        let (tabs, views, rules, mut accounts) = fixture();
        accounts[0]
            .identities
            .retain(|identity| identity.email != "sales@mentinkmemorial.org");
        accounts.push(ComposeAccount {
            id: 9,
            email: "other@mentinkmedia.com".into(),
            profile_id: Some("mentink-media".into()),
            identities: vec![identity(9, "sales@mentinkmemorial.org", "accepted")],
        });
        let groups = compose_groups(&tabs, &views, &rules, &accounts, &[], &[]);
        assert_eq!(
            transport_account(&groups, "sales@mentinkmemorial.org"),
            Some(9)
        );
        assert_eq!(
            transport_account(&groups, "contact@mentinkmemorial.org"),
            Some(7)
        );
    }
}
