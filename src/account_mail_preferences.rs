//! Native account signature and OpenPGP controls.
use super::*;
use flectar_mail_core::{
    Core,
    compose_senders::{self, ComposeAccount, ComposeGroup},
    mail_security::{MailSecurity, OpenedMessage},
    models::{
        AccountConfig, MailProtocol, SenderIdentity, SenderIdentitySource, Settings, Signature,
        SignatureDefaults,
    },
    sender_identities::{ManualIdentityPolicy, manual_identity_policy},
};
use slint::SharedString;
use std::{
    cell::Cell,
    collections::HashMap,
    future::Future,
};

/// Async work is owned by a profile/account generation. A removed account or
/// replaced core must never receive a late settings or decrypted-content result.
#[derive(Clone)]
struct UiWork {
    runtime: Rc<tokio::runtime::Runtime>,
    generation: Rc<Cell<u64>>,
}
impl UiWork {
    fn spawn<T: Send + 'static>(
        &self,
        app: &AppWindow,
        busy: bool,
        work: impl Future<Output = Result<T, String>> + Send + 'static,
        apply: impl FnOnce(AppWindow, Result<T, String>) + 'static,
    ) {
        if busy {
            app.global::<AccountMailPreferences>().set_busy(true);
        }
        let weak = app.as_weak();
        let generation = self.generation.clone();
        let expected = generation.get();
        let task = self.runtime.spawn(work);
        let abort = task.abort_handle();
        if let Err(error) = slint::spawn_local(async move {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            if generation.get() != expected {
                return;
            }
            if let Some(app) = weak.upgrade() {
                if busy {
                    app.global::<AccountMailPreferences>().set_busy(false);
                }
                apply(app, result);
            }
        }) {
            abort.abort();
            let ui = app.global::<AccountMailPreferences>();
            if busy {
                ui.set_busy(false);
            }
            ui.set_status(error.to_string().into());
        }
    }
}

async fn preferences(
    core: &Core,
) -> Result<(Settings, Vec<flectar_mail_core::models::AccountConfig>), String> {
    let settings = core.get_settings().await.map_err(|e| e.to_string())?;
    let configs = core
        .list_account_configs()
        .await
        .map_err(|e| e.to_string())?;
    Ok((settings, configs))
}

fn core(state: &Rc<RefCell<InboxState>>) -> Option<Arc<Core>> {
    state
        .borrow()
        .core
        .as_ref()
        .map(|s| s.account_preferences_core())
}
fn status(app: &AppWindow, result: Result<(), String>, success: &str) {
    app.global::<AccountMailPreferences>()
        .set_status(match result {
            Ok(()) => success.into(),
            Err(e) => e.into(),
        });
}
fn choices(settings: &Settings, account_id: i64) -> Vec<Signature> {
    settings
        .signature_list
        .iter()
        .filter(|s| s.account_id == account_id)
        .cloned()
        .collect()
}
fn project(app: &AppWindow, settings: &Settings, account_id: i64) {
    let ui = app.global::<AccountMailPreferences>();
    let signatures = choices(settings, account_id);
    ui.set_signatures(ModelRc::new(VecModel::from(
        signatures
            .iter()
            .map(|s| SignatureChoice {
                id: s.id.clone().into(),
                name: s.name.clone().into(),
                text: flectar_mail_core::signatures::plain_text(s).into(),
            })
            .collect::<Vec<_>>(),
    )));
    ui.set_signature_names(ModelRc::new(VecModel::from(
        signatures
            .iter()
            .map(|s| SharedString::from(&s.name))
            .collect::<Vec<_>>(),
    )));
    let mut names = vec![SharedString::from("No signature")];
    names.extend(signatures.iter().map(|s| SharedString::from(&s.name)));
    ui.set_default_names(ModelRc::new(VecModel::from(names)));
    let defaults = settings
        .signature_defaults
        .get(&account_id.to_string())
        .cloned()
        .unwrap_or_default();
    let index = |id: Option<String>| {
        signatures
            .iter()
            .position(|s| Some(&s.id) == id.as_ref())
            .map_or(0, |i| i as i32 + 1)
    };
    ui.set_new_index(index(defaults.new_id));
    ui.set_reply_index(index(defaults.reply_id));
}

struct IdentityChoices {
    all: RefCell<Vec<SenderIdentityChoice>>,
    matched_emails: RefCell<Vec<String>>,
    /// Last typed From display name per sender email (lowercase).
    last_display_names: RefCell<HashMap<String, String>>,
}

fn sender_query_matches(email: &str, label: &str, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let query = query.to_lowercase();
    email.to_lowercase().contains(&query) || label.to_lowercase().contains(&query)
}

fn publish_sender_matches(
    choices: &[SenderIdentityChoice],
    query: &str,
    matched_emails: &RefCell<Vec<String>>,
) -> Vec<SharedString> {
    let mut labels = Vec::new();
    let mut emails = Vec::new();
    for choice in choices {
        if sender_query_matches(&choice.email, &choice.label, query) {
            labels.push(choice.label.clone());
            emails.push(choice.email.to_string());
        }
    }
    *matched_emails.borrow_mut() = emails;
    labels
}

