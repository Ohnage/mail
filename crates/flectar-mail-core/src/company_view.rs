//! Combined company mailbox for one business tab.
//!
//! A tab's mailboxes stay separate on the server. The company view matches
//! folders by their full path and tags by their keyword, then asks each
//! message's own account to carry out the change.

use std::collections::{BTreeMap, HashMap, HashSet};

/// Local part of an address, without the domain. Empty when there is no `@`.
pub fn address_local_part(email: &str) -> &str {
    let email = email.trim();
    match email.rsplit_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => local,
        _ => "",
    }
}

/// Whether the address's local part is `hub`. Hub mailboxes hold their own mail.
pub fn is_hub_address(email: &str) -> bool {
    address_local_part(email).eq_ignore_ascii_case("hub")
}

/// Only `inbox@` starts excluded. Every other address, including `hub@`, starts included.
pub fn default_excluded_from_company_views(email: &str) -> bool {
    address_local_part(email).eq_ignore_ascii_case("inbox")
}

/// Whether this mailbox feeds Company Inbox, merged folders, and tab tags.
///
/// `explicit` is the stored `companyViewExcluded` flag. `None` uses the address
/// default. The flag wins in both directions, so `hub@` can be left out and
/// `inbox@` can be brought in.
pub fn excluded_from_company_views(email: &str, explicit: Option<bool>) -> bool {
    explicit.unwrap_or_else(|| default_excluded_from_company_views(email))
}

/// Case-folded full folder path.
///
/// Matching ignores case. JMAP stores a level as ` / `; that is the same
/// separator as `/`, so `Clients / Acme` and `Clients/Acme` are one folder.
pub fn folder_match_key(path: &str) -> String {
    path.trim().replace(" / ", "/").to_lowercase()
}

/// Positive id that fits in a sidebar `int` and stays clear of real folder ids.
pub fn stable_id(text: &str) -> i64 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    i64::from((hash % 1_000_000_000) + 1_000_000_000)
}

pub fn folder_scope_id(path: &str) -> i64 {
    stable_id(&format!("folder:{}", folder_match_key(path)))
}

pub fn tag_scope_id(keyword: &str) -> i64 {
    stable_id(&format!("tag:{keyword}"))
}

/// Leaf shown for a `/`-separated folder path.
pub fn folder_leaf(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(_, leaf)| leaf.trim())
        .filter(|leaf| !leaf.is_empty())
        .unwrap_or(path.trim())
}

/// One account's folder that can join the merged tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderLeaf {
    pub account_id: i64,
    pub folder_id: i64,
    pub path: String,
    pub unread: i64,
}

/// Folders that share a path, in tree order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedFolder {
    pub id: i64,
    pub path: String,
    pub label: String,
    pub depth: i32,
    pub parent_id: i64,
    pub has_children: bool,
    pub unread: i64,
    pub members: Vec<(i64, i64)>,
}

