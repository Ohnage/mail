//! Pixelated Cities hub routing from the live mailsort `pixelated_hub` schema.
//!
//! City hub accounts are not synced. Mail lives on hub@pixelatedcities.com;
//! city From addresses submit through that hub's SMTP and appear in the From
//! list from this catalog. City logins were renamed from `hub@<city>` to
//! `contact@<city>`; the allowed city send-as locals are listed in the JSON.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

const HUB_JSON: &str = include_str!("../../../resources/pixelated_hub.json");

#[derive(Debug, Deserialize)]
struct Root {
    pixelated_hub: HubConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubConfig {
    pub address: String,
    pub cities_parent: String,
    pub header: String,
    pub status_folders: BTreeMap<String, String>,
    #[serde(default)]
    pub hub_account_folders: Vec<String>,
    pub city_folders: BTreeMap<String, String>,
    pub city_domains: BTreeMap<String, Vec<String>>,
    /// Locals authorized as city From / send-as on every city domain.
    #[serde(default = "default_city_send_as")]
    pub city_send_as: Vec<String>,
}

fn default_city_send_as() -> Vec<String> {
    [
        "contact",
        "hi",
        "no-reply",
        "lead",
        "legal",
        "privacy",
        "postmaster",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn hub() -> &'static HubConfig {
    static HUB: OnceLock<HubConfig> = OnceLock::new();
    HUB.get_or_init(|| {
        serde_json::from_str::<Root>(HUB_JSON)
            .expect("resources/pixelated_hub.json")
            .pixelated_hub
    })
}

pub fn hub_address() -> &'static str {
    hub().address.as_str()
}

pub fn cities_parent() -> &'static str {
    hub().cities_parent.as_str()
}

pub fn hub_header() -> &'static str {
    hub().header.as_str()
}

pub fn status_folder_names() -> impl Iterator<Item = &'static str> {
    hub()
        .status_folders
        .values()
        .map(String::as_str)
        .chain(hub().hub_account_folders.iter().map(String::as_str))
}

pub fn city_count() -> usize {
    hub().city_folders.len()
}

pub fn city_send_as_locals() -> &'static BTreeSet<String> {
    static LOCALS: OnceLock<BTreeSet<String>> = OnceLock::new();
    LOCALS.get_or_init(|| {
        hub()
            .city_send_as
            .iter()
            .map(|local| local.trim().to_ascii_lowercase())
            .filter(|local| !local.is_empty())
            .collect()
    })
}

/// Primary city From address (`contact@…`) and display name per city folder.
pub fn city_from_addresses() -> impl Iterator<Item = (&'static str, &'static str)> {
    hub()
        .city_folders
        .iter()
        .map(|(email, name)| (email.as_str(), name.as_str()))
}

/// Every authorized city send-as address (`contact@`, `lead@`, …) with the
/// city display name. Used when expanding From identities from the catalog.
pub fn city_send_as_addresses() -> impl Iterator<Item = (String, &'static str)> {
    hub().city_folders.iter().flat_map(|(primary, name)| {
        let domain = primary
            .rsplit_once('@')
            .map(|(_, domain)| domain.to_ascii_lowercase())
            .unwrap_or_default();
        city_send_as_locals()
            .iter()
            .map(move |local| (format!("{local}@{domain}"), name.as_str()))
    })
}

/// Domains that identify city-routed mail for reply-From fallback.
pub fn city_domains() -> &'static BTreeSet<String> {
    static DOMAINS: OnceLock<BTreeSet<String>> = OnceLock::new();
    DOMAINS.get_or_init(|| {
        hub()
            .city_domains
            .values()
            .flatten()
            .map(|domain| domain.trim().to_ascii_lowercase())
            .filter(|domain| !domain.is_empty())
            .collect()
    })
}

pub fn is_city_from_address(email: &str) -> bool {
    let email = email.trim().to_ascii_lowercase();
    if hub().city_folders.contains_key(&email) {
        return true;
    }
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    if local.contains('+') {
        return false;
    }
    city_send_as_locals().contains(local) && city_domains().contains(domain)
}

pub fn is_city_domain_address(email: &str) -> bool {
    email
        .rsplit_once('@')
        .map(|(_, domain)| city_domains().contains(&domain.trim().to_ascii_lowercase()))
        .unwrap_or(false)
}