fn tim_mailbox_ids(settings: &Settings) -> Vec<i64> {
    let mut ids = Vec::new();
    for account in &settings.inbox_accounts {
        let tim = account.profile_id.as_deref() == Some("tim-personal")
            || account.name.eq_ignore_ascii_case("Tim Mentink");
        if tim {
            ids.extend(account.account_ids.iter().copied());
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

async fn load_compose_groups(core: &Core) -> Result<(Vec<ComposeGroup>, Option<String>), String> {
    let (settings, configs) = preferences(core).await?;
    let mut accounts = Vec::new();
    for config in configs {
        if config.mail_protocol == MailProtocol::Local || config.email.trim().is_empty() {
            continue;
        }
        let Ok(identities) = core.list_sender_identities(config.id).await else {
            continue;
        };
        accounts.push(ComposeAccount {
            id: config.id,
            email: config.email,
            profile_id: None,
            identities,
        });
    }
    let tim_ids = tim_mailbox_ids(&settings);
    let groups = compose_senders::compose_groups(
        &settings.business_tabs,
        &settings.address_views,
        &settings.address_rules,
        &accounts,
        &settings.mail_profiles,
        &tim_ids,
    );
    Ok((groups, settings.active_business_tab))
}

fn publish_group_senders(
    app: &AppWindow,
    group: &ComposeGroup,
    requested: &str,
    picker: &IdentityChoices,
) {
    let ui = app.global::<AccountMailPreferences>();
    let choices = group
        .senders
        .iter()
        .map(|sender| sender_choice(&sender.email, sender.display_name.as_deref()))
        .collect::<Vec<_>>();
    ui.set_composer_sender_names(ModelRc::new(VecModel::from(
        choices
            .iter()
            .map(|choice| choice.label.clone())
            .collect::<Vec<_>>(),
    )));
    ui.set_composer_sender_identities(ModelRc::new(VecModel::from(choices.clone())));
    let labels = publish_sender_matches(&choices, "", &picker.matched_emails);
    *picker.all.borrow_mut() = choices;
    ui.set_composer_sender_match_names(ModelRc::new(VecModel::from(labels)));
    ui.set_composer_sender_query("".into());
    let index = (!requested.is_empty())
        .then(|| {
            group
                .senders
                .iter()
                .position(|sender| compose_senders::same_sender_email(&sender.email, requested))
        })
        .flatten()
        .unwrap_or(0);
    let Some(sender) = group.senders.get(index) else {
        ui.set_composer_sender_index(0);
        ui.set_composer_sender_email("".into());
        ui.set_composer_sender_label("".into());
        ui.set_composer_sender_display_name("".into());
        ui.set_composer_preferences_ready(true);
        return;
    };
    ui.set_composer_sender_index(i32::try_from(index).unwrap_or(0));
    ui.set_composer_sender_email(sender.email.clone().into());
    ui.set_composer_sender_label(
        picker
            .all
            .borrow()
            .get(index)
            .map(|choice| choice.label.clone())
            .unwrap_or_else(|| sender.email.clone().into()),
    );
    let default_name = picker
        .all
        .borrow()
        .get(index)
        .map(|choice| choice.display_name.to_string())
        .unwrap_or_default();
    apply_composer_display_name(&ui, picker, &sender.email, &default_name);
    let Ok(account_id) = i32::try_from(sender.account_id) else {
        ui.set_composer_preferences_ready(true);
        return;
    };
    if app.get_compose_account_id() != account_id {
        app.set_compose_account_id(account_id);
        ui.invoke_composer_account_changed(account_id);
    } else {
        ui.set_composer_preferences_ready(true);
    }
}

fn apply_transport_account(app: &AppWindow, groups: &[ComposeGroup], email: &str) {
    let ui = app.global::<AccountMailPreferences>();
    let Some(group) = usize::try_from(ui.get_composer_group_index())
        .ok()
        .and_then(|index| groups.get(index))
    else {
        return;
    };
    let Some(account_id) = compose_senders::sender_account_id(group, email) else {
        return;
    };
    let Ok(account_id) = i32::try_from(account_id) else {
        return;
    };
    if app.get_compose_account_id() != account_id {
        app.set_compose_account_id(account_id);
        ui.invoke_composer_account_changed(account_id);
    }
}

fn sender_choice(email: &str, display_name: Option<&str>) -> SenderIdentityChoice {
    let name = display_name
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case(email));
    SenderIdentityChoice {
        email: email.into(),
        display_name: name.unwrap_or("").into(),
        label: match name {
            Some(name) => format!("{name}  <{email}>").into(),
            None => email.into(),
        },
    }
}

fn remember_composer_display_name(ui: &AccountMailPreferences, picker: &IdentityChoices) {
    let email = ui.get_composer_sender_email().to_string();
    if email.trim().is_empty() {
        return;
    }
    let name = ui.get_composer_sender_display_name().to_string();
    picker
        .last_display_names
        .borrow_mut()
        .insert(email.to_ascii_lowercase(), name);
}

fn apply_composer_display_name(
    ui: &AccountMailPreferences,
    picker: &IdentityChoices,
    email: &str,
    default_name: &str,
) {
    let key = email.to_ascii_lowercase();
    let name = picker
        .last_display_names
        .borrow()
        .get(&key)
        .cloned()
        .unwrap_or_else(|| default_name.to_owned());
    ui.set_composer_sender_display_name(name.into());
}

/// Concrete addresses that may be offered as From. Wildcard identities are
/// authorizations shown in settings, not addresses.
fn verified_identities(identities: &[SenderIdentity]) -> Vec<SenderIdentity> {
    identities
        .iter()
        .filter(|identity| identity.is_selectable())
        .cloned()
        .collect()
}

fn identity_rows(identities: &[SenderIdentity]) -> Vec<SenderIdentityRow> {
    identities
        .iter()
        .map(|identity| SenderIdentityRow {
            email: identity.email.clone().into(),
            name: identity
                .display_name
                .as_deref()
                .filter(|name| !name.trim().eq_ignore_ascii_case(&identity.email))
                .unwrap_or_default()
                .into(),
            reply_to: identity.reply_to_email.clone().unwrap_or_default().into(),
            primary: identity.is_primary,
            manual: identity.source == SenderIdentitySource::Manual,
            pending: !identity.is_verified(),
            wildcard: identity.is_wildcard(),
            delivery: identity.delivery_status.as_str().into(),
            error: identity.delivery_error.clone().unwrap_or_default().into(),
        })
        .collect()
}

fn policy_name(config: &AccountConfig) -> &'static str {
    match manual_identity_policy(config) {
        ManualIdentityPolicy::Any => "any",
        ManualIdentityPolicy::WithinWildcard => "wildcard",
        ManualIdentityPolicy::Never => "never",
    }
}

fn clear_sender_form(ui: &AccountMailPreferences<'_>) {
    ui.set_sender_form_email("".into());
    ui.set_sender_form_name("".into());
    ui.set_sender_form_reply_to("".into());
    ui.set_sender_form_editing(false);
}

fn project_identities(
    app: &AppWindow,
    config: &AccountConfig,
    identities: &[SenderIdentity],
    refresh_warning: Option<&str>,
    picker: &IdentityChoices,
) {
    let ui = app.global::<AccountMailPreferences>();
    let verified = verified_identities(identities);
    let choices = verified
        .iter()
        .map(|identity| sender_choice(&identity.email, identity.display_name.as_deref()))
        .collect::<Vec<_>>();
    ui.set_sender_identity_names(ModelRc::new(VecModel::from(
        choices
            .iter()
            .map(|identity| identity.label.clone())
            .collect::<Vec<_>>(),
    )));
    ui.set_sender_identities(ModelRc::new(VecModel::from(choices.clone())));
    let labels = publish_sender_matches(&choices, "", &picker.matched_emails);
    *picker.all.borrow_mut() = choices;
    ui.set_default_sender_match_names(ModelRc::new(VecModel::from(labels)));
    ui.set_default_sender_query("".into());
    ui.set_sender_list_query("".into());
    ui.set_sender_identity_rows(ModelRc::new(VecModel::from(identity_rows(identities))));
    ui.set_sender_identity_policy(policy_name(config).into());
    let configured = config.settings.default_sender_email.as_deref();
    let index = configured
        .and_then(|email| {
            verified
                .iter()
                .position(|identity| identity.email.eq_ignore_ascii_case(email))
        })
        .or_else(|| {
            verified
                .iter()
                .position(|identity| identity.is_provider_default)
        })
        .or_else(|| verified.iter().position(|identity| identity.is_primary))
        .unwrap_or(0);
    ui.set_default_sender_index(i32::try_from(index).unwrap_or(0));
    ui.set_default_sender_label(
        picker
            .all
            .borrow()
            .get(index)
            .map(|choice| choice.label.clone())
            .unwrap_or_default(),
    );
    let status = if let Some(warning) = refresh_warning {
        format!("Using cached identities: {warning}")
    } else {
        String::new()
    };
    ui.set_sender_identity_status(status.into());
}

/// Re-read identities after a local edit without another provider refresh.
async fn local_identities(
    core: &Core,
    account_id: i64,
) -> Result<(AccountConfig, Vec<SenderIdentity>), String> {
    let identities = core
        .list_sender_identities(account_id)
        .await
        .map_err(|error| error.to_string())?;
    let config = core
        .list_account_configs()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|config| config.id == account_id)
        .ok_or_else(|| "the account is no longer connected".to_owned())?;
    Ok((config, identities))
}

