//! Named inbox accounts are local views. Connections and sending identities are separate.
use crate::{
    brand_registry::{RegistryAccount, address_view_id},
    models::{InboxAccount, Settings},
};

fn key(value: &str) -> i64 {
    -address_view_id(&format!("inbox-account:{value}"))
}

pub fn initialize(settings: &mut Settings, connections: &[RegistryAccount]) -> bool {
    if settings.inbox_accounts_initialized {
        return false;
    }
    if settings.mail_profiles.is_empty() && settings.business_tabs.is_empty() {
        return false;
    }
    settings.inbox_accounts = settings
        .mail_profiles
        .iter()
        .map(|profile| InboxAccount {
            id: key(&profile.id),
            name: profile.name.clone(),
            icon: profile.logo.clone(),
            account_ids: Vec::new(),
            address_view_ids: Vec::new(),
            automatic_profile: Some(profile.id.clone()),
            profile_id: Some(profile.id.clone()),
        })
        .collect();
    // Preserve explicitly selected whole mailboxes and aliases as individual accounts.
    for tab in &settings.business_tabs {
        for id in tab.account_ids.iter().flatten() {
            let id_key = key(&format!("mailbox:{id}"));
            if settings
                .inbox_accounts
                .iter()
                .any(|account| account.id == id_key)
            {
                continue;
            }
            settings.inbox_accounts.push(InboxAccount {
                id: id_key,
                name: connections
                    .iter()
                    .find(|account| account.id == *id)
                    .map(|account| account.email.clone())
                    .unwrap_or_else(|| format!("Mailbox {id}")),
                icon: settings.account_icons.get(&id.to_string()).cloned(),
                account_ids: vec![*id],
                address_view_ids: Vec::new(),
                automatic_profile: None,
                profile_id: None,
            });
        }
        for id in tab.address_view_ids.iter().flatten() {
            let id_key = key(&format!("address:{id}"));
            if settings
                .inbox_accounts
                .iter()
                .any(|account| account.id == id_key)
            {
                continue;
            }
            if let Some(view) = settings.address_views.iter().find(|view| view.id == *id) {
                settings.inbox_accounts.push(InboxAccount {
                    id: id_key,
                    name: view.address.clone(),
                    icon: None,
                    account_ids: Vec::new(),
                    address_view_ids: vec![*id],
                    automatic_profile: None,
                    profile_id: None,
                });
            }
        }
    }
    settings.inbox_accounts_initialized = true;
    true
}