/// Combine folders by full path. Names match without regard to case.
///
/// The first path in input order keeps its spelling. A child hangs under the
/// merged folder whose path is the parent prefix before the last `/`.
/// Grouping is by path, so hundreds of mailboxes that share a tree stay one
/// tree rather than one row per mailbox.
pub fn merge_folders(folders: &[FolderLeaf]) -> Vec<MergedFolder> {
    let mut groups: BTreeMap<String, Vec<&FolderLeaf>> = BTreeMap::new();
    for folder in folders {
        let key = folder_match_key(&folder.path);
        if key.is_empty() {
            continue;
        }
        groups.entry(key).or_default().push(folder);
    }
    let mut id_by_key: HashMap<&str, i64> = HashMap::with_capacity(groups.len());
    for key in groups.keys() {
        id_by_key.insert(key.as_str(), folder_scope_id(key));
    }
    let mut child_keys: HashMap<&str, Vec<&str>> = HashMap::new();
    for key in groups.keys() {
        if let Some((parent, _)) = key.rsplit_once('/')
            && groups.contains_key(parent)
        {
            child_keys.entry(parent).or_default().push(key.as_str());
        }
    }
    let mut nodes = Vec::with_capacity(groups.len());
    for (key, members) in &groups {
        let parent_id = key
            .rsplit_once('/')
            .and_then(|(parent, _)| id_by_key.get(parent).copied())
            .unwrap_or(-1);
        let path = members[0].path.clone();
        let label = folder_leaf(&path).to_owned();
        nodes.push(MergedFolder {
            id: id_by_key[key.as_str()],
            path,
            label,
            depth: 0,
            parent_id,
            has_children: child_keys.contains_key(key.as_str()),
            unread: members.iter().map(|member| member.unread.max(0)).sum(),
            members: members
                .iter()
                .map(|member| (member.account_id, member.folder_id))
                .collect(),
        });
    }
    let mut by_parent: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        by_parent.entry(node.parent_id).or_default().push(index);
    }
    for children in by_parent.values_mut() {
        children.sort_by(|&left, &right| {
            nodes[left]
                .label
                .to_lowercase()
                .cmp(&nodes[right].label.to_lowercase())
                .then(nodes[left].id.cmp(&nodes[right].id))
        });
    }
    let mut ordered = Vec::with_capacity(nodes.len());
    let mut placed = HashMap::<i64, ()>::with_capacity(nodes.len());
    fn walk(
        nodes: &[MergedFolder],
        by_parent: &HashMap<i64, Vec<usize>>,
        parent: i64,
        depth: i32,
        placed: &mut HashMap<i64, ()>,
        out: &mut Vec<MergedFolder>,
    ) {
        let Some(children) = by_parent.get(&parent) else {
            return;
        };
        for &index in children {
            let id = nodes[index].id;
            if placed.insert(id, ()).is_some() {
                continue;
            }
            let mut child = nodes[index].clone();
            child.depth = depth;
            out.push(child);
            walk(nodes, by_parent, id, depth.saturating_add(1), placed, out);
        }
    }
    walk(&nodes, &by_parent, -1, 0, &mut placed, &mut ordered);
    // A cycle has no root. Those folders still appear, after the tree.
    let mut orphans = nodes
        .into_iter()
        .filter(|node| !placed.contains_key(&node.id))
        .collect::<Vec<_>>();
    orphans.sort_by_key(|node| node.label.to_lowercase());
    ordered.extend(orphans);
    ordered
}

/// The folder on `account_id` with this path, if that account already has it.
pub fn folder_for_account(merged: &MergedFolder, account_id: i64) -> Option<i64> {
    merged
        .members
        .iter()
        .find(|(owner, _)| *owner == account_id)
        .map(|(_, folder_id)| *folder_id)
}

/// One account's copy of a tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagCopy {
    pub account_id: i64,
    pub label_id: i64,
    pub keyword: String,
    pub name: String,
    pub color: String,
}

/// Tags that share a keyword, in keyword order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedTag {
    pub id: i64,
    pub keyword: String,
    pub name: String,
    pub color: String,
    pub members: Vec<TagCopy>,
}

pub fn merge_tags(tags: &[TagCopy]) -> Vec<MergedTag> {
    let mut groups: BTreeMap<&str, Vec<&TagCopy>> = BTreeMap::new();
    for tag in tags {
        let keyword = tag.keyword.trim();
        if keyword.is_empty() {
            continue;
        }
        groups.entry(keyword).or_default().push(tag);
    }
    groups
        .into_iter()
        .map(|(keyword, members)| MergedTag {
            id: tag_scope_id(keyword),
            keyword: keyword.to_owned(),
            name: members[0].name.clone(),
            color: members[0].color.clone(),
            members: members.into_iter().cloned().collect(),
        })
        .collect()
}

/// Where a tab tag lands on one message's account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagTarget {
    /// The account already has this keyword.
    Label(i64),
    /// Create the account's copy, then apply that copy.
    Create {
        name: String,
        color: String,
        keyword: String,
    },
}