async fn refreshed_preferences(
    core: &Core,
    account_id: i64,
) -> Result<
    (
        Settings,
        Vec<AccountConfig>,
        Vec<SenderIdentity>,
        Option<String>,
    ),
    String,
> {
    let (identities, warning) = match core.refresh_sender_identities(account_id).await {
        Ok(identities) => (identities, None),
        Err(error) => (
            core.list_sender_identities(account_id)
                .await
                .map_err(|cached_error| cached_error.to_string())?,
            Some(error.to_string()),
        ),
    };
    let (settings, configs) = preferences(core).await?;
    Ok((settings, configs, identities, warning))
}

/// The composer reads saved identities so opening it never waits on the
/// network. Account sync keeps them current and submission re-validates the
/// chosen From address.
async fn cached_preferences(
    core: &Core,
    account_id: i64,
) -> Result<(Settings, Vec<AccountConfig>, Vec<SenderIdentity>), String> {
    let identities = core
        .list_sender_identities(account_id)
        .await
        .map_err(|error| error.to_string())?;
    let (settings, configs) = preferences(core).await?;
    Ok((settings, configs, identities))
}

pub(super) fn register(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Rc<tokio::runtime::Runtime>,
    document: &Rc<RefCell<RichComposeDocument>>,
    editor: &Rc<RefCell<LazyComposeEditor>>,
) {
    let ui = app.global::<AccountMailPreferences>();
    ui.set_desktop_supported(!cfg!(mobile));
    let tasks = UiWork {
        runtime: runtime.clone(),
        generation: Rc::new(Cell::new(0)),
    };
    let default_picker = Rc::new(IdentityChoices {
        all: RefCell::new(Vec::new()),
        matched_emails: RefCell::new(Vec::new()),
        last_display_names: RefCell::new(HashMap::new()),
    });
    let context_state = state.clone();
    let context_generation = tasks.generation.clone();
    let context = Rc::new(RefCell::new((
        core(state),
        state
            .borrow()
            .connected_accounts
            .iter()
            .map(|a| a.id)
            .collect::<Vec<_>>(),
    )));
    let weak = app.as_weak();
    let picker = default_picker.clone();
    ui.on_context_changed(move || {
        let current_core = core(&context_state);
        let accounts = context_state
            .borrow()
            .connected_accounts
            .iter()
            .map(|a| a.id)
            .collect::<Vec<_>>();
        let mut previous = context.borrow_mut();
        let same_core = match (&previous.0, &current_core) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same_core && previous.1 == accounts {
            return;
        }
        *previous = (current_core, accounts);
        context_generation.set(context_generation.get().wrapping_add(1));
        let Some(app) = weak.upgrade() else {
            return;
        };
        let ui = app.global::<AccountMailPreferences>();
        ui.set_busy(false);
        ui.set_account_id(-1);
        ui.set_account_label("".into());
        ui.set_signatures(ModelRc::default());
        ui.set_signature_names(ModelRc::default());
        ui.set_default_names(ModelRc::default());
        ui.set_sender_identities(ModelRc::default());
        ui.set_sender_identity_names(ModelRc::default());
        ui.set_default_sender_match_names(ModelRc::default());
        ui.set_default_sender_label("".into());
        ui.set_default_sender_query("".into());
        ui.set_sender_list_query("".into());
        picker.all.borrow_mut().clear();
        picker.matched_emails.borrow_mut().clear();
        ui.set_default_sender_index(0);
        ui.set_sender_identity_status("".into());
        ui.set_sender_identity_rows(ModelRc::default());
        ui.set_sender_identity_policy("never".into());
        clear_sender_form(&ui);
        ui.invoke_edit_signature(-1);
        ui.set_fingerprint("".into());
        ui.set_recipient_keys("".into());
        ui.set_key_inventory("".into());
        ui.set_status("".into());
        ui.invoke_close_reader();
        ui.invoke_composer_reset();
    });
    let weak = app.as_weak();
    let state_load = state.clone();
    let work = tasks.clone();
    let picker = default_picker.clone();
    ui.on_load(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let Some(core) = core(&state_load) else {
            return;
        };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() {
            return;
        }
        let picker = picker.clone();
        work.spawn(
            &app,
            true,
            async move { refreshed_preferences(&core, i64::from(id)).await },
            move |app, result| {
                let ui = app.global::<AccountMailPreferences>();
                match result {
                    Ok((settings, configs, identities, warning)) => {
                        if let Some(config) = configs.iter().find(|c| c.id == i64::from(id)) {
                            ui.set_account_id(id);
                            ui.set_account_label(config.email.clone().into());
                            ui.set_status("".into());
                            ui.set_key_inventory("".into());
                            project(&app, &settings, config.id);
                            clear_sender_form(&ui);
                            project_identities(
                                &app,
                                config,
                                &identities,
                                warning.as_deref(),
                                &picker,
                            );
                            ui.invoke_edit_signature(-1);
                            let p = &config.settings.security;
                            ui.set_fingerprint(p.signing_fingerprint.clone().into());
                            ui.set_sign(p.sign_by_default);
                            ui.set_encrypt(p.require_encryption);
                            ui.set_recipient_keys(
                                p.recipient_keys
                                    .iter()
                                    .map(|(e, k)| format!("{e}={k}"))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                                    .into(),
                            );
                        }
                    }
                    Err(e) => ui.set_status(e.into()),
                }
            },
        );
    });
    let weak = app.as_weak();
    let state_refresh_identities = state.clone();
    let work = tasks.clone();
    let picker = default_picker.clone();
    ui.on_refresh_sender_identities(move || {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() || ui.get_account_id() <= 0 {
            return;
        }
        let account_id = i64::from(ui.get_account_id());
        let Some(core) = core(&state_refresh_identities) else {
            return;
        };
        let picker = picker.clone();
        work.spawn(
            &app,
            true,
            async move { refreshed_preferences(&core, account_id).await },
            move |app, result| match result {
                Ok((_settings, configs, identities, warning)) => {
                    if let Some(config) = configs.iter().find(|config| config.id == account_id) {
                        project_identities(&app, config, &identities, warning.as_deref(), &picker);
                        app.global::<AccountMailPreferences>()
                            .set_status("Sender identities refreshed.".into());
                    }
                }
                Err(error) => app
                    .global::<AccountMailPreferences>()
                    .set_status(error.into()),
            },
        );
    });
    let weak = app.as_weak();
    let picker = default_picker.clone();
    ui.on_filter_default_senders(move |query| {
        let Some(app) = weak.upgrade() else { return };
        let labels = publish_sender_matches(&picker.all.borrow(), &query, &picker.matched_emails);
        app.global::<AccountMailPreferences>()
            .set_default_sender_match_names(ModelRc::new(VecModel::from(labels)));
    });
    let weak = app.as_weak();
    let picker = default_picker.clone();
    ui.on_pick_default_sender(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        let Some(email) = usize::try_from(index)
            .ok()
            .and_then(|index| picker.matched_emails.borrow().get(index).cloned())
        else {
            return;
        };
        let choices = picker.all.borrow();
        let Some(position) = choices
            .iter()
            .position(|choice| choice.email.eq_ignore_ascii_case(&email))
        else {
            return;
        };
        ui.set_default_sender_index(i32::try_from(position).unwrap_or(0));
        ui.set_default_sender_label(choices[position].label.clone());
    });
    let weak = app.as_weak();
    let state_sender_default = state.clone();
    let work = tasks.clone();
    ui.on_save_default_sender(move || {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() || ui.get_account_id() <= 0 {
            return;
        }
        let Some(identity) = usize::try_from(ui.get_default_sender_index())
            .ok()
            .and_then(|index| ui.get_sender_identities().row_data(index))
        else {
            ui.set_status("Choose a verified sender identity.".into());
            return;
        };
        let account_id = i64::from(ui.get_account_id());
        let Some(core) = core(&state_sender_default) else {
            return;
        };
        work.spawn(
            &app,
            true,
            async move {
                core.set_default_sender_identity(account_id, identity.email.to_string())
                    .await
                    .map_err(|error| error.to_string())
            },
            |app, result| status(&app, result, "Default sender identity saved."),
        );
    });
    let weak = app.as_weak();
    let state_save_sender = state.clone();
    let work = tasks.clone();
    let picker = default_picker.clone();
    ui.on_save_sender_identity(move || {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() || ui.get_account_id() <= 0 {
            return;
        }
        let account_id = i64::from(ui.get_account_id());
        let Some(core) = core(&state_save_sender) else {
            return;
        };
        let email = ui.get_sender_form_email().to_string();
        let optional = |value: SharedString| {
            let value = value.trim().to_owned();
            (!value.is_empty()).then_some(value)
        };
        let name = optional(ui.get_sender_form_name());
        let reply_to = optional(ui.get_sender_form_reply_to());
        let editing = ui.get_sender_form_editing();
        let picker = picker.clone();
        work.spawn(
            &app,
            true,
            async move {
                core.save_sender_identity(account_id, email, name, reply_to)
                    .await
                    .map_err(|error| error.to_string())?;
                local_identities(&core, account_id).await
            },
            move |app, result| {
                let ui = app.global::<AccountMailPreferences>();
                match result {
                    Ok((config, identities)) => {
                        clear_sender_form(&ui);
                        project_identities(&app, &config, &identities, None, &picker);
                        ui.set_status(
                            if editing {
                                "Sender address saved."
                            } else {
                                "Sender address added. It can now be chosen in the composer."
                            }
                            .into(),
                        );
                    }
                    Err(error) => ui.set_status(error.into()),
                }
            },
        );
    });
    let weak = app.as_weak();
    ui.on_edit_sender_identity(move |email| {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        let Some(row) = ui
            .get_sender_identity_rows()
            .iter()
            .find(|row| row.manual && row.email == email)
        else {
            return;
        };
        ui.set_sender_form_email(row.email);
        ui.set_sender_form_name(row.name);
        ui.set_sender_form_reply_to(row.reply_to);
        ui.set_sender_form_editing(true);
    });
    let weak = app.as_weak();
    ui.on_clear_sender_form(move || {
        if let Some(app) = weak.upgrade() {
            clear_sender_form(&app.global::<AccountMailPreferences>());
        }
    });
    let weak = app.as_weak();
    let state_remove_sender = state.clone();
    let work = tasks.clone();
    let picker = default_picker.clone();
    ui.on_remove_sender_identity(move |email| {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() || ui.get_account_id() <= 0 {
            return;
        }
        let account_id = i64::from(ui.get_account_id());
        let Some(core) = core(&state_remove_sender) else {
            return;
        };
        let email = email.to_string();
        let picker = picker.clone();
        work.spawn(
            &app,
            true,
            async move {
                core.remove_sender_identity(account_id, email)
                    .await
                    .map_err(|error| error.to_string())?;
                local_identities(&core, account_id).await
            },
            move |app, result| {
                let ui = app.global::<AccountMailPreferences>();
                match result {
                    Ok((config, identities)) => {
                        if ui.get_sender_form_editing() {
                            clear_sender_form(&ui);
                        }
                        project_identities(&app, &config, &identities, None, &picker);
                        ui.set_status("Sender address removed.".into());
                    }
                    Err(error) => ui.set_status(error.into()),
                }
            },
        );
    });
    let weak = app.as_weak();
    ui.on_edit_signature(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        let signature = usize::try_from(index)
            .ok()
            .and_then(|i| ui.get_signatures().row_data(i));
        ui.set_editing_index(index);
        ui.set_editing_id(signature.as_ref().map(|s| s.id.clone()).unwrap_or_default());
        ui.set_signature_name(
            signature
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_default(),
        );
        ui.set_signature_text(signature.map(|s| s.text).unwrap_or_default());
    });
    let weak = app.as_weak();
    let state_save = state.clone();
    let work = tasks.clone();
    ui.on_save_signature(move || {
        let Some(app) = weak.upgrade() else { return };
        let Some(core) = core(&state_save) else {
            return;
        };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() {
            return;
        }
        let signature = Signature {
            id: ui.get_editing_id().to_string(),
            account_id: i64::from(ui.get_account_id()),
            name: ui.get_signature_name().to_string(),
            html: flectar_mail_core::signatures::text_html(ui.get_signature_text().as_str()),
        };
        work.spawn(
            &app,
            true,
            async move {
                let saved = core
                    .save_signature(signature)
                    .await
                    .map_err(|e| e.to_string())?;
                let settings = core.get_settings().await.map_err(|e| e.to_string())?;
                Ok((saved, settings))
            },
            |app, result| {
                let ui = app.global::<AccountMailPreferences>();
                match result {
                    Ok((saved, settings)) => {
                        project(&app, &settings, saved.account_id);
                        let index = choices(&settings, saved.account_id)
                            .iter()
                            .position(|s| s.id == saved.id)
                            .unwrap_or(0);
                        ui.invoke_edit_signature(index as i32);
                        ui.set_status("Signature saved.".into());
                    }
                    Err(e) => ui.set_status(e.into()),
                }
            },
        );
    });
    let weak = app.as_weak();
    let state_delete = state.clone();
    let work = tasks.clone();
    ui.on_delete_signature(move || {
        let Some(app) = weak.upgrade() else { return };
        let Some(core) = core(&state_delete) else {
            return;
        };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() {
            return;
        }
        let id = i64::from(ui.get_account_id());
        let signature = ui.get_editing_id().to_string();
        work.spawn(
            &app,
            true,
            async move {
                core.delete_signature(id, signature)
                    .await
                    .map_err(|e| e.to_string())?;
                core.get_settings().await.map_err(|e| e.to_string())
            },
            move |app, result| match result {
                Ok(settings) => {
                    project(&app, &settings, id);
                    app.global::<AccountMailPreferences>()
                        .invoke_edit_signature(-1);
                    status(
                        &app,
                        Ok(()),
                        "Signature deleted; affected defaults cleared.",
                    );
                }
                Err(e) => status(&app, Err(e), ""),
            },
        );
    });
    let weak = app.as_weak();
    let state_defaults = state.clone();
    let work = tasks.clone();
    ui.on_save_defaults(move || {
        let Some(app) = weak.upgrade() else { return };
        let Some(core) = core(&state_defaults) else {
            return;
        };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() {
            return;
        }
        let id = |index: i32| {
            usize::try_from(index - 1)
                .ok()
                .and_then(|i| ui.get_signatures().row_data(i))
                .map(|s| s.id.to_string())
        };
        let account = i64::from(ui.get_account_id());
        let defaults = SignatureDefaults {
            new_id: id(ui.get_new_index()),
            reply_id: id(ui.get_reply_index()),
        };
        work.spawn(
            &app,
            true,
            async move {
                core.set_signature_defaults(account, defaults)
                    .await
                    .map_err(|e| e.to_string())
            },
            |app, result| status(&app, result, "Signature defaults saved."),
        );
    });
    let weak = app.as_weak();
    let state_security = state.clone();
    let work = tasks.clone();
    ui.on_save_security(move || {
        let Some(app) = weak.upgrade() else { return };
        let Some(core) = core(&state_security) else {
            return;
        };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() {
            return;
        }
        let mut policy = MailSecurity {
            signing_fingerprint: ui.get_fingerprint().to_string(),
            sign_by_default: ui.get_sign(),
            require_encryption: ui.get_encrypt(),
            ..Default::default()
        };
        for line in ui
            .get_recipient_keys()
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            let Some((email, key)) = line.split_once('=') else {
                ui.set_status("Use one email=full-fingerprint per line.".into());
                return;
            };
            if policy
                .recipient_keys
                .insert(email.trim().to_lowercase(), key.trim().to_owned())
                .is_some()
            {
                ui.set_status("Remove duplicate recipient addresses.".into());
                return;
            }
        }
        let id = i64::from(ui.get_account_id());
        work.spawn(
            &app,
            true,
            async move {
                core.set_mail_security(id, policy)
                    .await
                    .map_err(|e| e.to_string())
            },
            |app, result| status(&app, result, "OpenPGP settings saved."),
        );
    });
    register_keys_and_reader(app, state, &tasks);
    register_composer(app, state, &tasks, document, editor);
}