pub fn selected_for_tab(settings: &Settings, tab: &crate::models::BusinessTab) -> Vec<i64> {
    if let Some(ids) = settings.business_tab_accounts.get(&tab.id) {
        return ids.clone();
    }
    settings
        .inbox_accounts
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

pub fn save(
    settings: &mut Settings,
    mut account: InboxAccount,
    connections: &[RegistryAccount],
) -> Result<(), String> {
    initialize(settings, connections);
    settings.inbox_accounts_initialized = true;
    account.name = account.name.trim().to_owned();
    if account.name.is_empty() || account.name.chars().count() > 120 {
        return Err("Enter an account name of 1–120 characters".into());
    }
    if account.id == 0 {
        account.id = key(&account.name);
        while settings
            .inbox_accounts
            .iter()
            .any(|item| item.id == account.id)
        {
            account.id = account.id.checked_sub(1).ok_or("Account key exhausted")?;
        }
    } else if !settings
        .inbox_accounts
        .iter()
        .any(|item| item.id == account.id)
    {
        return Err("Account no longer exists".into());
    }
    if settings
        .inbox_accounts
        .iter()
        .any(|item| item.id != account.id && item.name.eq_ignore_ascii_case(&account.name))
    {
        return Err("An account with this name already exists".into());
    }
    account.account_ids.sort_unstable();
    account.account_ids.dedup();
    account.address_view_ids.sort_unstable();
    account.address_view_ids.dedup();
    for id in &account.account_ids {
        if !connections.iter().any(|item| item.id == *id) {
            return Err(format!("Mailbox {id} is not connected"));
        }
    }
    for id in &account.address_view_ids {
        if !settings.address_views.iter().any(|item| item.id == *id) {
            return Err("A selected address is no longer available".into());
        }
    }
    if let Some(item) = settings
        .inbox_accounts
        .iter_mut()
        .find(|item| item.id == account.id)
    {
        *item = account;
    } else {
        settings.inbox_accounts.push(account);
    }
    Ok(())
}

impl crate::Core {
    pub async fn save_inbox_account(
        &self,
        account: InboxAccount,
    ) -> crate::error::Result<Settings> {
        if let Some(icon) = account.icon.as_deref() {
            if icon.starts_with("profile-logos/") {
                if crate::brand_registry::safe_logo_relative(icon).is_none() {
                    return Err(crate::error::CoreError::Other(
                        "Invalid account icon".into(),
                    ));
                }
            } else {
                crate::normalize_identity_icon(Some(icon.to_owned()), "account-icons/")?;
            }
        }
        self.db
            .write(move |conn| {
                let mut settings = crate::db::repo::settings::get(conn)?;
                let accounts = crate::db::repo::accounts::list(conn)?
                    .into_iter()
                    .map(|a| RegistryAccount {
                        id: a.id,
                        email: a.email,
                    })
                    .collect::<Vec<_>>();
                save(&mut settings, account, &accounts).map_err(crate::error::CoreError::Other)?;
                crate::db::repo::settings::set(conn, &settings)?;
                Ok(settings)
            })
            .await
    }

    pub async fn delete_inbox_account(&self, id: i64) -> crate::error::Result<Settings> {
        self.db
            .write(move |conn| {
                let mut settings = crate::db::repo::settings::get(conn)?;
                // Materialize inherited tab selections before removing a named account.
                for tab in &settings.business_tabs {
                    let mut ids = selected_for_tab(&settings, tab);
                    if !ids.contains(&id) {
                        continue;
                    }
                    ids.retain(|value| *value != id);
                    settings.business_tab_accounts.insert(tab.id.clone(), ids);
                }
                settings.inbox_accounts.retain(|account| account.id != id);
                crate::db::repo::settings::set(conn, &settings)?;
                Ok(settings)
            })
            .await
    }

    pub async fn save_account_tab(
        &self,
        id: Option<String>,
        name: String,
        icon: Option<String>,
        show_name: bool,
        members: Option<Vec<i64>>,
    ) -> crate::error::Result<Settings> {
        let name = crate::normalize_mail_profile_name(&name)?;
        let icon = crate::normalize_identity_icon(icon, "tab-icons/")?;
        self.db
            .write(move |conn| {
                use crate::{db::repo, error::CoreError};
                let mut settings = repo::settings::get(conn)?;
                let accounts = repo::accounts::list(conn)?
                    .into_iter()
                    .map(|a| RegistryAccount {
                        id: a.id,
                        email: a.email,
                    })
                    .collect::<Vec<_>>();
                initialize(&mut settings, &accounts);
                if settings
                    .business_tabs
                    .iter()
                    .any(|tab| Some(&tab.id) != id.as_ref() && tab.name.eq_ignore_ascii_case(&name))
                {
                    return Err(CoreError::Other(
                        "A tab with this name already exists".into(),
                    ));
                }
                if let Some(ids) = &members
                    && ids.iter().any(|id| {
                        !settings
                            .inbox_accounts
                            .iter()
                            .any(|account| account.id == *id)
                    })
                {
                    return Err(CoreError::Other(
                        "A selected account no longer exists".into(),
                    ));
                }
                let id = if let Some(id) = id {
                    let tab = settings
                        .business_tabs
                        .iter_mut()
                        .find(|tab| tab.id == id)
                        .ok_or_else(|| CoreError::NotFound("Business tab".into()))?;
                    tab.name = name;
                    tab.icon = icon;
                    tab.show_name = show_name;
                    id
                } else {
                    let mut id = format!("tab-{}", settings.business_tabs.len() + 1);
                    while settings.business_tabs.iter().any(|tab| tab.id == id) {
                        id.push('x');
                    }
                    settings.business_tabs.push(crate::models::BusinessTab {
                        id: id.clone(),
                        name,
                        icon,
                        show_name,
                        account_ids: Some(vec![]),
                        address_view_ids: Some(vec![]),
                        profile_ids: vec![],
                        recipient_patterns: vec![],
                        scope: "profiles".into(),
                        sender_addresses: vec![],
                        sender_names: vec![],
                        sender_domains: vec![],
                    });
                    id
                };
                if let Some(mut ids) = members {
                    ids.sort_unstable();
                    ids.dedup();
                    settings.business_tab_accounts.insert(id, ids);
                }
                settings.business_tabs_customized = true;
                settings.business_tab_icons_initialized = true;
                repo::settings::set(conn, &settings)?;
                Ok(settings)
            })
            .await
    }
}