/// Map a received city address to the From that should reply.
///
/// Exact allowed locals keep themselves. Plus-addressed `lead+x@` and legacy
/// `hub@<city>` map to `contact@<city>`. Non-city addresses return `None`.
pub fn city_reply_from(email: &str) -> Option<String> {
    let email = email.trim();
    let (local, domain) = email.rsplit_once('@')?;
    let domain = domain.trim().to_ascii_lowercase();
    if !city_domains().contains(&domain) {
        return None;
    }
    let local = local.trim().to_ascii_lowercase();
    let (base, has_plus) = match local.split_once('+') {
        Some((base, detail)) if !base.is_empty() && !detail.is_empty() => (base, true),
        _ => (local.as_str(), false),
    };
    if has_plus || base == "hub" || !city_send_as_locals().contains(base) {
        return Some(format!("contact@{domain}"));
    }
    Some(format!("{base}@{domain}"))
}

/// Rename a stored `hub@<city>` login/address to `contact@<city>`.
/// Leaves `hub@pixelatedcities.com` and non-city addresses unchanged.
pub fn migrate_hub_city_address(email: &str) -> Option<String> {
    let email = email.trim();
    let (local, domain) = email.rsplit_once('@')?;
    if !local.eq_ignore_ascii_case("hub") {
        return None;
    }
    let domain = domain.trim().to_ascii_lowercase();
    if domain == "pixelatedcities.com" || domain == "pixelated.cities" {
        return None;
    }
    if !city_domains().contains(&domain) {
        return None;
    }
    Some(format!("contact@{domain}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_catalog_matches_addendum_shape() {
        assert_eq!(hub_address(), "hub@pixelatedcities.com");
        assert_eq!(cities_parent(), "Cities");
        assert_eq!(hub_header(), "X-Pixelated-Hub");
        assert_eq!(city_count(), 273);
        assert_eq!(city_domains().len(), 420);
        assert!(status_folder_names().any(|name| name == "Action Items"));
        assert!(status_folder_names().any(|name| name == "Archive"));
        assert!(is_city_from_address("contact@jacksonmichigan.org"));
        assert!(is_city_from_address("lead@jacksonmichigan.org"));
        assert!(is_city_from_address("privacy@annarbormichigan.org"));
        assert!(!is_city_from_address("hub@jacksonmichigan.org"));
        assert!(!is_city_from_address("lead+roofing@jacksonmichigan.org"));
        assert!(is_city_domain_address("anyone@jacksonmichigan.org"));
        assert!(!is_city_from_address("hub@pixelatedcities.com"));
        assert!(city_send_as_locals().contains("contact"));
        assert!(!city_send_as_locals().contains("hub"));
    }

    #[test]
    fn city_reply_from_maps_lead_plus_legacy_hub_and_keeps_allowed_locals() {
        assert_eq!(
            city_reply_from("lead+roofing@jacksonmichigan.org").as_deref(),
            Some("contact@jacksonmichigan.org")
        );
        assert_eq!(
            city_reply_from("lead@jacksonmichigan.org").as_deref(),
            Some("lead@jacksonmichigan.org")
        );
        assert_eq!(
            city_reply_from("privacy@jacksonmichigan.org").as_deref(),
            Some("privacy@jacksonmichigan.org")
        );
        assert_eq!(
            city_reply_from("hub@jacksonmichigan.org").as_deref(),
            Some("contact@jacksonmichigan.org")
        );
        assert_eq!(city_reply_from("tim@example.com"), None);
        assert_eq!(city_reply_from("hub@pixelatedcities.com"), None);
    }

    #[test]
    fn migrate_hub_city_address_renames_cities_only() {
        assert_eq!(
            migrate_hub_city_address("hub@jacksonmichigan.org").as_deref(),
            Some("contact@jacksonmichigan.org")
        );
        assert_eq!(migrate_hub_city_address("hub@pixelatedcities.com"), None);
        assert_eq!(
            migrate_hub_city_address("contact@jacksonmichigan.org"),
            None
        );
        assert_eq!(migrate_hub_city_address("tim@example.com"), None);
    }
}
