//! Import a brand registry: profiles (companies), which accounts belong to
//! them, and the pinned business tabs.
//!
//! Two documents are accepted. A small file uses `profiles[].accounts` as
//! exact addresses or `*`/`?` wildcards. Tim's registry
//! (`schema: "flectar.brand-registry/v1-draft"`) is accepted as written:
//! profile `id`/`match`/`logo.file`, top-level `accounts[]` assignments, tab
//! `rule`, recipient lists, and `agentSenders`. Unknown fields are ignored.
//!
//! Applying a registry replaces mail profiles and business tabs. Profile ids
//! are the registry ids, not ids left over from an older grouping that
//! happened to share a name. Accounts and mail are never deleted. An account
//! no rule places goes to an Unassigned profile. The previous profiles and
//! tabs are kept once, on `settings.brand_registry_backup`, so the replace
//! can be undone. An explicit address beats a domain, and the first matching
//! rule in that group wins. The generator's "fallback to Tim Personal" is not
//! applied. A domain token that is not a domain (the Pixelated Cities hub
//! placeholder) is skipped; those mailboxes are assigned from `accounts[]`.

use crate::models::{BrandRegistryBackup, BusinessTab, MailProfile, Settings};
use crate::recipient_match::email_matches;
use base64::Engine;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandRegistry {
    pub profiles: Vec<RegistryProfile>,
    /// Recipient patterns and addresses for the Tim Mentink tab. When present
    /// they are merged into that tab, creating it when it is absent.
    pub tim_addresses: Vec<String>,
    pub tabs: Vec<RegistryTab>,
    /// Mailbox email to profile id or name, from the top-level `accounts` list.
    pub account_assignments: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryProfile {
    pub id: Option<String>,
    pub name: String,
    pub color: Option<String>,
    /// A `data:` URL or a path the host can read. Absent leaves a stored logo alone.
    pub logo: Option<String>,
    /// Exact addresses or wildcard patterns from the small schema, plus any
    /// string list in `accounts`.
    pub accounts: Vec<String>,
    pub domains: Vec<String>,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryTab {
    pub id: Option<String>,
    pub name: String,
    pub builtin: bool,
    /// Profile ids or names.
    pub profiles: Vec<String>,
    /// `profiles`, `recipients`, or `any`.
    pub scope: String,
    pub recipients: Vec<String>,
    pub sender_addresses: Vec<String>,
    pub sender_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryAccount {
    pub id: i64,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedLogo {
    pub profile_id: String,
    pub bytes: Vec<u8>,
    pub extension: String,
}

#[derive(Debug, Clone)]
pub struct BrandApply {
    pub settings: Settings,
    pub logos: Vec<AppliedLogo>,
    /// Accounts a rule moved or confirmed.
    pub matched: usize,
}

/// Prefixed to the registry bytes before hashing. Bumping it makes the next
/// launch re-apply a file whose bytes have not changed.
pub const IMPORTER_DIGEST_PREFIX: &str = "flectar-brand-importer/2\n";

const MAX_PROFILES: usize = 50;
const MAX_TABS: usize = 24;
const MAX_RULES: usize = 5_000;
const MAX_PATTERNS: usize = 4_096;
const PALETTE: [&str; 8] = [
    "#3B82F6", "#8B5CF6", "#0D9488", "#EA580C", "#DB2777", "#65A30D", "#0891B2", "#9333EA",
];

/// Decode a registry document.
pub fn parse_brand_registry(bytes: &[u8]) -> Result<BrandRegistry, String> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("brand registry is larger than 8 MB".into());
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid brand registry: {error}"))?;
    let root = value
        .as_object()
        .ok_or_else(|| "brand registry must be a JSON object".to_owned())?;
    if let Some(version) = root.get("version").and_then(Value::as_u64) {
        if version == 0 || version > 1 {
            return Err(format!("brand registry version {version} is not supported"));
        }
    }
    if let Some(schema) = root.get("schema").and_then(Value::as_str)
        && !schema.starts_with("flectar.brand-registry/")
    {
        return Err(format!("brand registry schema {schema:?} is not supported"));
    }

    let profiles = root
        .get("profiles")
        .and_then(Value::as_array)
        .map(|profiles| {
            profiles
                .iter()
                .map(parse_profile)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let tabs = root
        .get("tabs")
        .and_then(Value::as_array)
        .map(|tabs| tabs.iter().map(parse_tab).collect::<Result<Vec<_>, _>>())
        .transpose()?
        .unwrap_or_default();
    let tim_addresses = string_list(root.get("timAddresses"));
    let account_assignments = root
        .get("accounts")
        .and_then(Value::as_array)
        .map(|accounts| {
            accounts
                .iter()
                .filter_map(|account| {
                    let email = account.get("email")?.as_str()?.trim().to_owned();
                    let profile = account.get("profile")?.as_str()?.trim().to_owned();
                    if email.is_empty() || profile.is_empty() {
                        None
                    } else {
                        Some((email, profile))
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(BrandRegistry {
        profiles,
        tim_addresses,
        tabs,
        account_assignments,
    })
}

fn parse_profile(value: &Value) -> Result<RegistryProfile, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "a brand registry profile needs a name".to_owned())?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    let color = value
        .get("color")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let logo = value.get("logo").and_then(logo_source);
    let accounts = match value.get("accounts") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    let match_rules = value.get("match");
    Ok(RegistryProfile {
        id,
        name: name.to_owned(),
        color,
        logo,
        accounts,
        domains: string_list(match_rules.and_then(|rules| rules.get("domains"))),
        addresses: string_list(match_rules.and_then(|rules| rules.get("addresses"))),
    })
}

fn parse_tab(value: &Value) -> Result<RegistryTab, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "a brand registry tab needs a name".to_owned())?;
    let rule = value.get("rule").and_then(Value::as_str).unwrap_or("");
    let scope = if value.get("builtin").and_then(Value::as_bool) == Some(true) {
        "profiles".to_owned()
    } else if rule.contains("sender") {
        "any".to_owned()
    } else if rule == "recipient" {
        "recipients".to_owned()
    } else if rule == "profiles" || rule.is_empty() {
        "profiles".to_owned()
    } else if rule.contains("recipient") {
        "any".to_owned()
    } else {
        "profiles".to_owned()
    };
    let mut recipients = string_list(value.get("recipients"));
    recipients.extend(string_list(value.get("recipientPatterns")));
    recipients.extend(string_list(value.get("recipientAddresses")));
    recipients.extend(string_list(value.get("extraRecipientAddresses")));
    let senders = value.get("agentSenders");
    Ok(RegistryTab {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        name: name.to_owned(),
        builtin: value.get("builtin").and_then(Value::as_bool) == Some(true),
        profiles: string_list(value.get("profiles")),
        scope,
        recipients,
        sender_addresses: string_list(senders.and_then(|senders| senders.get("addresses"))),
        sender_names: string_list(senders.and_then(|senders| senders.get("displayNames"))),
    })
}

fn logo_source(value: &Value) -> Option<String> {
    match value {
        Value::String(path) => {
            let path = path.trim();
            (!path.is_empty()).then(|| path.to_owned())
        }
        Value::Object(object) => object
            .get("file")
            .or_else(|| object.get("data"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_owned),
        _ => None,
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Apply `registry` onto `settings`, replacing profiles and tabs.
///
/// `read_file` loads a logo path. Data URLs are decoded here. A registry logo
/// overwrites whatever was stored. When that file cannot be read, the bytes
/// of a previous logo with the same profile id, or else the same name, are
/// reused and written under the registry id. The returned logos are bytes for
/// the host to store under `profile-logos/<id>.<ext>`.
pub fn apply_brand_registry(
    settings: &Settings,
    accounts: &[RegistryAccount],
    registry: &BrandRegistry,
    mut read_file: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<BrandApply, String> {
    if registry.profiles.len() > MAX_PROFILES {
        return Err(format!(
            "brand registry has more than {MAX_PROFILES} profiles"
        ));
    }
    if registry.tabs.iter().filter(|tab| !tab.builtin).count() > MAX_TABS {
        return Err(format!("brand registry has more than {MAX_TABS} tabs"));
    }

    let mut settings = settings.clone();
    if settings.brand_registry_backup.is_none() {
        settings.brand_registry_backup = Some(BrandRegistryBackup {
            mail_profiles: settings.mail_profiles.clone(),
            business_tabs: settings.business_tabs.clone(),
        });
    }
    let previous = settings.mail_profiles.clone();
    let mut profiles: Vec<MailProfile> = Vec::new();
    let mut logos = Vec::new();
    let mut touched = HashSet::new();

    for (index, incoming) in registry.profiles.iter().enumerate() {
        let name = normalize_name(&incoming.name)?;
        let key = incoming
            .id
            .as_deref()
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| name.to_ascii_lowercase());
        if !touched.insert(key) {
            return Err(format!("brand registry repeats profile {name:?}"));
        }
        if incoming.accounts.len() + incoming.addresses.len() + incoming.domains.len() > MAX_RULES {
            return Err(format!(
                "profile {name:?} has more than {MAX_RULES} account rules"
            ));
        }
        let color = match incoming.color.as_deref() {
            Some(color) => normalize_color(color)?,
            None => None,
        };
        let profile_id = incoming
            .id
            .as_deref()
            .and_then(safe_slug)
            .filter(|id| profiles.iter().all(|profile| profile.id != *id))
            .unwrap_or_else(|| fresh_id("profile", &profiles, index));
        let mut profile = MailProfile {
            id: profile_id.clone(),
            name,
            color: color.unwrap_or_else(|| PALETTE[index % PALETTE.len()].to_owned()),
            account_ids: Vec::new(),
            logo: None,
        };
        if let Some(logo) = incoming.logo.as_deref()
            && let Some(applied) = decode_logo(&profile_id, logo, &mut read_file)?
        {
            profile.logo = Some(format!(
                "profile-logos/{}.{}",
                profile_id, applied.extension
            ));
            logos.push(applied);
        }
        if profile.logo.is_none()
            && let Some(applied) = carry_logo(&previous, incoming, &profile_id, &mut read_file)?
        {
            profile.logo = Some(format!(
                "profile-logos/{}.{}",
                profile_id, applied.extension
            ));
            logos.push(applied);
        }
        if profile.logo.is_none() {
            profile.logo = previous_logo_path(&previous, incoming, &profile_id);
        }
        profiles.push(profile);
    }

    let matched = assign_accounts(&mut profiles, accounts, registry)?;
    let mut placed = HashSet::new();
    for profile in &profiles {
        placed.extend(profile.account_ids.iter().copied());
    }
    let mut unassigned = accounts
        .iter()
        .map(|account| account.id)
        .filter(|id| placed.insert(*id))
        .collect::<Vec<_>>();
    unassigned.sort_unstable();
    unassigned.dedup();
    if !unassigned.is_empty() {
        if profiles.len() >= MAX_PROFILES {
            return Err(format!(
                "no more than {MAX_PROFILES} mail profiles can be created"
            ));
        }
        profiles.push(MailProfile {
            id: "unassigned".into(),
            name: "Unassigned".into(),
            color: "#6B7280".into(),
            account_ids: unassigned,
            logo: None,
        });
    }
    settings.mail_profiles = profiles;
    apply_tabs(&mut settings, registry)?;
    retire_active_profile(&mut settings);
    if settings
        .active_business_tab
        .as_ref()
        .is_some_and(|id| !settings.business_tabs.iter().any(|tab| &tab.id == id))
    {
        settings.active_business_tab = None;
    }
    Ok(BrandApply {
        settings,
        logos,
        matched,
    })
}

fn carry_logo(
    previous: &[MailProfile],
    incoming: &RegistryProfile,
    profile_id: &str,
    read_file: &mut impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<Option<AppliedLogo>, String> {
    let stored = previous
        .iter()
        .find(|profile| profile.id == profile_id)
        .or_else(|| {
            previous
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(incoming.name.trim()))
        })
        .and_then(|profile| profile.logo.as_deref());
    let Some(stored) = stored else {
        return Ok(None);
    };
    decode_logo(profile_id, stored, read_file)
}

fn previous_logo_path(
    previous: &[MailProfile],
    incoming: &RegistryProfile,
    profile_id: &str,
) -> Option<String> {
    previous
        .iter()
        .find(|profile| profile.id == profile_id)
        .or_else(|| {
            previous
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(incoming.name.trim()))
        })
        .and_then(|profile| profile.logo.as_deref())
        .and_then(safe_logo_relative)
        .map(str::to_owned)
}

fn assign_accounts(
    profiles: &mut [MailProfile],
    accounts: &[RegistryAccount],
    registry: &BrandRegistry,
) -> Result<usize, String> {
    let index_of = |key: &str| -> Option<usize> {
        profiles.iter().position(|profile| {
            profile.id.eq_ignore_ascii_case(key) || profile.name.eq_ignore_ascii_case(key)
        })
    };
    let mut exact: Vec<(usize, String)> = Vec::new();
    let mut domains: Vec<(usize, String)> = Vec::new();
    let mut wildcard: Vec<(usize, String)> = Vec::new();
    let mut seen_exact = HashSet::new();
    let mut seen_domains = HashSet::new();

    let push_exact = |exact: &mut Vec<(usize, String)>,
                      seen: &mut HashSet<String>,
                      profile: usize,
                      address: &str| {
        let address = address.trim().to_ascii_lowercase();
        if address.is_empty() || address.contains('*') || address.contains('?') {
            return;
        }
        if seen.insert(address.clone()) {
            exact.push((profile, address));
        }
    };

    for incoming in &registry.profiles {
        let Some(profile) = index_of(incoming.id.as_deref().unwrap_or(incoming.name.trim())) else {
            continue;
        };
        for address in &incoming.addresses {
            push_exact(&mut exact, &mut seen_exact, profile, address);
        }
    }
    for (email, profile_key) in &registry.account_assignments {
        let Some(profile) = index_of(profile_key) else {
            continue;
        };
        push_exact(&mut exact, &mut seen_exact, profile, email);
    }
    for incoming in &registry.profiles {
        let Some(profile) = index_of(incoming.id.as_deref().unwrap_or(incoming.name.trim())) else {
            continue;
        };
        for pattern in &incoming.accounts {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            if pattern.contains('*') || pattern.contains('?') {
                wildcard.push((profile, pattern.to_owned()));
            } else {
                push_exact(&mut exact, &mut seen_exact, profile, pattern);
            }
        }
        for domain in &incoming.domains {
            let domain = domain.trim().trim_start_matches('@').to_ascii_lowercase();
            if !is_domain(&domain) || !seen_domains.insert(domain.clone()) {
                continue;
            }
            domains.push((profile, domain));
        }
    }

    let mut matched = 0;
    for account in accounts {
        let email = account.email.trim().to_ascii_lowercase();
        let domain = email.split_once('@').map(|(_, domain)| domain);
        let winner = exact
            .iter()
            .find(|(_, address)| address == &email)
            .map(|(index, _)| *index)
            .or_else(|| {
                domain.and_then(|domain| {
                    domains
                        .iter()
                        .find(|(_, rule)| rule == domain)
                        .map(|(index, _)| *index)
                })
            })
            .or_else(|| {
                wildcard
                    .iter()
                    .find(|(_, pattern)| email_matches(pattern, &email))
                    .map(|(index, _)| *index)
            });
        let Some(winner) = winner else {
            continue;
        };
        matched += 1;
        for (index, profile) in profiles.iter_mut().enumerate() {
            profile.account_ids.retain(|id| *id != account.id);
            if index == winner {
                profile.account_ids.push(account.id);
                profile.account_ids.sort_unstable();
                profile.account_ids.dedup();
            }
        }
    }
    Ok(matched)
}

fn is_domain(value: &str) -> bool {
    value.contains('.')
        && !value.starts_with('.')
        && !value.ends_with('.')
        && !value.contains('<')
        && !value.contains(' ')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
}

fn apply_tabs(settings: &mut Settings, registry: &BrandRegistry) -> Result<(), String> {
    let sender_domains = sender_domains(registry);
    let profiles = settings.mail_profiles.clone();
    let mut tabs = Vec::new();
    let mut seen = HashSet::new();
    for (index, incoming) in registry.tabs.iter().enumerate() {
        if incoming.builtin {
            continue;
        }
        let name = normalize_name(&incoming.name)?;
        let key = incoming
            .id
            .as_deref()
            .unwrap_or(name.as_str())
            .to_ascii_lowercase();
        if !seen.insert(key) {
            return Err(format!("brand registry repeats tab {name:?}"));
        }
        tabs.push(build_tab(
            &profiles,
            &tabs,
            &name,
            incoming,
            index,
            &sender_domains,
        )?);
    }
    if !registry.tim_addresses.is_empty() {
        let name = "Tim Mentink";
        if let Some(tab) = tabs
            .iter_mut()
            .find(|tab| tab.id == "tim" || tab.name.eq_ignore_ascii_case(name))
        {
            tab.recipient_patterns =
                merge_patterns(&tab.recipient_patterns, &registry.tim_addresses);
            if tab.scope.is_empty() || (tab.scope == "profiles" && tab.profile_ids.is_empty()) {
                tab.scope = "recipients".to_owned();
            }
        } else {
            let id = incoming_tab_id("tim", &tabs, registry.tabs.len());
            tabs.insert(
                0,
                BusinessTab {
                    id,
                    name: name.to_owned(),
                    profile_ids: Vec::new(),
                    recipient_patterns: clean_patterns(&registry.tim_addresses),
                    scope: "recipients".to_owned(),
                    sender_addresses: Vec::new(),
                    sender_names: Vec::new(),
                    sender_domains: Vec::new(),
                },
            );
        }
    }
    if tabs.len() > MAX_TABS {
        return Err(format!(
            "no more than {MAX_TABS} business tabs can be saved"
        ));
    }
    settings.business_tabs = tabs;
    Ok(())
}

fn sender_domains(registry: &BrandRegistry) -> Vec<String> {
    let mut domains = Vec::new();
    let mut push = |value: &str| {
        let domain = value.trim().trim_start_matches('@').to_ascii_lowercase();
        let domain = domain
            .rsplit_once('@')
            .map(|(_, domain)| domain.to_owned())
            .unwrap_or(domain);
        if is_domain(&domain) && !domains.iter().any(|kept: &String| kept == &domain) {
            domains.push(domain);
        }
    };
    for profile in &registry.profiles {
        for domain in &profile.domains {
            push(domain);
        }
        for address in profile.addresses.iter().chain(profile.accounts.iter()) {
            push(address);
        }
    }
    for tab in &registry.tabs {
        for address in tab.recipients.iter().chain(tab.sender_addresses.iter()) {
            push(address);
        }
    }
    for address in &registry.tim_addresses {
        push(address);
    }
    domains
}

fn build_tab(
    profiles: &[MailProfile],
    taken: &[BusinessTab],
    name: &str,
    incoming: &RegistryTab,
    index: usize,
    sender_domains: &[String],
) -> Result<BusinessTab, String> {
    let profile_ids = incoming
        .profiles
        .iter()
        .filter_map(|key| {
            let key = key.trim();
            profiles
                .iter()
                .find(|profile| {
                    profile.id.eq_ignore_ascii_case(key) || profile.name.eq_ignore_ascii_case(key)
                })
                .map(|profile| profile.id.clone())
        })
        .collect::<Vec<_>>();
    let recipients = clean_patterns(&incoming.recipients);
    let sender_addresses = clean_patterns(&incoming.sender_addresses);
    let sender_names = clean_names(&incoming.sender_names);
    let sender_domains = if sender_names.is_empty() && sender_addresses.is_empty() {
        Vec::new()
    } else {
        sender_domains.to_vec()
    };
    let scope = if incoming.scope == "profiles"
        && !profile_ids.is_empty()
        && !recipients.is_empty()
        && sender_addresses.is_empty()
        && sender_names.is_empty()
    {
        // A small-schema tab that names both is the older AND filter.
        "narrow".to_owned()
    } else if incoming.scope == "profiles" && profile_ids.is_empty() && !recipients.is_empty() {
        "recipients".to_owned()
    } else {
        incoming.scope.clone()
    };
    let id = incoming
        .id
        .as_deref()
        .and_then(safe_slug)
        .filter(|id| taken.iter().all(|tab| tab.id != *id))
        .unwrap_or_else(|| incoming_tab_id(&format!("tab-{index}"), taken, index));
    Ok(BusinessTab {
        id,
        name: name.to_owned(),
        profile_ids,
        recipient_patterns: recipients,
        scope,
        sender_addresses,
        sender_names,
        sender_domains,
    })
}

fn incoming_tab_id(preferred: &str, taken: &[BusinessTab], salt: usize) -> String {
    let mut id = preferred.to_owned();
    if !taken.iter().any(|tab| tab.id == id) && safe_slug(&id).is_some() {
        return id;
    }
    id = format!("tab-{salt}-{}", taken.len());
    let mut suffix = 2u32;
    while taken.iter().any(|tab| tab.id == id) {
        id = format!("tab-{salt}-{}-{suffix}", taken.len());
        suffix += 1;
    }
    id
}

fn merge_patterns(current: &[String], extra: &[String]) -> Vec<String> {
    let mut combined = current.to_vec();
    combined.extend(extra.iter().cloned());
    clean_patterns(&combined)
}

fn clean_patterns(patterns: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for pattern in patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() || pattern.len() > 200 || out.len() >= MAX_PATTERNS {
            continue;
        }
        if !out
            .iter()
            .any(|kept: &String| kept.eq_ignore_ascii_case(pattern))
        {
            out.push(pattern.to_owned());
        }
    }
    out
}

fn clean_names(names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 || out.len() >= 32 {
            continue;
        }
        if !out
            .iter()
            .any(|kept: &String| kept.eq_ignore_ascii_case(name))
        {
            out.push(name.to_owned());
        }
    }
    out
}

fn decode_logo(
    profile_id: &str,
    logo: &str,
    read_file: &mut impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<Option<AppliedLogo>, String> {
    let logo = logo.trim();
    if logo.is_empty() {
        return Ok(None);
    }
    let (bytes, extension) = if let Some(rest) = logo.strip_prefix("data:") {
        decode_data_url(rest)?
    } else {
        let bytes = match read_file(logo) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(None),
        };
        let Some(extension) = extension_for(&bytes) else {
            return Ok(None);
        };
        (bytes, extension)
    };
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("a profile logo must be 2 MB or smaller".into());
    }
    Ok(Some(AppliedLogo {
        profile_id: profile_id.to_owned(),
        bytes,
        extension: extension.to_owned(),
    }))
}

fn decode_data_url(rest: &str) -> Result<(Vec<u8>, &'static str), String> {
    let (header, data) = rest
        .split_once(',')
        .ok_or_else(|| "a logo data URL is missing its payload".to_owned())?;
    if !header.split(';').any(|part| part == "base64") {
        return Err("a logo data URL must be base64".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(data.trim()))
        .map_err(|_| "a logo data URL is not valid base64".to_owned())?;
    let extension = extension_for(&bytes)
        .ok_or_else(|| "a logo data URL is not a PNG, JPEG, GIF, or WebP image".to_owned())?;
    Ok((bytes, extension))
}

pub fn extension_for(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// A stored logo path must stay inside `profile-logos/` and name one file.
pub fn safe_logo_relative(value: &str) -> Option<&str> {
    let value = value.trim();
    let rest = value.strip_prefix("profile-logos/")?;
    if rest.is_empty()
        || rest.contains('/')
        || rest.contains('\\')
        || rest.contains("..")
        || !rest
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return None;
    }
    Some(value)
}

fn safe_slug(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return None;
    }
    Some(value.to_owned())
}

fn normalize_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("a profile or tab name cannot be empty".into());
    }
    if value.chars().count() > 48 {
        return Err(format!("{value:?} is longer than 48 characters"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{value:?} contains a control character"));
    }
    Ok(value.to_owned())
}

fn normalize_color(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let valid = value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit());
    if !valid {
        return Err(format!("color {value:?} must use #RRGGBB"));
    }
    Ok(Some(value.to_ascii_uppercase()))
}

fn fresh_id(prefix: &str, profiles: &[MailProfile], salt: usize) -> String {
    let mut id = format!("{prefix}-{salt}-{}", profiles.len());
    let mut suffix = 2u32;
    while profiles.iter().any(|profile| profile.id == id) {
        id = format!("{prefix}-{salt}-{}-{suffix}", profiles.len());
        suffix += 1;
    }
    id
}

fn retire_active_profile(settings: &mut Settings) {
    let showable = settings
        .active_mail_profile
        .as_deref()
        .is_some_and(|active| {
            settings
                .mail_profiles
                .iter()
                .any(|profile| profile.id == active && !profile.account_ids.is_empty())
        });
    if !showable {
        settings.active_mail_profile = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: i64, email: &str) -> RegistryAccount {
        RegistryAccount {
            id,
            email: email.to_owned(),
        }
    }

    fn profile(name: &str, color: &str, logo: Option<&str>, accounts: &[&str]) -> RegistryProfile {
        RegistryProfile {
            id: None,
            name: name.into(),
            color: Some(color.into()),
            logo: logo.map(str::to_owned),
            accounts: accounts
                .iter()
                .map(|account| (*account).to_owned())
                .collect(),
            domains: Vec::new(),
            addresses: Vec::new(),
        }
    }

    fn tab(name: &str, profiles: &[&str], recipients: &[&str]) -> RegistryTab {
        RegistryTab {
            id: None,
            name: name.into(),
            builtin: false,
            profiles: profiles
                .iter()
                .map(|profile| (*profile).to_owned())
                .collect(),
            scope: if recipients.is_empty() {
                "profiles".into()
            } else if profiles.is_empty() {
                "recipients".into()
            } else {
                "narrow".into()
            },
            recipients: recipients
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
            sender_addresses: Vec::new(),
            sender_names: Vec::new(),
        }
    }

    fn registry() -> BrandRegistry {
        BrandRegistry {
            tim_addresses: vec![
                "tim@*".into(),
                "tim.mentink@*".into(),
                "ohnage@*".into(),
                "tmentink@gmail.com".into(),
            ],
            profiles: vec![
                profile(
                    "Mentinkizm LLC",
                    "#ea580c",
                    Some("data:image/png;base64,iVBORw0KGgo="),
                    &["*@mentinkizm.com", "owner@mentinkizm.com"],
                ),
                profile("Pixelated Cities", "#8b5cf6", None, &["*@pixelated.test"]),
            ],
            tabs: vec![
                tab("Tim Mentink", &[], &[]),
                tab("Mentinkizm", &["Mentinkizm LLC"], &[]),
                tab("Pixelated Cities", &["Pixelated Cities"], &[]),
            ],
            account_assignments: Vec::new(),
        }
    }

    #[test]
    fn explicit_address_beats_an_earlier_domain_rule() {
        let mut registry = registry();
        registry.profiles[0].accounts = vec!["*@mentinkizm.com".into()];
        registry.profiles.push(profile(
            "Personal",
            "#3b82f6",
            None,
            &["owner@mentinkizm.com"],
        ));
        let applied = apply_brand_registry(
            &Settings::default(),
            &[
                account(1, "owner@mentinkizm.com"),
                account(2, "other@mentinkizm.com"),
                account(3, "keep@elsewhere.test"),
            ],
            &registry,
            |_| Err("no file".into()),
        )
        .unwrap();
        let profile = |name: &str| {
            applied
                .settings
                .mail_profiles
                .iter()
                .find(|profile| profile.name == name)
                .unwrap()
        };
        assert_eq!(profile("Personal").account_ids, vec![1]);
        assert_eq!(profile("Mentinkizm LLC").account_ids, vec![2]);
        assert!(profile("Pixelated Cities").account_ids.is_empty());
        assert_eq!(profile("Unassigned").account_ids, vec![3]);
        assert_eq!(applied.matched, 2);
        assert!(
            profile("Mentinkizm LLC")
                .logo
                .as_deref()
                .unwrap()
                .ends_with(".png")
        );
        assert_eq!(
            applied
                .settings
                .business_tabs
                .iter()
                .map(|tab| tab.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Tim Mentink", "Mentinkizm", "Pixelated Cities"]
        );
        assert_eq!(
            applied.settings.business_tabs[0].recipient_patterns,
            vec!["tim@*", "tim.mentink@*", "ohnage@*", "tmentink@gmail.com"]
        );
        assert_eq!(
            applied.settings.business_tabs[0].resolved_scope(),
            "recipients"
        );
    }

    #[test]
    fn applying_twice_replaces_legacy_profiles_and_keeps_the_backup() {
        let mut settings = Settings::default();
        settings.mail_profiles.push(MailProfile {
            id: "personal".into(),
            name: "Personal".into(),
            color: "#3B82F6".into(),
            account_ids: vec![3, 9],
            logo: None,
        });
        let registry = registry();
        let accounts = [
            account(3, "keep@elsewhere.test"),
            account(9, "owner@mentinkizm.com"),
        ];
        let once = apply_brand_registry(&settings, &accounts, &registry, |_| Err("no file".into()))
            .unwrap();
        assert!(
            once.settings
                .mail_profiles
                .iter()
                .all(|profile| profile.id != "personal")
        );
        assert_eq!(
            once.settings
                .mail_profiles
                .iter()
                .find(|profile| profile.id == "unassigned")
                .unwrap()
                .account_ids,
            vec![3]
        );
        let backup = once.settings.brand_registry_backup.as_ref().unwrap();
        assert_eq!(backup.mail_profiles[0].id, "personal");
        let twice = apply_brand_registry(&once.settings, &accounts, &registry, |_| {
            Err("no file".into())
        })
        .unwrap();
        assert_eq!(once.settings.mail_profiles, twice.settings.mail_profiles);
        assert_eq!(once.settings.business_tabs, twice.settings.business_tabs);
        assert_eq!(
            once.settings.brand_registry_backup,
            twice.settings.brand_registry_backup
        );
    }

    #[test]
    fn a_tiny_png_data_url_decodes() {
        let png = b"\x89PNG\r\n\x1a\n";
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let (bytes, extension) = decode_data_url(&format!("image/png;base64,{encoded}")).unwrap();
        assert_eq!(bytes, png);
        assert_eq!(extension, "png");
    }

    #[test]
    fn trimmed_tim_registry_assigns_hubs_exact_addresses_and_sender_rules() {
        let png = include_bytes!("../../../docs/fixtures/brand-registry/logos/forty7.png");
        let bytes = include_str!("../../../docs/fixtures/brand-registry/registry.json");
        let registry = parse_brand_registry(bytes.as_bytes()).unwrap();
        let mut settings = Settings::default();
        settings.mail_profiles.push(MailProfile {
            id: "personal".into(),
            name: "Personal".into(),
            color: "#3B82F6".into(),
            account_ids: vec![9],
            logo: None,
        });
        let applied = apply_brand_registry(
            &settings,
            &[
                account(1, "tmentink@gmail.com"),
                account(2, "contact@mentinkizm.com"),
                account(3, "47@mentinkizm.com"),
                account(4, "hub@abilenetexas.org"),
                account(5, "other@gmail.com"),
                account(9, "keep@elsewhere.test"),
            ],
            &registry,
            |path| {
                if path.ends_with("forty7.png") {
                    Ok(png.to_vec())
                } else {
                    Err(format!("missing {path}"))
                }
            },
        )
        .unwrap();
        let profile = |id: &str| {
            applied
                .settings
                .mail_profiles
                .iter()
                .find(|profile| profile.id == id)
                .unwrap()
        };
        assert_eq!(profile("tim-personal").account_ids, vec![1, 5]);
        assert_eq!(profile("mentinkizm").account_ids, vec![2]);
        assert_eq!(profile("forty7").account_ids, vec![3]);
        assert_eq!(profile("pixelated-cities").account_ids, vec![4]);
        assert_eq!(profile("unassigned").account_ids, vec![9]);
        assert!(
            applied
                .settings
                .mail_profiles
                .iter()
                .all(|profile| profile.id != "personal")
        );
        assert!(profile("forty7").logo.as_deref().unwrap().ends_with(".png"));
        assert!(applied.logos.iter().any(|logo| logo.profile_id == "forty7"));
        let tim = applied
            .settings
            .business_tabs
            .iter()
            .find(|tab| tab.id == "tim")
            .unwrap();
        assert_eq!(tim.resolved_scope(), "recipients");
        assert!(
            tim.recipient_patterns
                .iter()
                .any(|pattern| pattern == "tim@*")
        );
        assert!(
            tim.recipient_patterns
                .iter()
                .any(|pattern| pattern == "ohnage10@ug4l.com")
        );
        let forty7 = applied
            .settings
            .business_tabs
            .iter()
            .find(|tab| tab.id == "forty7")
            .unwrap();
        assert_eq!(forty7.resolved_scope(), "any");
        assert_eq!(
            forty7.sender_names,
            vec!["Mailroom", "47 by Rook", "Rook", "Hermes"]
        );
        assert!(
            forty7
                .sender_addresses
                .iter()
                .any(|address| address == "hi@p47.io")
        );
        assert!(
            forty7
                .recipient_patterns
                .iter()
                .any(|address| address == "rook@helplearnhow.com")
        );
        assert!(
            forty7
                .sender_domains
                .iter()
                .any(|domain| domain == "p47.io")
        );
        assert!(
            forty7
                .sender_domains
                .iter()
                .any(|domain| domain == "mentinkizm.com")
        );
        assert!(
            applied
                .settings
                .business_tabs
                .iter()
                .all(|tab| tab.id != "all")
        );
        let again = apply_brand_registry(
            &applied.settings,
            &[
                account(1, "tmentink@gmail.com"),
                account(2, "contact@mentinkizm.com"),
                account(3, "47@mentinkizm.com"),
                account(4, "hub@abilenetexas.org"),
                account(5, "other@gmail.com"),
                account(9, "keep@elsewhere.test"),
            ],
            &registry,
            |path| {
                if path.ends_with("forty7.png") {
                    Ok(png.to_vec())
                } else {
                    Err(format!("missing {path}"))
                }
            },
        )
        .unwrap();
        assert_eq!(applied.settings.mail_profiles, again.settings.mail_profiles);
        assert_eq!(applied.settings.business_tabs, again.settings.business_tabs);
    }

    #[test]
    fn installed_settings_are_replaced_by_the_real_registry() {
        let registry = parse_brand_registry(include_bytes!(
            "../../../docs/fixtures/brand-registry/tim-registry.json"
        ))
        .unwrap();
        let excerpt: Value = serde_json::from_str(include_str!(
            "../../../docs/fixtures/brand-registry/settings-excerpt.json"
        ))
        .unwrap();
        let mut settings = Settings::default();
        settings.mail_profiles = serde_json::from_value(excerpt["mailProfiles"].clone()).unwrap();
        settings.business_tabs = serde_json::from_value(excerpt["businessTabs"].clone()).unwrap();
        settings.active_business_tab = excerpt["activeBusinessTab"].as_str().map(str::to_owned);
        settings.brand_registry_digest = excerpt["brandRegistryDigest"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let accounts = excerpt["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|account| RegistryAccount {
                id: account["id"].as_i64().unwrap(),
                email: account["email"].as_str().unwrap().to_owned(),
            })
            .collect::<Vec<_>>();
        assert_eq!(accounts.len(), 288);
        assert!(
            settings
                .mail_profiles
                .iter()
                .any(|profile| profile.id.starts_with("profile-"))
        );
        let png = b"\x89PNG\r\n\x1a\n".to_vec();
        let applied = apply_brand_registry(&settings, &accounts, &registry, |path| {
            if path.ends_with("mentink-media.png") {
                Ok(png.clone())
            } else {
                Err(format!("missing {path}"))
            }
        })
        .unwrap();
        assert!(
            applied
                .settings
                .mail_profiles
                .iter()
                .all(|profile| !profile.id.starts_with("profile-"))
        );
        let mut assigned = applied
            .settings
            .mail_profiles
            .iter()
            .flat_map(|profile| profile.account_ids.iter().copied())
            .collect::<Vec<_>>();
        assigned.sort_unstable();
        let mut expected = accounts
            .iter()
            .map(|account| account.id)
            .collect::<Vec<_>>();
        expected.sort_unstable();
        assert_eq!(assigned, expected);
        assert!(
            applied
                .settings
                .mail_profiles
                .iter()
                .all(|profile| profile.id != "unassigned")
        );
        let profile = |id: &str| {
            applied
                .settings
                .mail_profiles
                .iter()
                .find(|profile| profile.id == id)
                .unwrap()
        };
        let tab = |id: &str| {
            applied
                .settings
                .business_tabs
                .iter()
                .find(|tab| tab.id == id)
                .unwrap()
        };
        assert!(profile("pixelated-cities").account_ids.len() >= 275);
        assert_eq!(
            tab("pixelated-cities").profile_ids,
            vec!["pixelated-cities".to_owned()]
        );
        let contact = accounts
            .iter()
            .find(|account| account.email.eq_ignore_ascii_case("contact@mentinkizm.com"))
            .unwrap();
        assert!(profile("mentinkizm").account_ids.contains(&contact.id));
        assert!(
            tab("mentinkizm")
                .profile_ids
                .iter()
                .any(|id| id == "mentinkizm")
        );
        let tim = tab("tim");
        assert_eq!(tim.resolved_scope(), "recipients");
        let matcher = crate::recipient_match::RecipientMatcher::compile(&tim.recipient_patterns);
        assert!(matcher.matches_email("tim.mentink@mentinkizm.com"));
        assert!(matcher.matches_headers(
            r#"[{"name":null,"email":"tim.mentink@mentinkizm.com"}]"#,
            "[]",
            "[]",
            "hub@example.org",
        ));
        assert_eq!(
            tab("mentink-media")
                .icon_profile(&applied.settings.mail_profiles)
                .unwrap()
                .id,
            "mentink-media"
        );
        assert!(
            profile("mentink-media")
                .logo
                .as_deref()
                .unwrap()
                .ends_with("mentink-media.png")
        );
        assert_eq!(
            applied.settings.active_business_tab.as_deref(),
            Some("forty7")
        );
        assert_eq!(tab("forty7").resolved_scope(), "any");
        assert!(
            applied
                .settings
                .brand_registry_backup
                .as_ref()
                .unwrap()
                .mail_profiles
                .iter()
                .any(|profile| profile.id.starts_with("profile-"))
        );
        let again = apply_brand_registry(&applied.settings, &accounts, &registry, |path| {
            if path.ends_with("mentink-media.png") {
                Ok(png.clone())
            } else {
                Err(format!("missing {path}"))
            }
        })
        .unwrap();
        assert_eq!(applied.settings.mail_profiles, again.settings.mail_profiles);
        assert_eq!(applied.settings.business_tabs, again.settings.business_tabs);
        assert_eq!(
            applied.settings.brand_registry_backup,
            again.settings.brand_registry_backup
        );
        assert!(IMPORTER_DIGEST_PREFIX.contains("flectar-brand-importer/2"));
    }
}
