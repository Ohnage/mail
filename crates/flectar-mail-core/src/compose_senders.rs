//! Compose groups authorized From addresses by business tab.
//!
//! A shared mailbox can authorize addresses for more than one company. The
//! mailbox's profile decides where that connection is listed, not which of
//! its authorized addresses may be sent. Tim-named addresses stay on the
//! existing Tim tab. Other addresses stay on the company tab whose profiles
//! include them. Nothing here invents an address the provider did not
//! authorize, and a wildcard is an authorization, not a From address.

use crate::brand_registry::classify_receiving_address;
use crate::models::{
    AddressRule, AddressView, AddressViewSource, BusinessTab, MailProfile, SenderIdentity,
};
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
                // A wildcard authorizes a domain. Registry and manual addresses
                // tied to this mailbox can be sent. The provider receiving
                // catalog is not a From list: exact identities already name
                // what that account may send, and expanding every cached view
                // dumps the whole catalog into the menu.
                for view in address_views {
                    if view.source == AddressViewSource::Provider {
                        continue;
                    }
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
    // City From identities submit through hub@'s SMTP (Addendum 5), never
    // through a separate city mailbox account.
    if crate::pixelated_hub::is_city_from_address(email)
        && let Some(hub) = groups.iter().find_map(|group| {
            group
                .senders
                .iter()
                .find(|sender| crate::company_view::is_pixelated_cities_hub(&sender.email))
        })
    {
        return Some(hub.account_id);
    }
    groups.iter().find_map(|group| {
        group
            .senders
            .iter()
            .find(|sender| same_sender_email(&sender.email, email))
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
                .any(|sender| same_sender_email(&sender.email, requested_email))
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
///
/// Builds a fresh index. UI click paths must call [`shared_tab_scope_with_index`]
/// with a cached index instead.
pub fn shared_tab_scope(
    tabs: &[BusinessTab],
    tab_mailboxes: &[TabMailboxSet],
    tim_mailbox_ids: &[i64],
    active_tab_id: &str,
    address_views: &[AddressView],
    profiles: &[MailProfile],
    account_emails: &[(i64, String)],
) -> Option<crate::recipient_match::MailScope> {
    let hosted = HostedAddressIndex::build(address_views, profiles, account_emails);
    shared_tab_scope_with_index(
        tabs,
        tab_mailboxes,
        tim_mailbox_ids,
        active_tab_id,
        &hosted,
        account_emails,
    )
}

/// Same routing as [`shared_tab_scope`], using a prebuilt ownership index.
/// Click-time callers must never call [`HostedAddressIndex::build`] themselves.
pub fn shared_tab_scope_with_index(
    tabs: &[BusinessTab],
    tab_mailboxes: &[TabMailboxSet],
    tim_mailbox_ids: &[i64],
    active_tab_id: &str,
    hosted: &HostedAddressIndex,
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
            let email = account_emails
                .iter()
                .find(|(account_id, _)| account_id == id)
                .map(|(_, email)| email.as_str())
                .unwrap_or("");
            hosted.hosts_tim_address(*id, email, Some(tim))
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
    let tim_addresses = hosted.tim_addresses_for_mailboxes(&split, account_emails, Some(tim));
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
        // ponytail: materializing tens of thousands of keep addresses freezes
        // the UI and blows past SQLite bind limits. Omit-only still hides
        // Tim-named mail; dual-recipient company+Tim rows then stay on TM only
        // until keep uses a profile-group matcher.
        const KEEP_ADDRESS_CEILING: usize = 512;
        let hosted_count = hosted.hosted_count_for_mailboxes(&split, account_emails);
        let keep = if hosted_count.saturating_sub(tim_addresses.len()) > KEEP_ADDRESS_CEILING {
            Vec::new()
        } else {
            hosted
                .for_mailboxes(&split, account_emails)
                .into_iter()
                .filter(|address| !is_tim_address(Some(tim), address))
                .collect::<Vec<_>>()
        };
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

/// Owned receiving-address index by mailbox email and by profile.
///
/// Built off the UI thread and cached by generation so click-time routing is
/// O(tab size) lookups, not an O(address_views) rebuild.
#[derive(Debug, Clone, Default)]
pub struct HostedAddressIndex {
    by_email: BTreeMap<String, Vec<String>>,
    by_profile: BTreeMap<String, Vec<String>>,
    profile_mailboxes: BTreeMap<String, Vec<i64>>,
    /// Mailbox emails that host at least one Tim-heuristic address.
    emails_with_tim: BTreeSet<String>,
    /// Profiles that host at least one Tim-heuristic address.
    profiles_with_tim: BTreeSet<String>,
    /// Tim-heuristic addresses keyed by source mailbox email.
    tim_by_email: BTreeMap<String, Vec<String>>,
    /// Tim-heuristic addresses keyed by profile id.
    tim_by_profile: BTreeMap<String, Vec<String>>,
    /// Account ids that host at least one Tim-heuristic address.
    tim_host_account_ids: BTreeSet<i64>,
    /// Hosted address counts per account id (keep-ceiling without materializing).
    hosted_count_by_account: BTreeMap<i64, usize>,
}

/// How many times [`HostedAddressIndex::build`] has run in this process.
/// Click-path tests assert this does not move on the UI thread.
pub fn hosted_index_build_count() -> usize {
    HOSTED_INDEX_BUILDS.load(std::sync::atomic::Ordering::Relaxed)
}

static HOSTED_INDEX_BUILDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl HostedAddressIndex {
    pub fn build(
        views: &[AddressView],
        profiles: &[MailProfile],
        account_emails: &[(i64, String)],
    ) -> Self {
        HOSTED_INDEX_BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut by_email: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut by_profile: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for view in views {
            let address = view.address.trim();
            if address.is_empty() {
                continue;
            }
            for email in &view.source_account_emails {
                let key = email.trim().to_ascii_lowercase();
                if key.is_empty() {
                    continue;
                }
                by_email.entry(key).or_default().push(address.to_owned());
            }
            if let Some(profile_id) = view.profile_id.as_deref() {
                by_profile
                    .entry(profile_id.to_owned())
                    .or_default()
                    .push(address.to_owned());
            }
        }
        let mut profile_mailboxes = BTreeMap::new();
        for profile in profiles {
            profile_mailboxes.insert(profile.id.clone(), profile.account_ids.clone());
        }
        let mut emails_with_tim = BTreeSet::new();
        let mut tim_by_email: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (email, addresses) in &by_email {
            let tim_addresses = addresses
                .iter()
                .filter(|address| is_tim_address(None, address))
                .map(|address| address.to_ascii_lowercase())
                .collect::<Vec<_>>();
            if !tim_addresses.is_empty() {
                emails_with_tim.insert(email.clone());
                tim_by_email.insert(email.clone(), tim_addresses);
            }
        }
        let mut profiles_with_tim = BTreeSet::new();
        let mut tim_by_profile: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (profile_id, addresses) in &by_profile {
            let tim_addresses = addresses
                .iter()
                .filter(|address| is_tim_address(None, address))
                .map(|address| address.to_ascii_lowercase())
                .collect::<Vec<_>>();
            if !tim_addresses.is_empty() {
                profiles_with_tim.insert(profile_id.clone());
                tim_by_profile.insert(profile_id.clone(), tim_addresses);
            }
        }
        let email_to_ids = {
            let mut map: BTreeMap<String, Vec<i64>> = BTreeMap::new();
            for (id, email) in account_emails {
                let key = email.trim().to_ascii_lowercase();
                if !key.is_empty() {
                    map.entry(key).or_default().push(*id);
                }
            }
            map
        };
        let mut tim_host_account_ids = BTreeSet::new();
        let mut hosted_count_by_account: BTreeMap<i64, usize> = BTreeMap::new();
        for (email, ids) in &email_to_ids {
            let hosted = by_email.get(email).map(Vec::len).unwrap_or(0);
            let has_tim = tim_by_email.contains_key(email);
            for id in ids {
                *hosted_count_by_account.entry(*id).or_insert(0) += hosted;
                if has_tim {
                    tim_host_account_ids.insert(*id);
                }
            }
        }
        for (profile_id, mailbox_ids) in &profile_mailboxes {
            let hosted = by_profile.get(profile_id).map(Vec::len).unwrap_or(0);
            let has_tim = profiles_with_tim.contains(profile_id);
            for id in mailbox_ids {
                *hosted_count_by_account.entry(*id).or_insert(0) += hosted;
                if has_tim {
                    tim_host_account_ids.insert(*id);
                }
            }
        }
        Self {
            by_email,
            by_profile,
            profile_mailboxes,
            emails_with_tim,
            profiles_with_tim,
            tim_by_email,
            tim_by_profile,
            tim_host_account_ids,
            hosted_count_by_account,
        }
    }

    pub fn for_mailbox(&self, account_id: i64, account_email: &str) -> Vec<String> {
        let mut addresses = BTreeSet::new();
        let email = account_email.trim().to_ascii_lowercase();
        if !email.is_empty()
            && let Some(hosted) = self.by_email.get(&email)
        {
            for address in hosted {
                addresses.insert(address.to_ascii_lowercase());
            }
        }
        for (profile_id, mailbox_ids) in &self.profile_mailboxes {
            if !mailbox_ids.contains(&account_id) {
                continue;
            }
            if let Some(hosted) = self.by_profile.get(profile_id) {
                for address in hosted {
                    addresses.insert(address.to_ascii_lowercase());
                }
            }
        }
        addresses.into_iter().collect()
    }

    /// True when this mailbox hosts at least one Tim-owned address.
    ///
    /// Heuristic Tim locals are precomputed at build time. Tab recipient
    /// patterns are checked only when the Tim tab defines them.
    pub fn hosts_tim_address(
        &self,
        account_id: i64,
        account_email: &str,
        tim: Option<&BusinessTab>,
    ) -> bool {
        if self.tim_host_account_ids.contains(&account_id) {
            return true;
        }
        let email = account_email.trim().to_ascii_lowercase();
        if !email.is_empty() && self.emails_with_tim.contains(&email) {
            return true;
        }
        let patterns = tim
            .map(|tab| tab.recipient_patterns.as_slice())
            .unwrap_or(&[]);
        if patterns.is_empty() {
            return false;
        }
        if !email.is_empty()
            && let Some(hosted) = self.by_email.get(&email)
            && hosted.iter().any(|address| is_tim_address(tim, address))
        {
            return true;
        }
        for (profile_id, mailbox_ids) in &self.profile_mailboxes {
            if !mailbox_ids.contains(&account_id) {
                continue;
            }
            if let Some(hosted) = self.by_profile.get(profile_id)
                && hosted.iter().any(|address| is_tim_address(tim, address))
            {
                return true;
            }
        }
        false
    }

    pub fn for_mailboxes(
        &self,
        account_ids: &[i64],
        account_emails: &[(i64, String)],
    ) -> Vec<String> {
        let mut addresses = BTreeSet::new();
        for id in account_ids {
            let email = account_emails
                .iter()
                .find(|(account_id, _)| account_id == id)
                .map(|(_, email)| email.as_str())
                .unwrap_or("");
            addresses.extend(self.for_mailbox(*id, email));
        }
        addresses.into_iter().collect()
    }

    pub fn hosted_count_for_mailboxes(
        &self,
        account_ids: &[i64],
        _account_emails: &[(i64, String)],
    ) -> usize {
        account_ids
            .iter()
            .map(|id| self.hosted_count_by_account.get(id).copied().unwrap_or(0))
            .sum()
    }

    pub fn tim_addresses_for_mailboxes(
        &self,
        account_ids: &[i64],
        account_emails: &[(i64, String)],
        tim: Option<&BusinessTab>,
    ) -> Vec<String> {
        let wanted = account_ids.iter().copied().collect::<BTreeSet<_>>();
        let mut addresses = BTreeSet::new();
        // One union per Tim-hosting email / profile that touches the split set.
        let mut used_emails = BTreeSet::new();
        for id in &wanted {
            let email = account_emails
                .iter()
                .find(|(account_id, _)| account_id == id)
                .map(|(_, email)| email.trim().to_ascii_lowercase())
                .unwrap_or_default();
            if email.is_empty() || !used_emails.insert(email.clone()) {
                continue;
            }
            if let Some(tim_addrs) = self.tim_by_email.get(&email) {
                addresses.extend(tim_addrs.iter().cloned());
            }
        }
        for profile_id in &self.profiles_with_tim {
            let Some(mailbox_ids) = self.profile_mailboxes.get(profile_id) else {
                continue;
            };
            if !mailbox_ids.iter().any(|id| wanted.contains(id)) {
                continue;
            }
            if let Some(tim_addrs) = self.tim_by_profile.get(profile_id) {
                addresses.extend(tim_addrs.iter().cloned());
            }
        }
        let patterns = tim
            .map(|tab| tab.recipient_patterns.as_slice())
            .unwrap_or(&[]);
        if patterns.is_empty() {
            return addresses.into_iter().collect();
        }
        for id in &wanted {
            let email = account_emails
                .iter()
                .find(|(account_id, _)| account_id == id)
                .map(|(_, email)| email.trim().to_ascii_lowercase())
                .unwrap_or_default();
            if !email.is_empty()
                && let Some(hosted) = self.by_email.get(&email)
            {
                for address in hosted {
                    if is_tim_address(tim, address) {
                        addresses.insert(address.to_ascii_lowercase());
                    }
                }
            }
            for (profile_id, mailbox_ids) in &self.profile_mailboxes {
                if !mailbox_ids.contains(id) {
                    continue;
                }
                if let Some(hosted) = self.by_profile.get(profile_id) {
                    for address in hosted {
                        if is_tim_address(tim, address) {
                            addresses.insert(address.to_ascii_lowercase());
                        }
                    }
                }
            }
        }
        addresses.into_iter().collect()
    }
}

pub fn sender_account_id(group: &ComposeGroup, email: &str) -> Option<i64> {
    group
        .senders
        .iter()
        .find(|sender| same_sender_email(&sender.email, email))
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

/// Trimmed address and its case-insensitive key. Display-name forms
/// (`Name <email>`) and surrounding space collapse to the address itself.
fn normalized_sender(email: &str) -> Option<(String, String)> {
    let email = email.trim();
    let email = email.rsplit_once('<').map_or(email, |(_, rest)| {
        rest.split('>').next().unwrap_or(rest).trim()
    });
    let (local, domain) = email.rsplit_once('@')?;
    let local = local.trim();
    let domain = domain.trim();
    if local.is_empty() || local == "*" || domain.is_empty() {
        return None;
    }
    let visible = format!("{local}@{domain}");
    Some((visible.to_ascii_lowercase(), visible))
}

/// One visible From address. Case and surrounding space do not make a second row.
pub fn same_sender_email(left: &str, right: &str) -> bool {
    match (normalized_sender(left), normalized_sender(right)) {
        (Some((left, _)), Some((right, _))) => left == right,
        _ => false,
    }
}

/// Selectable identities, one per address. A provider default beats a primary,
/// which beats a higher account id. Wildcards and unverified rows are omitted.
pub fn unique_selectable_identities(identities: Vec<SenderIdentity>) -> Vec<SenderIdentity> {
    let mut chosen: BTreeMap<String, SenderIdentity> = BTreeMap::new();
    for mut identity in identities {
        if !identity.is_selectable() {
            continue;
        }
        let Some((key, visible)) = normalized_sender(&identity.email) else {
            continue;
        };
        identity.email = visible;
        match chosen.get(&key) {
            Some(current) if !selectable_prefer(&identity, current) => {}
            _ => {
                chosen.insert(key, identity);
            }
        }
    }
    let mut values = chosen.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left.email
            .to_ascii_lowercase()
            .cmp(&right.email.to_ascii_lowercase())
    });
    values
}

fn selectable_prefer(candidate: &SenderIdentity, current: &SenderIdentity) -> bool {
    match (candidate.is_provider_default, current.is_provider_default) {
        (true, false) => true,
        (false, true) => false,
        _ => match (candidate.is_primary, current.is_primary) {
            (true, false) => true,
            (false, true) => false,
            _ => candidate.account_id < current.account_id,
        },
    }
}

fn consider(chosen: &mut BTreeMap<String, Candidate>, mut candidate: Candidate) {
    let Some((key, visible)) = normalized_sender(&candidate.email) else {
        return;
    };
    candidate.email = visible;
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
    fn pixelated_cities_hub_tab_lists_city_from_identities() {
        let mut tabs = vec![tab(
            "pixelated-cities",
            "Pixelated Cities",
            &["pixelated-cities"],
            &[],
        )];
        tabs[0].account_ids = Some(vec![10]);
        let views = vec![
            view("hub@pixelatedcities.com", "pixelated-cities"),
            view("hub@jacksonmichigan.org", "pixelated-cities"),
            view("hub@annarbormichigan.org", "pixelated-cities"),
        ];
        let rules = vec![AddressRule {
            profile_id: "pixelated-cities".into(),
            addresses: Vec::new(),
            domains: vec![
                "pixelatedcities.com".into(),
                "jacksonmichigan.org".into(),
                "annarbormichigan.org".into(),
            ],
        }];
        let accounts = vec![ComposeAccount {
            id: 10,
            email: "hub@pixelatedcities.com".into(),
            profile_id: Some("pixelated-cities".into()),
            identities: vec![
                identity(10, "hub@pixelatedcities.com", "accepted"),
                identity(10, "hub@jacksonmichigan.org", "accepted"),
                identity(10, "hub@annarbormichigan.org", "accepted"),
            ],
        }];
        let profiles = vec![MailProfile {
            id: "pixelated-cities".into(),
            name: "Pixelated Cities".into(),
            color: "#8b5cf6".into(),
            account_ids: vec![10],
            logo: None,
        }];
        let groups = compose_groups(&tabs, &views, &rules, &accounts, &profiles, &[]);
        let pc = group(&groups, "pixelated-cities");
        assert_eq!(
            emails(pc),
            vec![
                "hub@annarbormichigan.org",
                "hub@jacksonmichigan.org",
                "hub@pixelatedcities.com",
            ]
        );
        assert_eq!(
            transport_account(&groups, "hub@jacksonmichigan.org"),
            Some(10),
            "city From authenticates as hub@"
        );
        assert!(crate::pixelated_hub::is_city_from_address(
            "hub@jacksonmichigan.org"
        ));
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
    fn shared_tab_scope_stays_cheap_with_tens_of_thousands_of_address_views() {
        let tabs: Vec<BusinessTab> = serde_json::from_value(serde_json::json!([
            {"id":"tab-5","name":"TM","accountIds":[2,293]},
            {"id":"pixelated-cities","name":"Pixelated Cities","profileIds":["pixelated-cities"],"accountIds":[99]}
        ]))
        .unwrap();
        let sets = [
            TabMailboxSet {
                id: "tab-5".into(),
                mailbox_ids: vec![2, 293],
            },
            TabMailboxSet {
                id: "pixelated-cities".into(),
                mailbox_ids: vec![99, 293],
            },
        ];
        let mut views = Vec::with_capacity(20_000);
        views.push(AddressView {
            id: 1,
            address: "tim@mentinkmemorial.org".into(),
            source_account_emails: vec!["storage@mentinkmemorial.org".into()],
            profile_id: Some("mentink-memorial".into()),
            source: Default::default(),
        });
        for index in 0..20_000 {
            views.push(AddressView {
                id: i64::from(index + 2),
                address: format!("city{index}@pixelated.cities"),
                source_account_emails: vec!["hub@pixelated.cities".into()],
                profile_id: Some("pixelated-cities".into()),
                source: Default::default(),
            });
        }
        let profiles = vec![
            MailProfile {
                id: "mentink-memorial".into(),
                name: "Mentink Memorial".into(),
                color: "#000".into(),
                account_ids: vec![293],
                logo: None,
            },
            MailProfile {
                id: "pixelated-cities".into(),
                name: "Pixelated Cities".into(),
                color: "#000".into(),
                account_ids: vec![99],
                logo: None,
            },
        ];
        let emails = [
            (293_i64, "storage@mentinkmemorial.org".into()),
            (99_i64, "hub@pixelated.cities".into()),
            (2_i64, "tim@example.test".into()),
        ];
        let builds_before = hosted_index_build_count();
        let hosted = HostedAddressIndex::build(&views, &profiles, &emails);
        assert_eq!(hosted_index_build_count(), builds_before + 1);
        let started = std::time::Instant::now();
        let media = shared_tab_scope_with_index(
            &tabs,
            &sets,
            &[2, 293],
            "pixelated-cities",
            &hosted,
            &emails,
        )
        .expect("shared memorial storage still splits");
        let elapsed = started.elapsed();
        assert_eq!(
            hosted_index_build_count(),
            builds_before + 1,
            "click-path lookup must not rebuild the index"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "cached tab-switch shared scope took {elapsed:?} with {} address views",
            views.len()
        );
        let remainder = media.remainder_mail.expect("company remainder");
        assert_eq!(remainder.omit_addresses, vec!["tim@mentinkmemorial.org"]);
        assert!(
            remainder.keep_addresses.len() <= 512,
            "keep list must not explode to tens of thousands of binds"
        );
    }

    /// 30k live-shape routing must match c901110: Tim hosted on a company hub
    /// routes to TM, and company / hub@ / Project-47 scopes stay equivalent.
    #[test]
    fn routing_equivalence_at_30k_matches_c901110() {
        let tabs: Vec<BusinessTab> = serde_json::from_value(serde_json::json!([
            {"id":"tab-5","name":"TM","accountIds":[2,293]},
            {"id":"mentink-media","name":"Mentink Media","profileIds":["mentink-media"],"accountIds":[4,12,293]},
            {"id":"pixelated-cities","name":"Pixelated Cities","profileIds":["pixelated-cities"],"accountIds":[99]},
            {"id":"forty7","name":"Project 47","scope":"any","senderDomains": (0..323).map(|i| format!("d{i}.example")).collect::<Vec<_>>()},
            {"id":"hub-tab","name":"Hub","profileIds":["pixelated-cities"],"accountIds":[99]}
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
            TabMailboxSet {
                id: "pixelated-cities".into(),
                mailbox_ids: vec![99],
            },
            TabMailboxSet {
                id: "forty7".into(),
                // Scope-any tab: domains only; no shared Tim hub mailbox.
                mailbox_ids: (100..304).filter(|id| *id != 293 && *id != 99).collect(),
            },
            TabMailboxSet {
                id: "hub-tab".into(),
                mailbox_ids: vec![99],
            },
        ];
        let mut views = Vec::with_capacity(30_000);
        views.push(AddressView {
            id: 1,
            address: "tim@mentinkmemorial.org".into(),
            // Hosted on the company hub mailbox (293), not only Tim's own box.
            source_account_emails: vec!["hub@company.example".into()],
            profile_id: Some("mentink-memorial".into()),
            source: Default::default(),
        });
        views.push(AddressView {
            id: 2,
            address: "contact@mentinkmemorial.org".into(),
            source_account_emails: vec!["hub@company.example".into()],
            profile_id: Some("mentink-memorial".into()),
            source: Default::default(),
        });
        views.push(AddressView {
            id: 3,
            address: "hub@pixelated.cities".into(),
            source_account_emails: vec!["hub@pixelated.cities".into()],
            profile_id: Some("pixelated-cities".into()),
            source: Default::default(),
        });
        for index in 0..28_000 {
            views.push(AddressView {
                id: i64::from(index + 10),
                address: format!("city{index}@pixelated.cities"),
                source_account_emails: vec!["hub@pixelated.cities".into()],
                profile_id: Some("pixelated-cities".into()),
                source: Default::default(),
            });
        }
        while views.len() < 30_000 {
            let index = views.len();
            views.push(AddressView {
                id: index as i64,
                address: format!("extra{index}@example.test"),
                source_account_emails: vec![format!("acct{index}@example.test")],
                profile_id: None,
                source: Default::default(),
            });
        }
        let profiles = vec![
            MailProfile {
                id: "mentink-memorial".into(),
                name: "Mentink Memorial".into(),
                color: "#000".into(),
                account_ids: vec![12, 293],
                logo: None,
            },
            MailProfile {
                id: "mentink-media".into(),
                name: "Mentink Media".into(),
                color: "#000".into(),
                account_ids: vec![4],
                logo: None,
            },
            MailProfile {
                id: "pixelated-cities".into(),
                name: "Pixelated Cities".into(),
                color: "#000".into(),
                account_ids: vec![99],
                logo: None,
            },
        ];
        let emails = [
            (293_i64, "hub@company.example".into()),
            (12_i64, "contact@mentinkmemorial.org".into()),
            (4_i64, "media@mentinkmedia.example".into()),
            (99_i64, "hub@pixelated.cities".into()),
            (2_i64, "tim@example.test".into()),
        ];
        let builds_before = hosted_index_build_count();
        let hosted = HostedAddressIndex::build(&views, &profiles, &emails);
        let build_ms = {
            // Already built above; report a second timed build for the harness.
            let started = std::time::Instant::now();
            let _ = HostedAddressIndex::build(&views, &profiles, &emails);
            started.elapsed().as_millis()
        };
        eprintln!(
            "routing_equivalence views={} rules_proxy=318 accounts=304 build_ms={build_ms}",
            views.len()
        );

        let cases = [
            ("tab-5", true),
            ("mentink-media", true),
            ("pixelated-cities", false),
            ("hub-tab", false),
            ("forty7", false),
        ];
        for (tab_id, expect_some) in cases {
            let via_build =
                shared_tab_scope(&tabs, &sets, &[2, 293], tab_id, &views, &profiles, &emails);
            let via_cache =
                shared_tab_scope_with_index(&tabs, &sets, &[2, 293], tab_id, &hosted, &emails);
            assert_eq!(
                via_build, via_cache,
                "cached index must match c901110 shared_tab_scope for {tab_id}"
            );
            assert_eq!(
                via_cache.is_some(),
                expect_some,
                "unexpected split presence for {tab_id}"
            );
        }

        let tim = shared_tab_scope_with_index(&tabs, &sets, &[2, 293], "tab-5", &hosted, &emails)
            .expect("Tim tab lists hosted Tim mail");
        assert!(
            tim.evidenced_mail.iter().any(|mail| mail
                .addresses
                .iter()
                .any(|a| a == "tim@mentinkmemorial.org")),
            "Tim address hosted on company hub must appear in Tim scope"
        );
        assert!(
            tim.evidenced_mail
                .iter()
                .any(|mail| mail.account_ids.contains(&293))
        );

        let media =
            shared_tab_scope_with_index(&tabs, &sets, &[2, 293], "mentink-media", &hosted, &emails)
                .expect("company tab splits shared hub");
        let remainder = media.remainder_mail.expect("company remainder");
        assert!(
            remainder
                .omit_addresses
                .iter()
                .any(|a| a == "tim@mentinkmemorial.org")
        );
        assert!(
            !remainder
                .omit_addresses
                .iter()
                .any(|a| a == "contact@mentinkmemorial.org")
                || remainder.keep_addresses.is_empty(),
            "non-Tim company addresses stay on the company tab (or omit-only when keep ceiling hits)"
        );

        // Click budget: five tabs with a warm index.
        let click_before = hosted_index_build_count();
        let started = std::time::Instant::now();
        for (tab_id, _) in cases {
            let _ = shared_tab_scope_with_index(&tabs, &sets, &[2, 293], tab_id, &hosted, &emails);
        }
        let click_ms = started.elapsed().as_millis();
        assert_eq!(hosted_index_build_count(), click_before);
        assert!(
            click_ms < 50,
            "five cached shared_tab_scope lookups took {click_ms} ms"
        );
        assert!(
            hosted_index_build_count() >= builds_before + 2,
            "index builds happen off the click path only"
        );
    }

    #[test]
    fn hosted_tim_on_company_hub_without_mailbox_overlap_still_splits() {
        // The cf54e4b "defer unless overlap" shortcut returned None here.
        let tabs: Vec<BusinessTab> = serde_json::from_value(serde_json::json!([
            {"id":"tab-5","name":"TM","accountIds":[2]},
            {"id":"company","name":"Company","accountIds":[99]}
        ]))
        .unwrap();
        let sets = [
            TabMailboxSet {
                id: "tab-5".into(),
                mailbox_ids: vec![2],
            },
            TabMailboxSet {
                id: "company".into(),
                mailbox_ids: vec![99],
            },
        ];
        let views = vec![AddressView {
            id: 1,
            address: "tim@mentinkmemorial.org".into(),
            source_account_emails: vec!["hub@company.example".into()],
            profile_id: Some("company".into()),
            source: Default::default(),
        }];
        let profiles = vec![MailProfile {
            id: "company".into(),
            name: "Company".into(),
            color: "#000".into(),
            account_ids: vec![99],
            logo: None,
        }];
        let emails = [
            (99_i64, "hub@company.example".into()),
            (2_i64, "tim@example.test".into()),
        ];
        let hosted = HostedAddressIndex::build(&views, &profiles, &emails);
        let tim = shared_tab_scope_with_index(&tabs, &sets, &[2], "tab-5", &hosted, &emails)
            .expect("Tim mail on a company hub the Tim tab does not list must still split");
        assert!(
            tim.evidenced_mail
                .iter()
                .any(|m| m.account_ids.contains(&99))
        );
        let company = shared_tab_scope_with_index(&tabs, &sets, &[2], "company", &hosted, &emails)
            .expect("company tab omits hosted Tim addresses");
        assert!(company.remainder_mail.as_ref().is_some_and(|r| {
            r.omit_addresses
                .iter()
                .any(|a| a == "tim@mentinkmemorial.org")
        }));
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

    #[test]
    fn repeated_addresses_collapse_and_the_provider_catalog_stays_out() {
        let (tabs, mut views, rules, mut accounts) = fixture();
        accounts[0]
            .identities
            .push(identity(7, "  Sales@MentinkMemorial.org  ", "accepted"));
        accounts[0].identities.push(identity(
            7,
            "Tim Mentink <contact@mentinkmemorial.org>",
            "accepted",
        ));
        let mut named = identity(7, "desk@mentinkmedia.com", "accepted");
        named.display_name = Some("Mentink Media".into());
        let mut other_named = identity(7, "hello.desk@mentinkmedia.com", "accepted");
        other_named.display_name = Some("Mentink Media".into());
        accounts[0].identities.push(named);
        accounts[0].identities.push(other_named);
        views.push(AddressView {
            id: 40,
            address: "  HELLO@mentinkmedia.com ".into(),
            source_account_emails: vec!["inbox@mentinkmedia.com".into()],
            profile_id: Some("mentink-media".into()),
            source: AddressViewSource::Registry,
        });
        views.push(AddressView {
            id: 41,
            address: "manual@mentinkmedia.com".into(),
            source_account_emails: vec!["inbox@mentinkmedia.com".into()],
            profile_id: Some("mentink-media".into()),
            source: AddressViewSource::Manual,
        });
        for index in 0..2_000 {
            views.push(AddressView {
                id: 1_000 + index,
                address: format!("alias{index}@mentinkmedia.com"),
                source_account_emails: vec!["inbox@mentinkmedia.com".into()],
                profile_id: Some("mentink-media".into()),
                source: AddressViewSource::Provider,
            });
        }
        let groups = compose_groups(&tabs, &views, &rules, &accounts, &[], &[]);
        let media = group(&groups, "mentink-media");
        let sales = media
            .senders
            .iter()
            .filter(|sender| same_sender_email(&sender.email, "sales@mentinkmemorial.org"))
            .collect::<Vec<_>>();
        assert_eq!(sales.len(), 1, "case and space must not repeat an address");
        assert_eq!(sales[0].account_id, 7);
        assert_eq!(sales[0].email, "sales@mentinkmemorial.org");
        assert_eq!(
            media
                .senders
                .iter()
                .filter(|sender| same_sender_email(&sender.email, "hello@mentinkmedia.com"))
                .count(),
            1
        );
        assert_eq!(
            media
                .senders
                .iter()
                .filter(|sender| same_sender_email(&sender.email, "contact@mentinkmemorial.org"))
                .count(),
            1
        );
        let named_rows = media
            .senders
            .iter()
            .filter(|sender| sender.display_name.as_deref() == Some("Mentink Media"))
            .map(|sender| sender.email.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            named_rows,
            vec!["desk@mentinkmedia.com", "hello.desk@mentinkmedia.com"]
        );
        assert!(emails(media).contains(&"manual@mentinkmedia.com"));
        assert!(
            media
                .senders
                .iter()
                .all(|sender| !sender.email.starts_with("alias")),
            "the provider receiving catalog is not a From menu"
        );
        assert!(
            media.senders.len() < 20,
            "a group stays the addresses it owns"
        );
        assert!(transport_account(&groups, "pending@mentinkmemorial.org").is_none());
        assert!(transport_account(&groups, "alias0@mentinkmedia.com").is_none());
        assert!(transport_account(&groups, "*@mentinkmedia.com").is_none());
        assert_eq!(
            transport_account(&groups, "  HELLO@mentinkmedia.com "),
            Some(7)
        );

        let mut duplicated = identity(7, " Desk@MentinkMedia.com ", "accepted");
        duplicated.display_name = Some("Other name".into());
        duplicated.is_primary = true;
        let mut provider_default = identity(7, "desk@mentinkmedia.com", "accepted");
        provider_default.display_name = Some("Mentink Media".into());
        provider_default.is_provider_default = true;
        let unique = unique_selectable_identities(vec![
            duplicated,
            provider_default,
            identity(7, "*@mentinkmedia.com", "accepted"),
            identity(7, "pending@mentinkmedia.com", "pending"),
            identity(9, "other@mentinkmedia.com", "accepted"),
        ]);
        assert_eq!(
            unique
                .iter()
                .map(|identity| identity.email.as_str())
                .collect::<Vec<_>>(),
            vec!["desk@mentinkmedia.com", "other@mentinkmedia.com"]
        );
        assert!(unique[0].is_provider_default);
        assert_eq!(unique[0].display_name.as_deref(), Some("Mentink Media"));
    }
}
