//! Combined company mailbox for one business tab.
//!
//! A tab's mailboxes stay separate on the server. The company view matches
//! folders by their full path and tags by their keyword, then asks each
//! message's own account to carry out the change.

use std::collections::{BTreeMap, HashMap};

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
}
