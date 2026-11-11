//! Pixelated Cities hub@ routing from the live mailsort `pixelated_hub` schema.
//!
//! City hub accounts are not synced. Mail lives on hub@; city From addresses
//! submit through hub@'s SMTP and appear in the From list from this catalog.

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

pub fn city_from_addresses() -> impl Iterator<Item = (&'static str, &'static str)> {
    hub()
        .city_folders
        .iter()
        .map(|(email, name)| (email.as_str(), name.as_str()))
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
    hub().city_folders.contains_key(&email)
}

pub fn is_city_domain_address(email: &str) -> bool {
    email
        .rsplit_once('@')
        .map(|(_, domain)| city_domains().contains(&domain.trim().to_ascii_lowercase()))
        .unwrap_or(false)
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
        assert!(status_folder_names().any(|name| name == "Action Items"));
        assert!(status_folder_names().any(|name| name == "Archive"));
        assert!(is_city_from_address("hub@jacksonmichigan.org"));
        assert!(is_city_domain_address("anyone@jacksonmichigan.org"));
        assert!(!is_city_from_address("hub@pixelatedcities.com"));
    }
}