fn show_message(app: &AppWindow, message: OpenedMessage, memory: &Arc<std::sync::Mutex<Vec<u8>>>) {
    *memory.lock().unwrap() = message.mime;
    let ui = app.global::<AccountMailPreferences>();
    ui.set_reader_text(message.text.into());
    ui.set_reader_status(message.status.into());
    ui.set_reader_open(true);
}
fn register_keys_and_reader(app: &AppWindow, state: &Rc<RefCell<InboxState>>, tasks: &UiWork) {
    let memory = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let reader_generation = Rc::new(Cell::new(0_u64));
    let ui = app.global::<AccountMailPreferences>();
    let weak = app.as_weak();
    let work = tasks.clone();
    let opened = memory.clone();
    let generation = reader_generation.clone();
    ui.on_key_action(move |action| {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() || ui.get_account_id() < 0 { return; }
        let key = ui.get_fingerprint().to_string();
        let email = ui.get_account_label().to_string();
        let opened = opened.clone();
        let generation = generation.clone();
        let expected = generation.get();
        work.spawn(&app, true, async move { key_action(action.as_str(), &key, &email).await }, move |app, result| {
            let ui = app.global::<AccountMailPreferences>();
            match result {
                Ok(KeyResult::Generated(key)) => { ui.set_fingerprint(key.into()); ui.set_status("Key created. Back up your private key and revocation certificate in GnuPG, then save OpenPGP settings.".into()); }
                Ok(KeyResult::Inventory(text)) => {
                    ui.set_key_inventory(text.into());
                    ui.set_status("Keys loaded. Verify fingerprints before trusting recipient keys.".into());
                }
                Ok(KeyResult::Message(message)) if generation.get() == expected => show_message(&app, message, &opened),
                Ok(KeyResult::Message(_)) => {},
                Ok(KeyResult::Done(text)) => ui.set_status(text.into()),
                Err(e) => ui.set_status(e.into()),
            }
        });
    });
    let weak = app.as_weak();
    let work = tasks.clone();
    let state = state.clone();
    let opened = memory.clone();
    let generation = reader_generation.clone();
    ui.on_read_message(move || {
        let Some(app) = weak.upgrade() else { return };
        let Some(core) = core(&state) else { return };
        let Some((selected_id, target)) = ({
            let state = state.borrow();
            state
                .selected_id
                .and_then(|id| state.messages.iter().find(|m| m.id == id))
                .and_then(|m| crate::mail::MailTarget::of(m).map(|target| (m.id, target)))
        }) else {
            return;
        };
        let ui = app.global::<AccountMailPreferences>();
        if ui.get_busy() {
            return;
        }
        ui.set_reader_text("".into());
        ui.set_reader_status("Opening OpenPGP message…".into());
        ui.set_reader_open(true);
        *opened.lock().unwrap() = Vec::new();
        let opened = opened.clone();
        let state = state.clone();
        let generation = generation.clone();
        let expected = generation.get();
        work.spawn(
            &app,
            true,
            async move {
                // A conversation opens its newest message; a single message's
                // row, that message.
                let message_id = match target {
                    crate::mail::MailTarget::Message { id, .. } => id,
                    crate::mail::MailTarget::Thread(thread_id) => {
                        core.get_latest_thread_body(thread_id)
                            .await
                            .map_err(|e| e.to_string())?
                            .id
                    }
                };
                core.open_openpgp_message(message_id)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |app, result| {
                if generation.get() != expected || state.borrow().selected_id != Some(selected_id) {
                    return;
                }
                match result {
                    Ok(message) => show_message(&app, message, &opened),
                    Err(e) => app
                        .global::<AccountMailPreferences>()
                        .set_reader_status(e.into()),
                }
            },
        );
    });
    let memory_close = memory.clone();
    let weak = app.as_weak();
    let generation = reader_generation.clone();
    ui.on_close_reader(move || {
        generation.set(generation.get().wrapping_add(1));
        *memory_close.lock().unwrap() = Vec::new();
        if let Some(app) = weak.upgrade() {
            let ui = app.global::<AccountMailPreferences>();
            ui.set_reader_open(false);
            ui.set_reader_text("".into());
            ui.set_reader_status("".into());
        }
    });
    let weak = app.as_weak();
    let work = tasks.clone();
    ui.on_export_message(move || {
        let Some(app) = weak.upgrade() else { return };
        if app.global::<AccountMailPreferences>().get_busy() {
            return;
        }
        let data = memory.lock().unwrap().clone();
        if data.is_empty() {
            return;
        }
        let generation = reader_generation.clone();
        let expected = generation.get();
        work.spawn(&app, true, export_message(data), move |app, result| {
            if generation.get() == expected
                && let Err(e) = result
            {
                app.global::<AccountMailPreferences>()
                    .set_reader_status(e.into());
            }
        });
    });
}

// Mobile keeps the shared UI result interface, but key actions return an
// unsupported-platform error until a native OpenPGP backend is available.
#[cfg_attr(mobile, allow(dead_code))]
enum KeyResult {
    Generated(String),
    Inventory(String),
    Message(OpenedMessage),
    Done(String),
}
#[cfg(desktop)]
async fn key_action(action: &str, fingerprint: &str, email: &str) -> Result<KeyResult, String> {
    let gpg = flectar_mail_core::mail_security::Gpg::default();
    match action {
        "generate" => gpg
            .generate_key(email)
            .await
            .map(KeyResult::Generated)
            .map_err(|e| e.to_string()),
        "list" => {
            let secret = gpg.list_keys(true).await.map_err(|e| e.to_string())?;
            let public = gpg.list_keys(false).await.map_err(|e| e.to_string())?;
            let display = |listing: &str| {
                listing
                    .lines()
                    .filter_map(|line| {
                        let f: Vec<_> = line.split(':').collect();
                        match f.first().copied()? {
                            "pub" => Some("Public key".to_owned()),
                            "sec" => Some("Private key".to_owned()),
                            "fpr" => f.get(9).map(|s| format!("  {s}")),
                            "uid" => f.get(9).map(|s| {
                                format!("  {}", flectar_mail_core::mail_security::decode_colon(s))
                            }),
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(KeyResult::Inventory(format!(
                "{}\n\n{}",
                display(&secret),
                display(&public)
            )))
        }
        "import" | "open" => {
            let Some(file) = rfd::AsyncFileDialog::new()
                .set_title(if action == "import" {
                    "Import OpenPGP key"
                } else {
                    "Open OpenPGP message"
                })
                .pick_file()
                .await
            else {
                return Ok(KeyResult::Done("Cancelled.".into()));
            };
            let limit = if action == "import" {
                1024 * 1024
            } else {
                100 * 1024 * 1024
            };
            let input = tokio::fs::File::open(file.path())
                .await
                .map_err(|e| e.to_string())?;
            use tokio::io::AsyncReadExt;
            let mut data = Vec::new();
            input
                .take(limit + 1)
                .read_to_end(&mut data)
                .await
                .map_err(|e| e.to_string())?;
            if data.len() as u64 > limit {
                return Err("File exceeds the size limit.".into());
            }
            if action == "import" {
                gpg.import_key(&data).await.map_err(|e| e.to_string())?;
                Ok(KeyResult::Done(
                    "Key imported. Verify its fingerprint before trusting it.".into(),
                ))
            } else {
                gpg.open_message(&data, None)
                    .await
                    .map(KeyResult::Message)
                    .map_err(|e| e.to_string())
            }
        }
        "export" => {
            let data = gpg
                .export_public_key(fingerprint)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(file) = rfd::AsyncFileDialog::new()
                .set_file_name("public-key.asc")
                .save_file()
                .await
            {
                file.write(&data).await.map_err(|e| e.to_string())?;
                Ok(KeyResult::Done("Public key exported.".into()))
            } else {
                Ok(KeyResult::Done("Cancelled.".into()))
            }
        }
        _ => Err("Unknown key action.".into()),
    }
}
#[cfg(mobile)]
async fn key_action(_: &str, _: &str, _: &str) -> Result<KeyResult, String> {
    Err("OpenPGP requires desktop GnuPG.".into())
}
#[cfg(desktop)]
async fn export_message(data: Vec<u8>) -> Result<(), String> {
    if let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("Save decrypted message (contains private content)")
        .set_file_name("decrypted-message.eml")
        .save_file()
        .await
    {
        file.write(&data).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[cfg(mobile)]
async fn export_message(_: Vec<u8>) -> Result<(), String> {
    Err("Message export is unavailable on this platform.".into())
}

fn register_composer(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    tasks: &UiWork,
    document: &Rc<RefCell<RichComposeDocument>>,
    editor: &Rc<RefCell<LazyComposeEditor>>,
) {
    // Only replace the exact unedited block we inserted. User edits are never
    // discarded when switching identity or selecting a different signature.
    let inserted = Rc::new(RefCell::new(None::<String>));
    let available = Rc::new(RefCell::new(Vec::<Signature>::new()));
    let composer_picker = Rc::new(IdentityChoices {
        all: RefCell::new(Vec::new()),
        matched_emails: RefCell::new(Vec::new()),
        last_display_names: RefCell::new(HashMap::new()),
    });
    let ui = app.global::<AccountMailPreferences>();
    let reset = inserted.clone();
    let generation = Rc::new(Cell::new(0_u64));
    let compose_groups = Rc::new(RefCell::new(Vec::<ComposeGroup>::new()));
    let need_catalog = Rc::new(Cell::new(true));
    let catalog_generation = Rc::new(Cell::new(0_u64));
    let reset_generation = generation.clone();
    let reset_groups = compose_groups.clone();
    let reset_catalog = need_catalog.clone();
    let reset_catalog_generation = catalog_generation.clone();
    let reset_list = available.clone();
    let weak = app.as_weak();
    let picker = composer_picker.clone();
    ui.on_composer_reset(move || {
        reset_generation.set(reset_generation.get().wrapping_add(1));
        reset_catalog.set(true);
        reset_catalog_generation.set(reset_catalog_generation.get().wrapping_add(1));
        reset_groups.borrow_mut().clear();
        *reset.borrow_mut() = None;
        reset_list.borrow_mut().clear();
        if let Some(app) = weak.upgrade() {
            let ui = app.global::<AccountMailPreferences>();
            ui.set_composer_signature_names(ModelRc::default());
            ui.set_composer_signature_index(0);
            ui.set_composer_sender_identities(ModelRc::default());
            ui.set_composer_sender_names(ModelRc::default());
            ui.set_composer_sender_match_names(ModelRc::default());
            ui.set_composer_sender_label("".into());
            ui.set_composer_sender_query("".into());
            picker.all.borrow_mut().clear();
            picker.matched_emails.borrow_mut().clear();
            ui.set_composer_sender_index(0);
            ui.set_composer_sender_email("".into());
            ui.set_composer_group_names(ModelRc::default());
            ui.set_composer_group_index(0);
            ui.set_composer_security("".into());
            ui.set_composer_preferences_ready(false);
        }
    });
    let weak = app.as_weak();
    let state = state.clone();
    let work = tasks.clone();
    let list = available.clone();
    let managed = inserted.clone();
    let picker = composer_picker.clone();
    let groups_for_account = compose_groups.clone();
    let catalog_for_account = need_catalog.clone();
    let catalog_groups = compose_groups.clone();
    let catalog_generation_for_account = catalog_generation.clone();
    ui.on_composer_account_changed(move |account_id| {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        let grouped = !groups_for_account.borrow().is_empty();
        ui.set_composer_preferences_ready(false);
        ui.set_composer_signature_names(ModelRc::default());
        if !grouped {
            ui.set_composer_sender_identities(ModelRc::default());
            ui.set_composer_sender_names(ModelRc::default());
            ui.set_composer_sender_match_names(ModelRc::default());
            ui.set_composer_sender_label("".into());
            ui.set_composer_sender_query("".into());
        }
        ui.set_composer_security("".into());
        let Some(core) = core(&state) else { return };
        if catalog_for_account.get() {
            catalog_for_account.set(false);
            let catalog_groups = catalog_groups.clone();
            let catalog_picker = picker.clone();
            let retry_catalog = catalog_for_account.clone();
            let catalog_generation = catalog_generation_for_account.clone();
            let expected_catalog = catalog_generation.get();
            work.spawn(
                &app,
                false,
                {
                    let core = core.clone();
                    async move { load_compose_groups(&core).await }
                },
                move |app, result| {
                    if catalog_generation.get() != expected_catalog {
                        return;
                    }
                    match result {
                    Ok((groups, active)) => {
                        let requested = app
                            .global::<AccountMailPreferences>()
                            .get_composer_sender_email()
                            .to_string();
                        let index = compose_senders::compose_group_index(
                            &groups,
                            &requested,
                            active.as_deref(),
                        );
                        let ui = app.global::<AccountMailPreferences>();
                        if groups.is_empty() {
                            ui.set_composer_group_names(ModelRc::default());
                            ui.set_composer_group_index(0);
                            *catalog_groups.borrow_mut() = groups;
                            return;
                        }
                        ui.set_composer_group_names(ModelRc::new(VecModel::from(
                            groups
                                .iter()
                                .map(|group| SharedString::from(group.name.as_str()))
                                .collect::<Vec<_>>(),
                        )));
                        ui.set_composer_group_index(i32::try_from(index).unwrap_or(0));
                        let group = groups[index].clone();
                        *catalog_groups.borrow_mut() = groups;
                        publish_group_senders(&app, &group, &requested, &catalog_picker);
                    }
                    Err(error) => {
                        retry_catalog.set(true);
                        app.set_compose_notice(UiMessage::detail(
                            "Could not load account preferences: {}",
                            error,
                        ));
                        app.set_compose_notice_is_error(true);
                    }
                }
                },
            );
        }
        generation.set(generation.get().wrapping_add(1));
        let generation = generation.clone();
        let expected = generation.get();
        let list = list.clone();
        let managed = managed.clone();
        let picker = picker.clone();
        let sender_groups = groups_for_account.clone();
        work.spawn(
            &app,
            false,
            async move { cached_preferences(&core, i64::from(account_id)).await },
            move |app, result| {
                if generation.get() != expected || app.get_compose_account_id() != account_id {
                    return;
                }
                let ui = app.global::<AccountMailPreferences>();
                match result {
                    Ok((settings, configs, identities)) => {
                        let Some(config) = configs
                            .iter()
                            .find(|config| config.id == i64::from(account_id))
                        else {
                            return;
                        };
                        let use_account_identities = sender_groups.borrow().is_empty();
                        if use_account_identities {
                        let requested = ui.get_composer_sender_email().to_string();
                        let identities = {
                            let all = identities;
                            let mut selectable = compose_senders::unique_selectable_identities(
                                verified_identities(&all),
                            );
                            // A reply to an address under a server wildcard
                            // (`*@domain`) answers from that exact address.
                            if !requested.is_empty()
                                && !selectable.iter().any(|identity| {
                                    compose_senders::same_sender_email(&identity.email, &requested)
                                })
                                && let Some(wildcard) = all.iter().find(|identity| {
                                    identity.is_verified()
                                        && identity.is_wildcard()
                                        && identity.authorizes(&requested)
                                })
                            {
                                selectable.push(SenderIdentity {
                                    email: requested.trim().to_owned(),
                                    is_primary: false,
                                    is_provider_default: false,
                                    ..wildcard.clone()
                                });
                            }
                            selectable
                        };
                        let sender_choices = identities
                            .iter()
                            .map(|identity| {
                                sender_choice(&identity.email, identity.display_name.as_deref())
                            })
                            .collect::<Vec<_>>();
                        ui.set_composer_sender_names(ModelRc::new(VecModel::from(
                            sender_choices
                                .iter()
                                .map(|identity| identity.label.clone())
                                .collect::<Vec<_>>(),
                        )));
                        ui.set_composer_sender_identities(ModelRc::new(VecModel::from(
                            sender_choices.clone(),
                        )));
                        let labels =
                            publish_sender_matches(&sender_choices, "", &picker.matched_emails);
                        *picker.all.borrow_mut() = sender_choices;
                        ui.set_composer_sender_match_names(ModelRc::new(VecModel::from(labels)));
                        ui.set_composer_sender_query("".into());
                        let default_email = config.settings.default_sender_email.as_deref();
                        let sender_index = (!requested.is_empty())
                            .then(|| {
                                identities.iter().position(|identity| {
                                    compose_senders::same_sender_email(&identity.email, &requested)
                                })
                            })
                            .flatten()
                            .or_else(|| {
                                default_email.and_then(|email| {
                                    identities.iter().position(|identity| {
                                        compose_senders::same_sender_email(&identity.email, email)
                                    })
                                })
                            })
                            .or_else(|| {
                                identities
                                    .iter()
                                    .position(|identity| identity.is_provider_default)
                            })
                            .or_else(|| identities.iter().position(|identity| identity.is_primary))
                            .unwrap_or(0);
                        ui.set_composer_preferences_ready(true);
                        ui.invoke_composer_sender(i32::try_from(sender_index).unwrap_or(0));
                        } else {
                            ui.set_composer_preferences_ready(true);
                        }
                        let signatures = choices(&settings, i64::from(account_id));
                        let mut names = vec![SharedString::from("No signature")];
                        names.extend(signatures.iter().map(|s| SharedString::from(&s.name)));
                        ui.set_composer_signature_names(ModelRc::new(VecModel::from(names)));
                        *list.borrow_mut() = signatures.clone();
                        let p = &config.settings.security;
                        ui.set_composer_security(
                            if p.require_encryption {
                                if p.sign_by_default {
                                    "OpenPGP: encryption required · digitally signed"
                                } else {
                                    "OpenPGP: encryption required"
                                }
                            } else if p.sign_by_default {
                                "OpenPGP: digitally signed · not encrypted"
                            } else {
                                "Message is not end-to-end encrypted"
                            }
                            .into(),
                        );
                        if app.get_compose_mode() == "draft" {
                            *managed.borrow_mut() = None;
                            ui.set_composer_signature_index(0);
                            return;
                        }
                        // A cleared composer starts a fresh insertion lifecycle.
                        if app.get_compose_body().is_empty() {
                            *managed.borrow_mut() = None;
                        }
                        let defaults = settings
                            .signature_defaults
                            .get(&account_id.to_string())
                            .cloned()
                            .unwrap_or_default();
                        let id = if app.get_compose_mode() == "new" {
                            defaults.new_id
                        } else {
                            defaults.reply_id
                        };
                        let index = signatures
                            .iter()
                            .position(|s| Some(&s.id) == id.as_ref())
                            .map_or(0, |i| i as i32 + 1);
                        ui.invoke_composer_signature(index);
                    }
                    Err(e) => {
                        app.set_compose_notice(UiMessage::detail(
                            "Could not load account preferences: {}",
                            e,
                        ));
                        app.set_compose_notice_is_error(true);
                    }
                }
            },
        );
    });
    let weak = app.as_weak();
    let compose_groups_for_sender = compose_groups.clone();
    let picker_for_sender = composer_picker.clone();
    let groups_for_change = compose_groups.clone();
    let picker_for_change = composer_picker.clone();
    ui.on_composer_group_changed(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let groups = groups_for_change.borrow();
        let Some(group) = usize::try_from(index)
            .ok()
            .and_then(|index| groups.get(index))
            .cloned()
        else {
            return;
        };
        let ui = app.global::<AccountMailPreferences>();
        ui.set_composer_group_index(index);
        let current = ui.get_composer_sender_email().to_string();
        let requested = if group
            .senders
            .iter()
            .any(|sender| compose_senders::same_sender_email(&sender.email, &current))
        {
            current
        } else {
            String::new()
        };
        drop(groups);
        publish_group_senders(&app, &group, &requested, &picker_for_change);
    });
    let weak = app.as_weak();
    ui.on_composer_sender(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        let Some(identity) = usize::try_from(index)
            .ok()
            .and_then(|index| ui.get_composer_sender_identities().row_data(index))
        else {
            return;
        };
        remember_composer_display_name(&ui, &picker_for_sender);
        ui.set_composer_sender_index(index);
        ui.set_composer_sender_email(identity.email.clone());
        ui.set_composer_sender_label(identity.label.clone());
        apply_composer_display_name(
            &ui,
            &picker_for_sender,
            identity.email.as_str(),
            identity.display_name.as_str(),
        );
        apply_transport_account(&app, &compose_groups_for_sender.borrow(), identity.email.as_str());
    });
    let weak = app.as_weak();
    let picker = composer_picker.clone();
    ui.on_filter_composer_senders(move |query| {
        let Some(app) = weak.upgrade() else { return };
        let labels = publish_sender_matches(&picker.all.borrow(), &query, &picker.matched_emails);
        app.global::<AccountMailPreferences>()
            .set_composer_sender_match_names(ModelRc::new(VecModel::from(labels)));
    });
    let weak = app.as_weak();
    let picker = composer_picker.clone();
    let compose_groups_for_pick = compose_groups.clone();
    ui.on_pick_composer_sender(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let ui = app.global::<AccountMailPreferences>();
        let Some(email) = usize::try_from(index)
            .ok()
            .and_then(|index| picker.matched_emails.borrow().get(index).cloned())
        else {
            return;
        };
        let choices = picker.all.borrow();
        let Some(position) = choices
            .iter()
            .position(|choice| compose_senders::same_sender_email(&choice.email, &email))
        else {
            return;
        };
        remember_composer_display_name(&ui, &picker);
        let email = choices[position].email.clone();
        let label = choices[position].label.clone();
        let default_name = choices[position].display_name.to_string();
        ui.set_composer_sender_index(i32::try_from(position).unwrap_or(0));
        ui.set_composer_sender_email(email.clone());
        ui.set_composer_sender_label(label);
        apply_composer_display_name(&ui, &picker, email.as_str(), &default_name);
        apply_transport_account(
            &app,
            &compose_groups_for_pick.borrow(),
            email.as_str(),
        );
    });
    let weak = app.as_weak();
    let document = document.clone();
    let editor = editor.clone();
    ui.on_composer_signature(move |index| {
        let Some(app) = weak.upgrade() else { return }; let mut document = document.borrow_mut();
        let current = document.text().to_owned();
        let old = inserted.borrow().clone();
        let body = match remove_managed_signature(&current, old.as_deref()) {
            Ok(body) => body,
            Err(()) if index == 0 => {
                *inserted.borrow_mut() = None;
                app.global::<AccountMailPreferences>().set_composer_signature_index(0);
                return;
            }
            Err(()) => {
                app.set_compose_notice(UiMessage::plain("Your edited signature was preserved. Choose No signature to keep it as message text before inserting another."));
                app.set_compose_notice_is_error(true); return;
            }
        };
        let signature = usize::try_from(index - 1).ok().and_then(|i| available.borrow().get(i).cloned());
        let text = signature.as_ref().map(flectar_mail_core::signatures::plain_text).unwrap_or_default();
        let new_body = flectar_mail_core::signatures::insert(&body, &text);
        *inserted.borrow_mut() = if text.is_empty() { None } else { Some(format!("\n\n-- \n{}", text.trim())) };
        let selection = document.synchronize(&new_body, 0, 0);
        apply_rich_compose(&app, &document, selection, &mut editor.borrow_mut());
        app.global::<AccountMailPreferences>().set_composer_signature_index(index);
    });
}

fn remove_managed_signature(body: &str, managed: Option<&str>) -> Result<String, ()> {
    match managed {
        None => Ok(body.to_owned()),
        Some(block) if body.matches(block).count() == 1 => Ok(body.replacen(block, "", 1)),
        Some(_) if body.is_empty() => Ok(String::new()),
        Some(_) => Err(()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_search_matches_email_or_label_without_case() {
        assert!(sender_query_matches(
            "Alias259@Example.com",
            "Desk",
            "  alias259 "
        ));
        assert!(sender_query_matches("a@example.com", "Sales Desk", "desk"));
        assert!(!sender_query_matches("a@example.com", "Sales", "missing"));
        assert!(sender_query_matches("a@example.com", "Sales", "   "));
    }

    #[test]
    fn asynchronous_results_are_discarded_after_account_context_changes() {
        use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
        use std::sync::mpsc;
        struct Proxy(mpsc::Sender<Box<dyn FnOnce() + Send>>);
        impl EventLoopProxy for Proxy {
            fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
                Ok(())
            }
            fn invoke_from_event_loop(
                &self,
                event: Box<dyn FnOnce() + Send>,
            ) -> Result<(), slint::EventLoopError> {
                self.0
                    .send(event)
                    .map_err(|_| slint::EventLoopError::EventLoopTerminated)
            }
        }
        struct Headless(mpsc::Sender<Box<dyn FnOnce() + Send>>);
        impl Platform for Headless {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(
                    slint::platform::software_renderer::MinimalSoftwareWindow::new(
                        slint::platform::software_renderer::RepaintBufferType::ReusedBuffer,
                    ),
                )
            }
            fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
                Some(Box::new(Proxy(self.0.clone())))
            }
        }
        struct Delivered(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Delivered {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let (events, receiver) = mpsc::channel();
        slint::platform::set_platform(Box::new(Headless(events))).unwrap();
        let app = AppWindow::new().unwrap();
        let tasks = UiWork {
            runtime: Rc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .unwrap(),
            ),
            generation: Rc::new(Cell::new(0)),
        };
        let (release, waiting) = tokio::sync::oneshot::channel();
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let payload = Delivered(dropped.clone());
        tasks.spawn(
            &app,
            true,
            async move {
                waiting.await.unwrap();
                Ok(payload)
            },
            |_, _| panic!("removed account received private content"),
        );
        assert!(app.global::<AccountMailPreferences>().get_busy());
        tasks.generation.set(1);
        release.send(()).unwrap();
        while !dropped.load(std::sync::atomic::Ordering::SeqCst) {
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()();
        }
        // A stale result must not release a newer operation's busy indicator.
        assert!(app.global::<AccountMailPreferences>().get_busy());
        let applied = Rc::new(Cell::new(false));
        let applied2 = applied.clone();
        tasks.spawn(&app, true, async { Ok(()) }, move |app, result| {
            result.unwrap();
            assert!(!app.global::<AccountMailPreferences>().get_busy());
            applied2.set(true);
        });
        while !applied.get() {
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()();
        }
    }

    #[test]
    fn switching_signature_preserves_edits_and_rejects_ambiguous_replacement() {
        let block = "\n\n-- \nZoë";
        assert_eq!(
            remove_managed_signature(&format!("Reply{block}\n\n> Quote"), Some(block)),
            Ok("Reply\n\n> Quote".into())
        );
        assert!(remove_managed_signature("Reply\n\n-- \nEdited", Some(block)).is_err());
        assert!(remove_managed_signature(&format!("{block}{block}"), Some(block)).is_err());
    }
}