pub fn tag_target_for_account(
    tags: &[TagCopy],
    account_id: i64,
    keyword: &str,
    name: &str,
    color: &str,
) -> TagTarget {
    if let Some(existing) = tags
        .iter()
        .find(|tag| tag.account_id == account_id && tag.keyword == keyword)
    {
        return TagTarget::Label(existing.label_id);
    }
    TagTarget::Create {
        name: name.to_owned(),
        color: color.to_owned(),
        keyword: keyword.to_owned(),
    }
}

/// Keyword written for a tag created on a tab, so sync stores `$label:<id>`.
pub fn keyword_for_new_tag(name: &str) -> String {
    let slug = crate::db::repo::labels::keyword_for(name)
        .to_lowercase()
        .replace('_', "-");
    format!("$label:{slug}")
}

/// Mailboxes a tab contributes before the company-view exclusion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabMailboxSources {
    /// `Some` when the tab is defined by named accounts. `Some([])` is empty.
    pub named_mailboxes: Option<Vec<i64>>,
    /// Profile membership, used when `named_mailboxes` is `None`.
    pub profile_mailboxes: Vec<i64>,
}

pub fn tab_mailbox_ids(sources: &TabMailboxSources) -> Vec<i64> {
    let mut ids = sources
        .named_mailboxes
        .clone()
        .unwrap_or_else(|| sources.profile_mailboxes.clone());
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompanyAccount {
    pub id: i64,
    pub email: String,
}

/// Tab mailboxes that feed the company inbox, merged folders, and tab tags.
///
/// `explicit` is the stored flag for an account id. A missing account record
/// stays included, because its address is unknown. The filter is one pass
/// over the tab's ids.
pub fn company_mailbox_ids(
    sources: &TabMailboxSources,
    accounts: &[CompanyAccount],
    explicit: &HashMap<i64, bool>,
) -> Vec<i64> {
    tab_mailbox_ids(sources)
        .into_iter()
        .filter(|id| {
            accounts
                .iter()
                .find(|account| account.id == *id)
                .is_none_or(|account| {
                    !excluded_from_company_views(&account.email, explicit.get(id).copied())
                })
        })
        .collect()
}

/// Mailboxes a named account lists itself. Membership in a profile is not a claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxClaim {
    pub named_account_id: i64,
    pub mailbox_ids: Vec<i64>,
}

/// Drop mailboxes claimed by a named account this view did not select.
///
/// A mailbox stays when a selected named account lists it explicitly, or when
/// no unselected named account lists it. Profile membership alone never claims
/// a mailbox, so a personal account that lists its mailboxes stays out of a
/// company view that did not select that account.
pub fn without_foreign_claims(
    mailboxes: impl IntoIterator<Item = i64>,
    selected_named_accounts: &[i64],
    claims: &[MailboxClaim],
) -> Vec<i64> {
    let selected = selected_named_accounts
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut kept = HashSet::new();
    let mut foreign = HashSet::new();
    for claim in claims {
        let bucket = if selected.contains(&claim.named_account_id) {
            &mut kept
        } else {
            &mut foreign
        };
        bucket.extend(claim.mailbox_ids.iter().copied());
    }
    let mut ids = mailboxes
        .into_iter()
        .filter(|id| kept.contains(id) || !foreign.contains(id))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// No extra icon. The chip and the sidebar keep the ordinary tag glyph.
pub const TAG_ICON_NONE: i32 = 0;
/// Exclamation, for action and urgent tags.
pub const TAG_ICON_ALERT: i32 = 1;
/// Dollar sign, for past-due and unpaid tags.
pub const TAG_ICON_DOLLAR: i32 = 2;
/// Check, for paid and done tags.
pub const TAG_ICON_CHECK: i32 = 3;
/// Clock, for due-soon and waiting tags.
pub const TAG_ICON_CLOCK: i32 = 4;

/// Lowercase keyword with `$label:` removed and spaces or underscores as hyphens.
pub fn tag_key(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    let value = value.strip_prefix("$label:").unwrap_or(&value);
    value
        .chars()
        .map(|character| match character {
            '_' | ' ' => '-',
            _ => character,
        })
        .collect()
}

/// Labels to add and remove after `added_keyword` is applied.
///
/// Keywords are compared with [`tag_key`]. A rule fired by an added label can
/// add further labels, which fire their own rules. The label that started the
/// cascade is never removed, and a loop cannot add the same keyword twice.
/// A keyword that one rule removes is not added by another rule in the same
/// cascade. Keywords already on the message are not added again.
pub fn label_rule_changes(
    rules: &[crate::models::LabelRule],
    added_keyword: &str,
    present: &[String],
) -> (Vec<String>, Vec<String>) {
    let trigger = tag_key(added_keyword);
    if trigger.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let present = present
        .iter()
        .map(|value| tag_key(value))
        .collect::<HashSet<_>>();
    let mut add = Vec::new();
    let mut remove = Vec::new();
    let mut seen = HashSet::from([trigger.clone()]);
    let mut queue = vec![trigger.clone()];
    while let Some(key) = queue.pop() {
        for rule in rules {
            if tag_key(&rule.when_added) != key {
                continue;
            }
            for target in &rule.remove {
                let target = tag_key(target);
                if target.is_empty()
                    || target == trigger
                    || remove.iter().any(|item| item == &target)
                {
                    continue;
                }
                remove.push(target);
            }
            for target in &rule.add {
                let target = tag_key(target);
                if target.is_empty()
                    || seen.contains(&target)
                    || present.contains(&target)
                    || remove.iter().any(|item| item == &target)
                {
                    continue;
                }
                seen.insert(target.clone());
                add.push(target.clone());
                queue.push(target);
            }
        }
    }
    add.retain(|target| !remove.iter().any(|item| item == target));
    (add, remove)
}

fn normalized_hex(value: &str) -> Option<String> {
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    (hex.len() == 6 && hex.chars().all(|character| character.is_ascii_hexdigit()))
        .then(|| format!("#{hex}").to_ascii_lowercase())
}

fn generic_tag_color(hex: &str) -> bool {
    matches!(
        hex,
        "#6b7280" | "#9ca3af" | "#808080" | "#cccccc" | "#4a86e8" | "#000000" | "#ffffff"
    )
}

/// A stored swatch the user or the server chose, rather than a placeholder gray.
pub fn is_tag_color_override(value: &str) -> bool {
    normalized_hex(value).is_some_and(|hex| !generic_tag_color(&hex))
}

fn semantic_tag(key: &str) -> Option<(&'static str, i32)> {
    const RED: &str = "#DC2626";
    const ORANGE: &str = "#EA580C";
    const YELLOW: &str = "#CA8A04";
    const BLUE: &str = "#2563EB";
    const GREEN: &str = "#16A34A";
    match key {
        "needs-action" | "requires-action" | "action-required" | "urgent" => {
            Some((RED, TAG_ICON_ALERT))
        }
        "overdue" | "past-due" | "unpaid" => Some((RED, TAG_ICON_DOLLAR)),
        "due-soon" | "upcoming" | "due-this-week" => Some((ORANGE, TAG_ICON_CLOCK)),
        "waiting" | "waiting-on" | "follow-up" | "followup" => Some((YELLOW, TAG_ICON_CLOCK)),
        "important" | "informational" | "info" => Some((BLUE, TAG_ICON_NONE)),
        "paid" | "done" | "complete" | "completed" => Some((GREEN, TAG_ICON_CHECK)),
        _ => None,
    }
}

/// Color and icon for a tag chip or sidebar row.
///
/// A stored non-generic `#RRGGBB` wins over the default. The icon still follows
/// the keyword or the display name. Tags with no default keep their stored color.
pub fn tag_display(keyword: &str, name: &str, stored_colors: &[&str]) -> (String, i32) {
    let appearance = semantic_tag(&tag_key(keyword)).or_else(|| semantic_tag(&tag_key(name)));
    if let Some(color) = stored_colors
        .iter()
        .copied()
        .find(|color| is_tag_color_override(color))
    {
        let icon = appearance.map(|(_, icon)| icon).unwrap_or(TAG_ICON_NONE);
        return (color.trim().to_owned(), icon);
    }
    if let Some((color, icon)) = appearance {
        return (color.to_owned(), icon);
    }
    let fallback = stored_colors
        .iter()
        .copied()
        .find(|color| normalized_hex(color).is_some())
        .unwrap_or("#6b7280");
    (fallback.to_owned(), TAG_ICON_NONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(account: i64, folder: i64, path: &str, unread: i64) -> FolderLeaf {
        FolderLeaf {
            account_id: account,
            folder_id: folder,
            path: path.to_owned(),
            unread,
        }
    }

    #[test]
    fn merged_folders_match_full_paths_without_regard_to_case() {
        let merged = merge_folders(&[
            leaf(1, 10, "Clients/Acme", 2),
            leaf(2, 20, "clients/acme", 1),
            leaf(1, 11, "Clients", 0),
            leaf(2, 21, "CLIENTS", 4),
            leaf(1, 12, "Clients/Other", 0),
            leaf(3, 30, "Vendors", 3),
        ]);
        let clients = merged.iter().find(|node| node.label == "Clients").unwrap();
        let acme = merged
            .iter()
            .find(|node| node.label.eq_ignore_ascii_case("acme"))
            .unwrap();
        assert_eq!(clients.members, vec![(1, 11), (2, 21)]);
        assert_eq!(clients.unread, 4);
        assert_eq!(acme.parent_id, clients.id);
        assert_eq!(acme.depth, 1);
        assert_eq!(acme.members, vec![(1, 10), (2, 20)]);
        assert_eq!(acme.unread, 3);
        assert!(clients.has_children);
        assert_eq!(
            merged
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Clients", "Acme", "Other", "Vendors"]
        );
        assert_eq!(folder_for_account(acme, 2), Some(20));
        assert_eq!(folder_for_account(acme, 3), None);
        assert_ne!(folder_scope_id("Clients/Acme"), folder_scope_id("Vendors"));
    }

    #[test]
    fn different_paths_stay_separate() {
        let merged = merge_folders(&[
            leaf(1, 1, "Clients/Acme", 0),
            leaf(2, 2, "Clients/Acme Team", 0),
        ]);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|node| node.members.len() == 1));
    }

    #[test]
    fn jmap_spaced_paths_merge_with_slash_paths() {
        let merged = merge_folders(&[
            leaf(1, 1, "Clients", 0),
            leaf(1, 2, "Clients / Acme", 1),
            leaf(2, 3, "clients/acme", 2),
        ]);
        assert_eq!(merged.len(), 2);
        let acme = merged.iter().find(|node| node.label == "Acme").unwrap();
        assert_eq!(acme.depth, 1);
        assert_eq!(acme.members, vec![(1, 2), (2, 3)]);
        assert_eq!(acme.unread, 3);
        assert_eq!(
            folder_match_key("Clients / Acme"),
            folder_match_key("clients/acme")
        );
    }

    #[test]
    fn company_view_exclusion_follows_the_flag_and_defaults_only_inbox() {
        assert!(!excluded_from_company_views("hub@example.com", None));
        assert!(!excluded_from_company_views("HUB@example.com", Some(false)));
        assert!(excluded_from_company_views("hub@example.com", Some(true)));
        assert!(excluded_from_company_views("inbox@example.com", None));
        assert!(!excluded_from_company_views(
            "inbox@example.com",
            Some(false)
        ));
        assert!(excluded_from_company_views("inbox@example.com", Some(true)));
        assert!(!excluded_from_company_views("work@example.com", None));
        assert!(excluded_from_company_views("work@example.com", Some(true)));
        assert!(!is_hub_address("hubs@example.com"));
        assert!(!excluded_from_company_views("not-an-email", None));
    }

    #[test]
    fn hundreds_of_mailboxes_merge_shared_paths_once() {
        let mut leaves = Vec::new();
        for account in 1..=275 {
            for (offset, path) in [(1, "Clients"), (2, "Clients/Acme"), (3, "Vendors")] {
                leaves.push(leaf(account, account * 10 + offset, path, 1));
            }
        }
        let merged = merge_folders(&leaves);
        assert_eq!(merged.len(), 3);
        let clients = merged.iter().find(|node| node.label == "Clients").unwrap();
        assert_eq!(clients.members.len(), 275);
        assert_eq!(clients.unread, 275);
        assert!(clients.has_children);
        let accounts = (1..=275)
            .map(|id| CompanyAccount {
                id,
                email: format!("box{id}@example.com"),
            })
            .chain([
                CompanyAccount {
                    id: 276,
                    email: "hub@example.com".into(),
                },
                CompanyAccount {
                    id: 277,
                    email: "inbox@example.com".into(),
                },
            ])
            .collect::<Vec<_>>();
        let sources = TabMailboxSources {
            named_mailboxes: Some((1..=277).collect()),
            profile_mailboxes: Vec::new(),
        };
        let included = company_mailbox_ids(&sources, &accounts, &HashMap::new());
        assert_eq!(included.len(), 276);
        assert!(included.contains(&276));
        assert!(!included.contains(&277));
        let mut explicit = HashMap::new();
        explicit.insert(276, true);
        explicit.insert(277, false);
        let flagged = company_mailbox_ids(&sources, &accounts, &explicit);
        assert!(!flagged.contains(&276));
        assert!(flagged.contains(&277));
    }

    #[test]
    fn tags_merge_by_keyword_and_route_to_each_account() {
        let copies = vec![
            TagCopy {
                account_id: 1,
                label_id: 8,
                keyword: "$label:lead".into(),
                name: "Lead".into(),
                color: "#112233".into(),
            },
            TagCopy {
                account_id: 2,
                label_id: 9,
                keyword: "$label:lead".into(),
                name: "Lead".into(),
                color: "#112233".into(),
            },
            TagCopy {
                account_id: 1,
                label_id: 10,
                keyword: "$label:invoice".into(),
                name: "Invoice".into(),
                color: "#abcdef".into(),
            },
        ];
        let merged = merge_tags(&copies);
        assert_eq!(merged.len(), 2);
        let lead = merged
            .iter()
            .find(|tag| tag.keyword == "$label:lead")
            .unwrap();
        assert_eq!(lead.members.len(), 2);
        assert_eq!(lead.color, "#112233");
        assert_eq!(
            tag_target_for_account(&copies, 2, "$label:lead", "Lead", "#112233"),
            TagTarget::Label(9)
        );
        assert_eq!(
            tag_target_for_account(&copies, 2, "$label:invoice", "Invoice", "#abcdef"),
            TagTarget::Create {
                name: "Invoice".into(),
                color: "#abcdef".into(),
                keyword: "$label:invoice".into(),
            }
        );
        assert_eq!(keyword_for_new_tag("Needs Action"), "$label:needs-action");
    }

    #[test]
    fn tab_scope_uses_named_mailboxes_or_profiles_then_drops_excluded() {
        let named = TabMailboxSources {
            named_mailboxes: Some(vec![3, 1, 1, 2]),
            profile_mailboxes: vec![9],
        };
        assert_eq!(tab_mailbox_ids(&named), vec![1, 2, 3]);
        let profiles = TabMailboxSources {
            named_mailboxes: None,
            profile_mailboxes: vec![4, 2, 4],
        };
        assert_eq!(tab_mailbox_ids(&profiles), vec![2, 4]);
        let accounts = vec![
            CompanyAccount {
                id: 1,
                email: "work@example.com".into(),
            },
            CompanyAccount {
                id: 2,
                email: "hub@example.com".into(),
            },
            CompanyAccount {
                id: 3,
                email: "inbox@example.com".into(),
            },
        ];
        let mut explicit = HashMap::new();
        explicit.insert(3, false);
        assert_eq!(
            company_mailbox_ids(&named, &accounts, &explicit),
            vec![1, 2, 3]
        );
        explicit.insert(2, true);
        assert_eq!(
            company_mailbox_ids(&named, &accounts, &explicit),
            vec![1, 3]
        );
    }

    #[test]
    fn a_named_account_claim_drops_that_mailbox_from_other_tabs() {
        let claims = vec![
            MailboxClaim {
                named_account_id: -5,
                mailbox_ids: vec![15, 16],
            },
            MailboxClaim {
                named_account_id: -9,
                mailbox_ids: vec![16, 40],
            },
        ];
        assert_eq!(
            without_foreign_claims(vec![15, 16, 40, 41], &[-9], &claims),
            vec![16, 40, 41]
        );
        assert_eq!(
            without_foreign_claims(vec![15, 16, 40, 41], &[-5, -9], &claims),
            vec![15, 16, 40, 41]
        );
        assert_eq!(
            without_foreign_claims(vec![41, 15, 41], &[], &claims),
            vec![41]
        );
    }

    #[test]
    fn standard_tags_take_a_default_color_unless_a_real_color_is_stored() {
        assert_eq!(
            tag_display("$label:needs-action", "Needs Action", &["#6b7280"]),
            ("#DC2626".into(), TAG_ICON_ALERT)
        );
        assert_eq!(
            tag_display("$label:past_due", "Past Due", &[""]),
            ("#DC2626".into(), TAG_ICON_DOLLAR)
        );
        assert_eq!(
            tag_display("$label:due-soon", "Due Soon", &["#4A86E8"]),
            ("#EA580C".into(), TAG_ICON_CLOCK)
        );
        assert_eq!(
            tag_display("$label:follow-up", "Follow Up", &["#cccccc"]),
            ("#CA8A04".into(), TAG_ICON_CLOCK)
        );
        assert_eq!(
            tag_display("$label:important", "Important", &["#9ca3af"]),
            ("#2563EB".into(), TAG_ICON_NONE)
        );
        assert_eq!(
            tag_display("$label:paid", "Paid", &["#ffffff"]),
            ("#16A34A".into(), TAG_ICON_CHECK)
        );
        assert_eq!(
            tag_display("$label:needs-action", "Needs Action", &["#112233"]),
            ("#112233".into(), TAG_ICON_ALERT)
        );
        assert_eq!(
            tag_display("$label:clients", "Clients", &["#6b7280"]),
            ("#6b7280".into(), TAG_ICON_NONE)
        );
        assert!(!is_tag_color_override("#808080"));
        assert!(is_tag_color_override("#112233"));
    }

    #[test]
    fn paying_a_bill_clears_the_open_bill_labels_and_can_add_another() {
        use crate::models::LabelRule;
        let rules = vec![
            LabelRule {
                when_added: "Paid".into(),
                remove: vec!["Needs Action".into(), "Overdue".into(), "unpaid".into()],
                add: Vec::new(),
            },
            LabelRule {
                when_added: "$label:paid".into(),
                remove: Vec::new(),
                add: vec!["done".into()],
            },
        ];
        let (add, remove) =
            label_rule_changes(&rules, "$label:paid", &["$label:needs-action".into()]);
        assert_eq!(add, vec!["done"]);
        assert_eq!(remove, vec!["needs-action", "overdue", "unpaid"]);
    }

    #[test]
    fn label_rules_do_not_loop_or_remove_the_label_just_added() {
        use crate::models::LabelRule;
        let rules = vec![
            LabelRule {
                when_added: "a".into(),
                remove: Vec::new(),
                add: vec!["b".into()],
            },
            LabelRule {
                when_added: "b".into(),
                remove: vec!["a".into()],
                add: vec!["a".into(), "c".into()],
            },
        ];
        let (add, remove) = label_rule_changes(&rules, "a", &[]);
        assert_eq!(add, vec!["b", "c"]);
        assert!(remove.is_empty());
    }
}
