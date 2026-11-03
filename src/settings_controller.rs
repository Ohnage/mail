//! Settings preference callback wiring.
//!
//! These callbacks share one lifecycle and persistence boundary: update the
//! session immediately where needed, then write through the core when it is
//! available. Keeping them together prevents the application composition root
//! from accumulating one closure per setting.

use super::*;
use flectar_mail_core::models::CustomTheme;

fn apply_account_presentation_settings(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Runtime,
    settings: &flectar_mail_core::models::Settings,
) {
    {
        let mut state = state.borrow_mut();
        state.account_presentation = AccountPresentationSettings::from_settings(settings);
        if let Some(core) = state.core.clone() {
            state.account_presentation.load_logos(&core.data_dir());
        }
    }
    app.set_show_message_preview(settings.show_message_preview);
    refresh_connected_accounts(app, state);
    refresh_rows_only(app, state, runtime);
    // A profile change can change which accounts the mail view shows.
    app.invoke_mail_profile_changed();
}

/// Persists the download location and only then reflects it in the UI, so a
/// failed write never leaves Settings showing a directory that was not saved.
#[cfg(desktop)]
fn save_download_directory(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Runtime,
    directory: String,
) {
    let Some(core) = state.borrow().core.clone() else {
        app.set_sync_status(UiMessage::plain(
            "Download location is saved once an account is set up.",
        ));
        return;
    };
    match runtime.block_on(core.set_download_directory(&directory)) {
        Ok(()) => {
            app.set_download_directory(
                crate::documents::download_directory_label(&directory).into(),
            );
            app.set_sync_status(UiMessage::plain("Download location saved."));
        }
        Err(error) => app.set_sync_status(UiMessage::detail(
            "Could not save the download location: {}",
            error,
        )),
    }
}

pub(super) fn register_settings_preference_callbacks(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Rc<tokio::runtime::Runtime>,
) {
    register_branding_callbacks(app, state, runtime);
    let app_weak = app.as_weak();
    let state_for_notifications = Rc::clone(state);
    let runtime_for_notifications = Rc::clone(runtime);
    app.on_save_notification_settings(move |enabled, sound, scope| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_notifications.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Notification preferences are active for this session.",
            ));
            return;
        };
        match runtime_for_notifications.block_on(core.set_notification_settings(
            enabled,
            sound,
            scope.as_str(),
        )) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Notification preferences saved.")),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save notifications: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_sync_interval = Rc::clone(state);
    let runtime_for_sync_interval = Rc::clone(runtime);
    app.on_save_sync_interval(move |minutes| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_sync_interval.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Sync interval is active for this session.",
            ));
            return;
        };
        match runtime_for_sync_interval.block_on(core.set_sync_interval_minutes(i64::from(minutes)))
        {
            Ok(()) => app.set_sync_status(UiMessage::detail(
                "Sync interval set to {} minutes.",
                minutes,
            )),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save sync interval: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_read_on_open = Rc::clone(state);
    let runtime_for_read_on_open = Rc::clone(runtime);
    app.on_save_mark_read_on_open(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        state_for_read_on_open.borrow_mut().mark_read_on_open = enabled;
        let Some(core) = state_for_read_on_open.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Message-opening preference updated for this session.",
            ));
            return;
        };
        match runtime_for_read_on_open.block_on(core.set_mark_read_on_open(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Message-opening preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save message-opening preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_contacts = Rc::clone(state);
    let runtime_for_contacts = Rc::clone(runtime);
    app.on_save_contact_discovery_settings(move |outgoing, incoming, all_accounts, suggest| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_contacts.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Contact suggestion preferences are active for this session.",
            ));
            return;
        };
        match runtime_for_contacts.block_on(core.set_contact_discovery_settings(
            outgoing,
            incoming,
            all_accounts,
            suggest,
        )) {
            Ok(()) => {
                app.set_sync_status(UiMessage::plain("Contact suggestion preferences saved."))
            }
            Err(error) => {
                if let Ok(settings) = runtime_for_contacts.block_on(core.load_settings()) {
                    app.set_collect_outgoing_contacts(settings.collect_outgoing_contacts);
                    app.set_collect_incoming_contacts(settings.collect_incoming_contacts);
                    app.set_contact_suggest_all_accounts(settings.contact_suggest_all_accounts);
                    app.set_suggest_learned_contacts(settings.suggest_learned_contacts);
                }
                app.set_sync_status(UiMessage::detail(
                    "Could not save contact suggestion preferences: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_clear_contacts = Rc::clone(state);
    let runtime_for_clear_contacts = Rc::clone(runtime);
    app.on_clear_contact_suggestions(move || {
        let Some(app) = app_weak.upgrade() else {
            return false;
        };
        let Some(core) = state_for_clear_contacts.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Contact storage is still starting. Try again shortly.",
            ));
            return false;
        };
        match runtime_for_clear_contacts.block_on(core.clear_contact_suggestions()) {
            Ok(removed) => {
                app.set_sync_status(UiMessage::detail("Cleared {} suggested people.", removed));
                app.invoke_search_contacts(app.get_contact_search_query());
                true
            }
            Err(error) => {
                app.set_sync_status(UiMessage::detail(
                    "Could not clear suggested people: {}",
                    error,
                ));
                false
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_theme = Rc::clone(state);
    let runtime_for_theme = Rc::clone(runtime);
    app.on_save_theme(move |theme| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_theme.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Theme updated for this session."));
            return;
        };
        match runtime_for_theme.block_on(core.set_theme(&theme)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Theme preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail("Could not save theme: {}", error)),
        }
    });

    let app_weak = app.as_weak();
    // Only desktop hosts pick a filesystem directory; mobile hosts export
    // through their own document provider and hide the setting entirely.
    #[cfg(desktop)]
    {
        let state_for_download = Rc::clone(state);
        let runtime_for_download = Rc::clone(runtime);
        let app_weak_for_download = app_weak.clone();
        let picking_download = Rc::new(Cell::new(false));
        app.on_choose_download_directory(move || {
            let Some(app) = app_weak_for_download.upgrade() else {
                return;
            };
            if picking_download.replace(true) {
                return;
            }
            let title = translated(&app, &UiMessage::plain("Choose download location")).to_string();
            let core = state_for_download.borrow().core.clone();
            // The portal picker needs the runtime: rfd's blocking dialog
            // panics on the UI thread, which has no Tokio context.
            let request: crate::documents::DocumentFuture<Option<std::path::PathBuf>> =
                Box::pin(async move {
                    // Starts in the saved folder as stored, not as Settings
                    // shows it: inside Flatpak the shown host path may be
                    // outside the sandbox.
                    let current = match core {
                        Some(core) => core.load_settings().await.ok().and_then(|settings| {
                            crate::documents::resolve_download_directory(
                                &settings.download_directory,
                            )
                        }),
                        None => None,
                    };
                    let dialog = rfd::AsyncFileDialog::new().set_title(title);
                    let dialog = match current {
                        Some(directory) => {
                            dialog.set_directory(crate::documents::picker_directory(&directory))
                        }
                        None => dialog,
                    };
                    Ok(dialog
                        .pick_folder()
                        .await
                        .map(|folder| folder.path().to_owned()))
                });
            let state = Rc::clone(&state_for_download);
            let runtime = Rc::clone(&runtime_for_download);
            let picking = Rc::clone(&picking_download);
            let app_weak = app.as_weak();
            crate::documents::run_request(&runtime_for_download, request, move |picked| {
                picking.set(false);
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                match picked {
                    Ok(Some(directory)) => save_download_directory(
                        &app,
                        &state,
                        &runtime,
                        directory.to_string_lossy().into_owned(),
                    ),
                    Ok(None) => {}
                    Err(error) => app.set_sync_status(UiMessage::detail(
                        "Could not save the download location: {}",
                        error,
                    )),
                }
            });
        });
        let state_for_clear = Rc::clone(state);
        let runtime_for_clear = Rc::clone(runtime);
        let app_weak_for_clear = app_weak.clone();
        app.on_clear_download_directory(move || {
            let Some(app) = app_weak_for_clear.upgrade() else {
                return;
            };
            save_download_directory(&app, &state_for_clear, &runtime_for_clear, String::new());
        });
    }
    let state_for_theme_preset = Rc::clone(state);
    let runtime_for_theme_preset = Rc::clone(runtime);
    app.on_save_theme_preset(move |preset| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_theme_preset.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Theme palette updated for this session."));
            return;
        };
        match runtime_for_theme_preset.block_on(core.set_theme_preset(preset.as_str())) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Theme palette saved.")),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save theme palette: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_appearance = Rc::clone(state);
    let runtime_for_appearance = Rc::clone(runtime);
    app.global::<MessageColors>().on_choose(move |preference| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if flectar_mail_core::models::MessageAppearance::parse(&preference).is_none() {
            return;
        }
        let colors = app.global::<MessageColors>();
        let previous = colors.get_preference();
        colors.set_preference(preference.clone());
        let Some(core) = state_for_appearance.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Message appearance updated for this session.",
            ));
            return;
        };
        match runtime_for_appearance.block_on(core.set_message_appearance(&preference)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Message appearance preference saved.")),
            Err(error) => {
                colors.set_preference(previous);
                app.set_sync_status(UiMessage::detail(
                    "Could not save message appearance: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_custom_theme = Rc::clone(state);
    let runtime_for_custom_theme = Rc::clone(runtime);
    app.on_save_custom_theme(
        move |light_primary,
              light_page,
              light_surface,
              light_text,
              light_border,
              dark_primary,
              dark_page,
              dark_surface,
              dark_text,
              dark_border| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let custom_theme = CustomTheme {
                light_primary: theme::color_to_hex(light_primary),
                light_page_background: theme::color_to_hex(light_page),
                light_surface: theme::color_to_hex(light_surface),
                light_text: theme::color_to_hex(light_text),
                light_border: theme::color_to_hex(light_border),
                dark_primary: theme::color_to_hex(dark_primary),
                dark_page_background: theme::color_to_hex(dark_page),
                dark_surface: theme::color_to_hex(dark_surface),
                dark_text: theme::color_to_hex(dark_text),
                dark_border: theme::color_to_hex(dark_border),
            };
            let Some(core) = state_for_custom_theme.borrow().core.clone() else {
                app.set_sync_status(UiMessage::plain("Custom theme updated for this session."));
                return false;
            };
            match runtime_for_custom_theme.block_on(core.set_custom_theme(custom_theme)) {
                Ok(()) => {
                    app.set_sync_status(UiMessage::plain("Custom theme saved."));
                    true
                }
                Err(error) => {
                    app.set_sync_status(UiMessage::detail(
                        "Could not save custom theme: {}",
                        error,
                    ));
                    false
                }
            }
        },
    );

    let app_weak = app.as_weak();
    let state_for_language = Rc::clone(state);
    let runtime_for_language = Rc::clone(runtime);
    app.on_save_language(move |language| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        apply_language(&app, language.as_str());
        let Some(core) = state_for_language.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Language updated for this session."));
            return;
        };
        match runtime_for_language.block_on(core.set_language(language.as_str())) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Language preference saved.")),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save language: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_time_format = Rc::clone(state);
    let runtime_for_time_format = Rc::clone(runtime);
    app.on_save_time_format(move |value| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(format) = flectar_mail_core::models::TimeFormat::parse(&value) else {
            return;
        };
        date_format::reload(&app);
        let Some(core) = state_for_time_format.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Time format updated for this session."));
            return;
        };
        match runtime_for_time_format.block_on(core.set_time_format(format)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Time format preference saved.")),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save time format: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_app_icon = Rc::clone(state);
    let runtime_for_app_icon = Rc::clone(runtime);
    app.global::<AppIcon>().on_choose(move |variant| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let previous = crate::app_icon::choice(app.global::<AppIcon>().get_variant());
        let icon = crate::app_icon::choice(variant);
        crate::app_icon::apply(&app, icon);
        let Some(core) = state_for_app_icon.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("App icon updated for this session."));
            return;
        };
        match runtime_for_app_icon.block_on(core.set_app_icon(icon)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("App icon saved.")),
            Err(error) => {
                crate::app_icon::apply(&app, previous);
                app.set_sync_status(UiMessage::detail("Could not save app icon: {}", error));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_sidebar_icons = Rc::clone(state);
    let runtime_for_sidebar_icons = Rc::clone(runtime);
    app.on_save_sidebar_icon_style(move |monochrome| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_sidebar_icons.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Sidebar icon style updated for this session.",
            ));
            return;
        };
        match runtime_for_sidebar_icons.block_on(core.set_monochrome_sidebar_icons(monochrome)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Sidebar icon preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save sidebar icon preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_avatars = Rc::clone(state);
    let runtime_for_avatars = Rc::clone(runtime);
    app.on_save_avatar_visibility(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_avatars.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Avatar preference updated for this session.",
            ));
            return;
        };
        match runtime_for_avatars.block_on(core.set_show_avatars(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Avatar preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save avatar preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_tabs = Rc::clone(state);
    let runtime_for_tabs = Rc::clone(runtime);
    app.on_save_show_tabs(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_tabs.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Tabs preference updated for this session.",
            ));
            return;
        };
        match runtime_for_tabs.block_on(core.set_show_tabs(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Tabs preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save tabs preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_grouping = Rc::clone(state);
    let runtime_for_grouping = Rc::clone(runtime);
    app.on_save_conversation_view(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let grouping = if enabled {
            MailGrouping::Conversations
        } else {
            MailGrouping::Messages
        };
        apply_mail_grouping(&app, &state_for_grouping, &runtime_for_grouping, grouping);
        let Some(core) = state_for_grouping.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Conversation view updated for this session.",
            ));
            return;
        };
        match runtime_for_grouping.block_on(core.set_conversation_view(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Conversation view preference saved.")),
            Err(error) => {
                let previous = if enabled {
                    MailGrouping::Messages
                } else {
                    MailGrouping::Conversations
                };
                apply_mail_grouping(&app, &state_for_grouping, &runtime_for_grouping, previous);
                app.set_sync_status(UiMessage::detail(
                    "Could not save conversation view preference: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_date_groups = Rc::clone(state);
    let runtime_for_date_groups = Rc::clone(runtime);
    app.on_save_group_mail_by_date(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        state_for_date_groups.borrow_mut().mail_groups.clear();
        refresh_rows_only(&app, &state_for_date_groups, &runtime_for_date_groups);
        let Some(core) = state_for_date_groups.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Mail grouping is active for this session.",
            ));
            return;
        };
        match runtime_for_date_groups.block_on(core.set_group_mail_by_date(enabled)) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Mail grouping preference saved.")),
            Err(error) => {
                app.set_group_mail_by_date(!enabled);
                refresh_rows_only(&app, &state_for_date_groups, &runtime_for_date_groups);
                app.set_sync_status(UiMessage::detail(
                    "Could not save mail grouping preference: {}",
                    error,
                ));
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_profiles = Rc::clone(state);
    let runtime_for_profiles = Rc::clone(runtime);
    app.on_save_mail_profile(move |profile_id, name, color| {
        let Some(app) = app_weak.upgrade() else {
            return Default::default();
        };
        let Some(core) = state_for_profiles.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Profile changes require local mail data."));
            return Default::default();
        };
        let profile_id = (!profile_id.trim().is_empty()).then(|| profile_id.to_string());
        let (mut settings, profile) = match runtime_for_profiles.block_on(core.save_mail_profile(
            profile_id,
            name.to_string(),
            theme::color_to_hex(color),
        )) {
            Ok(saved) => saved,
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save profile: {}", error));
                return Default::default();
            }
        };
        // The editor's checked accounts become the profile's accounts.
        let members = app.get_profile_members();
        let mut failure = None;
        for member in members.iter() {
            let account_id = i64::from(member.id);
            let in_profile = profile.account_ids.contains(&account_id);
            let assignment = match (member.checked, in_profile) {
                (true, false) => Some(profile.id.clone()),
                (false, true) => None,
                _ => continue,
            };
            match runtime_for_profiles.block_on(core.assign_account_profile(account_id, assignment))
            {
                Ok(updated) => settings = updated,
                Err(error) => failure = Some(error),
            }
        }
        apply_account_presentation_settings(
            &app,
            &state_for_profiles,
            &runtime_for_profiles,
            &settings,
        );
        match failure {
            Some(error) => app.set_sync_status(UiMessage::detail(
                "Could not update account profile: {}",
                error,
            )),
            None => app.set_sync_status(UiMessage::plain("Profile saved.")),
        }
        profile.id.into()
    });

    let app_weak = app.as_weak();
    app.on_prepare_profile_members(move |profile_id, checked_account| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let rows = connected_account_rows(&app)
            .into_iter()
            .map(|account| {
                let member = !profile_id.is_empty() && account.profile_id == profile_id;
                ProfileMemberRow {
                    id: account.id,
                    name: account.name,
                    email: account.email,
                    initials: account.initials,
                    avatar: account.avatar_small,
                    has_avatar: account.has_avatar,
                    other_profile: if member {
                        Default::default()
                    } else {
                        account.profile_name
                    },
                    checked: member || account.id == checked_account,
                }
            })
            .collect::<Vec<_>>();
        app.set_profile_members(ModelRc::new(VecModel::from(rows)));
    });

    let app_weak = app.as_weak();
    let state_for_profile_assignment = Rc::clone(state);
    let runtime_for_profile_assignment = Rc::clone(runtime);
    app.on_assign_account_profile(move |account_id, profile_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_profile_assignment.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Profile changes require local mail data."));
            return;
        };
        let profile_id = (!profile_id.trim().is_empty()).then(|| profile_id.to_string());
        match runtime_for_profile_assignment
            .block_on(core.assign_account_profile(i64::from(account_id), profile_id))
        {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_profile_assignment,
                    &runtime_for_profile_assignment,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Account profile updated."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not update account profile: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_account_color = Rc::clone(state);
    let runtime_for_account_color = Rc::clone(runtime);
    app.on_save_account_color(move |account_id, color, generated| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_account_color.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Account color changes require local mail data.",
            ));
            return;
        };
        let color = (!generated).then(|| theme::color_to_hex(color));
        match runtime_for_account_color
            .block_on(core.set_account_color(i64::from(account_id), color))
        {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_account_color,
                    &runtime_for_account_color,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Account color updated."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not update account color: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_active_profile = Rc::clone(state);
    let runtime_for_active_profile = Rc::clone(runtime);
    app.on_select_mail_profile(move |profile_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_active_profile.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Profile changes require local mail data."));
            return;
        };
        let profile_id = (!profile_id.trim().is_empty()).then(|| profile_id.to_string());
        match runtime_for_active_profile.block_on(core.set_active_mail_profile(profile_id)) {
            Ok(settings) => apply_account_presentation_settings(
                &app,
                &state_for_active_profile,
                &runtime_for_active_profile,
                &settings,
            ),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not switch profile: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_profile_delete = Rc::clone(state);
    let runtime_for_profile_delete = Rc::clone(runtime);
    app.on_delete_mail_profile(move |profile_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_profile_delete.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain("Profile changes require local mail data."));
            return;
        };
        match runtime_for_profile_delete.block_on(core.delete_mail_profile(profile_id.to_string()))
        {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_profile_delete,
                    &runtime_for_profile_delete,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Profile deleted."));
            }
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not delete profile: {}", error))
            }
        }
    });

    let app_weak = app.as_weak();
    let state_for_marker_visibility = Rc::clone(state);
    let runtime_for_marker_visibility = Rc::clone(runtime);
    app.on_save_account_markers_visibility(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_marker_visibility.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Marker preference is active for this session.",
            ));
            state_for_marker_visibility
                .borrow_mut()
                .account_presentation
                .show_markers = enabled;
            refresh_rows_only(
                &app,
                &state_for_marker_visibility,
                &runtime_for_marker_visibility,
            );
            return;
        };
        match runtime_for_marker_visibility.block_on(core.set_show_account_badges(enabled)) {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_marker_visibility,
                    &runtime_for_marker_visibility,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Mail-list marker preference saved."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save mail-list marker preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_workspace_layout = Rc::clone(state);
    let runtime_for_workspace_layout = Rc::clone(runtime);
    app.on_save_workspace_layout(move |layout| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_workspace_layout.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Workspace layout updated for this session.",
            ));
            return;
        };
        match runtime_for_workspace_layout.block_on(core.set_workspace_layout(layout.as_str())) {
            Ok(()) => app.set_sync_status(UiMessage::plain("Workspace layout preference saved.")),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save workspace layout: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_list_pane_width = Rc::clone(state);
    let runtime_for_list_pane_width = Rc::clone(runtime);
    app.on_save_workspace_list_pane_width(move |width| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let width =
            flectar_mail_core::models::normalized_workspace_list_pane_width(width.round() as i64);
        app.set_workspace_list_pane_width(width as f32);
        let Some(core) = state_for_list_pane_width.borrow().core.clone() else {
            return;
        };
        if let Err(error) =
            runtime_for_list_pane_width.block_on(core.set_workspace_list_pane_width(width))
        {
            app.set_sync_status(UiMessage::detail(
                "Could not save list pane width: {}",
                error,
            ));
        }
    });

    let app_weak = app.as_weak();
    let state_for_preview = Rc::clone(state);
    let runtime_for_preview = Rc::clone(runtime);
    app.on_save_message_preview(move |enabled| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_preview.borrow().core.clone() else {
            app.set_show_message_preview(enabled);
            return;
        };
        match runtime_for_preview.block_on(core.set_show_message_preview(enabled)) {
            Ok(settings) => apply_account_presentation_settings(
                &app,
                &state_for_preview,
                &runtime_for_preview,
                &settings,
            ),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save the preview preference: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_registry = Rc::clone(state);
    let runtime_for_registry = Rc::clone(runtime);
    app.on_import_brand_registry(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_registry.borrow().core.clone() else {
            app.set_sync_status(UiMessage::plain(
                "Importing a brand registry requires local mail data.",
            ));
            return;
        };
        let Some(path) =
            choose_settings_file_path("Import brand registry", "Brand registry", &["json"])
        else {
            return;
        };
        match runtime_for_registry.block_on(core.import_brand_registry_file(&path)) {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &state_for_registry,
                    &runtime_for_registry,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Brand registry imported."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not import the brand registry: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_tab = Rc::clone(state);
    let runtime_for_tab = Rc::clone(runtime);
    app.on_select_business_tab(move |tab_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_tab.borrow().core.clone() else {
            return;
        };
        let tab_id = (!tab_id.trim().is_empty()).then(|| tab_id.to_string());
        match runtime_for_tab.block_on(core.set_active_business_tab(tab_id)) {
            Ok(settings) => apply_account_presentation_settings(
                &app,
                &state_for_tab,
                &runtime_for_tab,
                &settings,
            ),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not open that business tab: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_save_tab = Rc::clone(state);
    let runtime_for_save_tab = Rc::clone(runtime);
    app.on_save_business_tab(move |tab_id, name| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_save_tab.borrow().core.clone() else {
            return;
        };
        let existing = state_for_save_tab
            .borrow()
            .account_presentation
            .business_tabs
            .iter()
            .find(|tab| tab.id == tab_id.as_str())
            .cloned();
        let profile_ids = existing
            .as_ref()
            .map(|tab| tab.profile_ids.clone())
            .unwrap_or_default();
        let patterns = existing
            .as_ref()
            .map(|tab| tab.recipient_patterns.clone())
            .unwrap_or_default();
        let tab_id = (!tab_id.trim().is_empty()).then(|| tab_id.to_string());
        match runtime_for_save_tab.block_on(core.save_business_tab(
            tab_id,
            name.to_string(),
            profile_ids,
            patterns,
        )) {
            Ok(settings) => apply_account_presentation_settings(
                &app,
                &state_for_save_tab,
                &runtime_for_save_tab,
                &settings,
            ),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not save that business tab: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_delete_tab = Rc::clone(state);
    let runtime_for_delete_tab = Rc::clone(runtime);
    app.on_delete_business_tab(move |tab_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_delete_tab.borrow().core.clone() else {
            return;
        };
        match runtime_for_delete_tab.block_on(core.delete_business_tab(tab_id.to_string())) {
            Ok(settings) => apply_account_presentation_settings(
                &app,
                &state_for_delete_tab,
                &runtime_for_delete_tab,
                &settings,
            ),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not remove that business tab: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_move_tab = Rc::clone(state);
    let runtime_for_move_tab = Rc::clone(runtime);
    app.on_move_business_tab(move |tab_id, delta| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_move_tab.borrow().core.clone() else {
            return;
        };
        match runtime_for_move_tab.block_on(core.move_business_tab(tab_id.to_string(), delta)) {
            Ok(settings) => apply_account_presentation_settings(
                &app,
                &state_for_move_tab,
                &runtime_for_move_tab,
                &settings,
            ),
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not move that business tab: {}",
                error,
            )),
        }
    });

    let app_weak = app.as_weak();
    let state_for_logo = Rc::clone(state);
    let runtime_for_logo = Rc::clone(runtime);
    app.on_choose_profile_logo(move |profile_id| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(core) = state_for_logo.borrow().core.clone() else {
            return;
        };
        let Some(path) = choose_settings_file_path(
            "Choose profile logo",
            "Image",
            &["png", "jpg", "jpeg", "gif", "webp"],
        ) else {
            return;
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not read that logo: {}", error));
                return;
            }
        };
        let Some(extension) = flectar_mail_core::brand_registry::extension_for(&bytes) else {
            app.set_sync_status(UiMessage::plain(
                "A profile logo must be a PNG, JPEG, GIF, or WebP image.",
            ));
            return;
        };
        if bytes.len() > 2 * 1024 * 1024 {
            app.set_sync_status(UiMessage::plain("A profile logo must be 2 MB or smaller."));
            return;
        }
        let relative = format!("profile-logos/{profile_id}.{extension}");
        let destination = core.data_dir().join(&relative);
        if let Some(parent) = destination.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            app.set_sync_status(UiMessage::detail("Could not store that logo: {}", error));
            return;
        }
        if let Err(error) = std::fs::write(&destination, &bytes) {
            app.set_sync_status(UiMessage::detail("Could not store that logo: {}", error));
            return;
        }
        match runtime_for_logo
            .block_on(core.set_profile_logo(profile_id.to_string(), Some(relative)))
        {
            Ok(settings) => apply_account_presentation_settings(
                &app,
                &state_for_logo,
                &runtime_for_logo,
                &settings,
            ),
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save that logo: {}", error))
            }
        }
    });
}

/// Both entry points share one asynchronous persistence path and busy state.
pub(super) fn register_oauth_settings_callbacks(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Rc<tokio::runtime::Runtime>,
) {
    let weak = app.as_weak();
    let single_state = Rc::clone(state);
    let single_runtime = Rc::clone(runtime);
    app.on_save_oauth_app(move |provider, client_id, client_secret| {
        let Some(app) = weak.upgrade() else { return };
        let (google, microsoft) = match provider.as_str() {
            "gmail" => (
                Some((client_id.to_string(), client_secret.to_string())),
                None,
            ),
            "microsoft" => (None, Some(client_id.to_string())),
            _ => return,
        };
        save_oauth_settings(
            &app,
            &single_state,
            &single_runtime,
            google,
            microsoft,
            false,
        );
    });

    let weak = app.as_weak();
    let state = Rc::clone(state);
    let runtime = Rc::clone(runtime);
    app.on_save_oauth_apps(move |google_id, google_secret, microsoft_id| {
        let Some(app) = weak.upgrade() else { return };
        save_oauth_settings(
            &app,
            &state,
            &runtime,
            Some((google_id.to_string(), google_secret.to_string())),
            Some(microsoft_id.to_string()),
            true,
        );
    });
}

fn save_oauth_settings(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &tokio::runtime::Runtime,
    google: Option<(String, String)>,
    microsoft: Option<String>,
    close_dialog: bool,
) {
    if app.get_oauth_settings_saving() || app.get_oauth_in_progress() {
        return;
    }
    let Some(core) = state.borrow().core.clone() else {
        let message = UiMessage::plain("Local mail data is unavailable. Retry startup.");
        app.set_sync_status(message.clone());
        app.set_oauth_settings_error(ui_message::translated(app, &message));
        return;
    };
    let update_google = google.is_some();
    let update_microsoft = microsoft.is_some();
    app.set_oauth_settings_error("".into());
    app.set_oauth_settings_saving(true);
    let weak = app.as_weak();
    runtime.spawn(async move {
        let result = core.set_oauth_apps(google, microsoft).await;
        let _ = weak.upgrade_in_event_loop(move |app| {
            app.set_oauth_settings_saving(false);
            match result {
                Ok(settings) => {
                    if update_google {
                        app.set_google_client_id(settings.google_client_id.clone().into());
                        app.set_google_client_secret(settings.google_client_secret.clone().into());
                    }
                    if update_microsoft {
                        app.set_ms_client_id(settings.ms_client_id.clone().into());
                    }
                    app.set_custom_oauth_configured(
                        !settings.google_client_id.is_empty() || !settings.ms_client_id.is_empty(),
                    );
                    startup::refresh_oauth_availability(&app);
                    app.set_sync_status(UiMessage::plain("OAuth app keys saved."));
                    if close_dialog {
                        app.set_oauth_setup_open(false);
                    }
                }
                Err(error) => {
                    let message = UiMessage::detail("OAuth settings failed: {}", error);
                    app.set_sync_status(message.clone());
                    app.set_oauth_settings_error(ui_message::translated(&app, &message));
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_settings_discards_an_unsaved_palette() {
        use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
        struct Headless(Rc<MinimalSoftwareWindow>);
        impl slint::platform::Platform for Headless {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window))).unwrap();
        let app = AppWindow::new().unwrap();
        let theme = app.global::<AppTheme>();
        let draft = app.global::<ThemeDraft>();
        let saved = theme.get_custom_light_primary();
        app.set_settings_open(true);
        slint::platform::update_timers_and_animations();

        // The editor previews a color before it is saved.
        draft.invoke_begin();
        theme.set_custom_light_primary(slint::Color::from_rgb_u8(0xff, 0, 0));
        app.set_settings_open(false);
        slint::platform::update_timers_and_animations();
        assert_eq!(theme.get_custom_light_primary(), saved);
        assert!(!draft.get_dirty());

        // A saved palette stays.
        app.set_settings_open(true);
        slint::platform::update_timers_and_animations();
        draft.invoke_begin();
        let chosen = slint::Color::from_rgb_u8(0, 0x80, 0);
        theme.set_custom_light_primary(chosen);
        draft.set_dirty(false);
        app.set_settings_open(false);
        slint::platform::update_timers_and_animations();
        assert_eq!(theme.get_custom_light_primary(), chosen);
    }
}

#[derive(Default)]
struct BusinessTabDraft {
    inbox_account: Option<flectar_mail_core::models::InboxAccount>,
    named_accounts: std::collections::BTreeSet<i64>,
    id: Option<String>,
    icon: Option<String>,
    accounts: std::collections::BTreeSet<i64>,
    address_views: std::collections::BTreeSet<i64>,
    original_account_ids: Option<Vec<i64>>,
    original_address_view_ids: Option<Vec<i64>>,
    membership_dirty: bool,
}

#[derive(Debug, PartialEq)]
enum InboxChoiceKey {
    Account(i64),
    Alias(i64),
}

fn parse_inbox_choice_key(key: &str) -> Option<InboxChoiceKey> {
    let (kind, id) = key.split_once(':')?;
    let id = id.parse::<i64>().ok().filter(|id| *id > 0)?;
    match kind {
        "account" => Some(InboxChoiceKey::Account(id)),
        "alias" => Some(InboxChoiceKey::Alias(id)),
        _ => None,
    }
}

impl BusinessTabDraft {
    fn set_choice(&mut self, key: &str, checked: bool) -> bool {
        if let Some(id) = key
            .strip_prefix("inbox:")
            .and_then(|id| id.parse::<i64>().ok())
            .filter(|id| *id < 0)
        {
            let changed = if checked {
                self.named_accounts.insert(id)
            } else {
                self.named_accounts.remove(&id)
            };
            self.membership_dirty |= changed;
            return true;
        }
        let (choices, id) = match parse_inbox_choice_key(key) {
            Some(InboxChoiceKey::Account(id)) => (&mut self.accounts, id),
            Some(InboxChoiceKey::Alias(id)) => (&mut self.address_views, id),
            None => return false,
        };
        let changed = if checked {
            choices.insert(id)
        } else {
            choices.remove(&id)
        };
        self.membership_dirty |= changed;
        true
    }

    fn memberships_for_save(&self) -> (Option<Vec<i64>>, Option<Vec<i64>>) {
        if self.id.is_none() || self.membership_dirty {
            (
                Some(self.accounts.iter().copied().collect()),
                Some(self.address_views.iter().copied().collect()),
            )
        } else {
            (
                self.original_account_ids.clone(),
                self.original_address_view_ids.clone(),
            )
        }
    }
}

fn discard_business_tab_draft(draft: &RefCell<Option<BusinessTabDraft>>) {
    *draft.borrow_mut() = None;
}

// The draft starts from the sidebar's current membership. Recipient-only
// views inherit every connected account until an explicit checklist is saved.
// Address-filtered groups never infer whole source mailboxes from profile data.
fn initial_tab_accounts(
    explicit: Option<&[i64]>,
    scope: &str,
    inherited: &[i64],
    connected: &[i64],
    has_address_catalog: bool,
) -> std::collections::BTreeSet<i64> {
    explicit
        .unwrap_or(if scope == "recipients" {
            connected
        } else if has_address_catalog && matches!(scope, "profiles" | "accounts") {
            &[]
        } else {
            inherited
        })
        .iter()
        .copied()
        .collect()
}

#[cfg(test)]
mod branding_draft_tests {
    use super::{
        BusinessTabDraft, InboxChoiceKey, discard_business_tab_draft, initial_tab_accounts,
        parse_inbox_choice_key,
    };

    fn automatic_draft() -> BusinessTabDraft {
        BusinessTabDraft {
            id: Some("automatic-group".into()),
            accounts: [3].into_iter().collect(),
            address_views: [101, 102].into_iter().collect(),
            inbox_account: None,
            named_accounts: Default::default(),
            ..Default::default()
        }
    }

    #[test]
    fn presentation_only_save_keeps_automatic_membership() {
        let mut draft = automatic_draft();
        draft.icon = Some("tab-icons/changed.png".into());
        assert_eq!(draft.memberships_for_save(), (None, None));
        assert!(!draft.membership_dirty);
    }

    #[test]
    fn untouched_mixed_memberships_preserve_explicit_empty_and_automatic() {
        let mut draft = automatic_draft();
        draft.original_account_ids = Some(vec![]);
        assert_eq!(draft.memberships_for_save(), (Some(vec![]), None));
    }

    #[test]
    fn checklist_changes_make_both_memberships_explicit() {
        let mut draft = automatic_draft();
        assert!(draft.set_choice("alias:103", true));
        assert_eq!(
            draft.memberships_for_save(),
            (Some(vec![3]), Some(vec![101, 102, 103]))
        );
        // Restoring the visible selection still records the user's explicit choice.
        assert!(draft.set_choice("alias:103", false));
        assert_eq!(
            draft.memberships_for_save(),
            (Some(vec![3]), Some(vec![101, 102]))
        );
    }

    #[test]
    fn new_empty_group_saves_explicit_empty_memberships() {
        assert_eq!(
            BusinessTabDraft::default().memberships_for_save(),
            (Some(vec![]), Some(vec![]))
        );
    }

    #[test]
    fn cancel_discards_membership_changes_before_reopening() {
        let editor = std::cell::RefCell::new(Some(automatic_draft()));
        assert!(
            editor
                .borrow_mut()
                .as_mut()
                .unwrap()
                .set_choice("alias:103", true)
        );
        discard_business_tab_draft(&editor);
        assert!(editor.borrow().is_none());
        *editor.borrow_mut() = Some(automatic_draft());
        assert_eq!(
            editor.borrow().as_ref().unwrap().memberships_for_save(),
            (None, None)
        );
    }

    #[test]
    fn alias_and_account_choices_keep_independent_membership() {
        let mut draft = BusinessTabDraft::default();
        assert!(draft.set_choice("account:7", true));
        assert!(draft.set_choice("alias:7", true));
        assert!(draft.set_choice("alias:7", false));
        assert_eq!(draft.accounts.into_iter().collect::<Vec<_>>(), vec![7]);
        assert!(draft.address_views.is_empty());
    }

    #[test]
    fn choice_keys_preserve_large_alias_ids_and_reject_invalid_ids() {
        assert_eq!(
            parse_inbox_choice_key("alias:9000000000000000000"),
            Some(InboxChoiceKey::Alias(9_000_000_000_000_000_000))
        );
        for key in ["alias:0", "account:-1", "alias:bad", "other:7"] {
            assert!(parse_inbox_choice_key(key).is_none());
        }
    }

    #[test]
    fn recipient_tab_without_explicit_members_starts_with_every_connected_account() {
        let selected = initial_tab_accounts(None, "recipients", &[9], &[3, 1, 3, 2], true);
        assert_eq!(selected.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn recipient_tab_keeps_explicit_members_including_empty_selection() {
        assert!(initial_tab_accounts(Some(&[]), "recipients", &[9], &[1, 2], true).is_empty());
        let selected = initial_tab_accounts(Some(&[2, 2]), "recipients", &[9], &[1, 2], true);
        assert_eq!(selected.into_iter().collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn address_groups_do_not_select_whole_source_mailboxes_by_default() {
        for scope in ["profiles", "accounts"] {
            assert!(initial_tab_accounts(None, scope, &[4, 9], &[1, 2], true).is_empty());
            assert_eq!(
                initial_tab_accounts(Some(&[9]), scope, &[4, 9], &[1, 2], true)
                    .into_iter()
                    .collect::<Vec<_>>(),
                vec![9]
            );
        }
    }

    #[test]
    fn other_scopes_keep_inherited_profile_membership() {
        for scope in ["accounts", "any", "narrow"] {
            let selected = initial_tab_accounts(None, scope, &[9, 4, 9], &[1, 2], false);
            assert_eq!(selected.into_iter().collect::<Vec<_>>(), vec![4, 9]);
        }
    }
}

fn publish_tab_members(app: &AppWindow, draft: &BusinessTabDraft) {
    if draft.inbox_account.is_none() {
        let editor = app.global::<BusinessTabEditorUi>();
        let query = editor.get_query().to_lowercase();
        let choices = app
            .global::<InboxAccountsUi>()
            .get_accounts()
            .iter()
            .filter_map(|mut row| {
                if !query.is_empty() && !row.name.to_lowercase().contains(&query) {
                    return None;
                }
                row.checked = row
                    .key
                    .strip_prefix("inbox:")
                    .and_then(|id| id.parse::<i64>().ok())
                    .is_some_and(|id| draft.named_accounts.contains(&id));
                Some(row)
            })
            .collect::<Vec<_>>();
        editor.set_members(ModelRc::new(VecModel::from(choices)));
        editor.set_selected_count(draft.named_accounts.len().try_into().unwrap_or(i32::MAX));
        return;
    }
    let editor = app.global::<BusinessTabEditorUi>();
    let query = editor.get_query().trim().to_lowercase();
    let mut choices = app
        .get_connected_accounts()
        .iter()
        .map(|account| InboxViewChoiceRow {
            key: format!("account:{}", account.id).into(),
            kind: InboxViewKind::Account,
            name: account.name,
            detail: account.email,
            initials: account.initials,
            avatar: account.avatar_small,
            has_avatar: account.has_avatar,
            checked: draft.accounts.contains(&i64::from(account.id)),
            color: Default::default(),
        })
        .collect::<Vec<_>>();
    choices.extend(app.get_address_inboxes().iter().map(|mut view| {
        view.checked = match parse_inbox_choice_key(view.key.as_str()) {
            Some(InboxChoiceKey::Alias(id)) => draft.address_views.contains(&id),
            _ => false,
        };
        view
    }));
    choices.retain(|choice| {
        query.is_empty()
            || choice.name.to_lowercase().contains(&query)
            || choice.detail.to_lowercase().contains(&query)
    });
    editor.set_members(ModelRc::new(VecModel::from(choices)));
    editor.set_selected_count(
        (draft.accounts.len() + draft.address_views.len())
            .try_into()
            .unwrap_or(i32::MAX),
    );
}

#[cfg(desktop)]
fn choose_settings_file_path(
    title: &str,
    filter_name: &str,
    extensions: &[&str],
) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .add_filter(filter_name, extensions)
        .pick_file()
}

#[cfg(mobile)]
fn choose_settings_file_path(
    _title: &str,
    _filter_name: &str,
    _extensions: &[&str],
) -> Option<std::path::PathBuf> {
    None
}

fn palette_color(color: slint::Color) -> bool {
    matches!(
        (color.red(), color.green(), color.blue()),
        (0x3b, 0x82, 0xf6)
            | (0x8b, 0x5c, 0xf6)
            | (0x0d, 0x94, 0x88)
            | (0xea, 0x58, 0x0c)
            | (0xdb, 0x27, 0x77)
            | (0x65, 0xa3, 0x0d)
            | (0x08, 0x91, 0xb2)
            | (0x93, 0x33, 0xea)
    )
}

fn show_editor_color(editor: &BusinessTabEditorUi, color: slint::Color) {
    editor.set_accent(color);
    editor.set_custom_accent(!palette_color(color));
}

fn choose_custom_icon_path() -> Option<std::path::PathBuf> {
    #[cfg(desktop)]
    {
        rfd::FileDialog::new()
            .set_title("Choose icon")
            .add_filter("Image", &["png", "jpg", "jpeg", "gif", "webp", "ico"])
            .pick_file()
    }
    #[cfg(not(desktop))]
    {
        None
    }
}

fn register_branding_callbacks(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Rc<tokio::runtime::Runtime>,
) {
    use crate::custom_icons::{IconKind, import_icon};
    let draft = Rc::new(RefCell::new(None::<BusinessTabDraft>));
    let catalog_draft = Rc::clone(&draft);
    let weak = app.as_weak();
    let catalog_state = Rc::clone(state);
    let catalog_runtime = Rc::clone(runtime);
    app.global::<ReceivingInboxUi>().on_create(move || {
        let Some(app) = weak.upgrade() else {
            return false;
        };
        let form = app.global::<ReceivingInboxUi>();
        form.set_created(false);
        let Some(core) = catalog_state.borrow().core.clone() else {
            form.set_error("Inbox views require local mail data.".into());
            return false;
        };
        let address = form.get_address().trim().to_string();
        let sources = form
            .get_source_emails()
            .split([',', ';', '\n'])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
        match catalog_runtime.block_on(core.save_address_view(address, sources)) {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &catalog_state,
                    &catalog_runtime,
                    &settings,
                );
                form.set_address(Default::default());
                form.set_source_emails(Default::default());
                form.set_error(Default::default());
                form.set_created(true);
                if let Some(draft) = catalog_draft.borrow().as_ref() {
                    publish_tab_members(&app, draft);
                }
                true
            }
            Err(error) => {
                form.set_error(format!("Could not add inbox view: {error}").into());
                false
            }
        }
    });

    let weak = app.as_weak();
    let open_state = Rc::clone(state);
    let open_draft = Rc::clone(&draft);
    app.global::<BusinessTabEditorUi>().on_open(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let state = open_state.borrow();
        let presentation = &state.account_presentation;
        let tab = presentation
            .business_tabs
            .iter()
            .find(|tab| tab.id == id.as_str());
        if !id.is_empty() && tab.is_none() {
            return;
        }
        let data_root = state.core.as_ref().map(|core| core.data_dir());
        let icon = tab.and_then(|tab| tab.icon.clone());
        let preview = icon.as_ref().and_then(|relative| {
            data_root
                .as_ref()
                .and_then(|root| load_profile_logo(&root.join(relative)))
        });
        let initial = BusinessTabDraft {
            inbox_account: None,
            named_accounts: tab
                .map(|tab| presentation.selected_inbox_accounts(tab))
                .unwrap_or_default()
                .into_iter()
                .collect(),
            id: tab.map(|tab| tab.id.clone()),
            icon,
            original_account_ids: tab.and_then(|tab| tab.account_ids.clone()),
            original_address_view_ids: tab.and_then(|tab| tab.address_view_ids.clone()),
            membership_dirty: false,
            accounts: tab
                .map(|tab| {
                    initial_tab_accounts(
                        tab.account_ids.as_deref(),
                        tab.resolved_scope(),
                        &presentation.accounts_in(&tab.profile_ids),
                        &app.get_connected_accounts()
                            .iter()
                            .map(|account| i64::from(account.id))
                            .collect::<Vec<_>>(),
                        !presentation.address_views.is_empty(),
                    )
                })
                .unwrap_or_default(),
            address_views: tab
                .map(|tab| {
                    tab.address_view_ids.clone().unwrap_or_else(|| {
                        presentation
                            .address_views
                            .iter()
                            .filter(|view| {
                                view.profile_id
                                    .as_ref()
                                    .is_some_and(|id| tab.profile_ids.contains(id))
                            })
                            .map(|view| view.id)
                            .collect()
                    })
                })
                .unwrap_or_default()
                .into_iter()
                .collect(),
        };
        let editor = app.global::<BusinessTabEditorUi>();
        editor.set_account_mode(false);
        editor.set_editing_id(id);
        editor.set_name(tab.map(|tab| tab.name.clone()).unwrap_or_default().into());
        editor.set_show_name(tab.map(|tab| tab.show_name).unwrap_or(true));
        editor.set_query(Default::default());
        editor.set_error(Default::default());
        editor.set_has_icon(initial.icon.is_some());
        editor.set_icon(preview.unwrap_or_default());
        editor.set_recipient_only(tab.is_some_and(|tab| tab.resolved_scope() == "recipients"));
        editor.set_has_special_rules(tab.is_some_and(|tab| {
            !tab.recipient_patterns.is_empty()
                || !tab.sender_addresses.is_empty()
                || !tab.sender_names.is_empty()
                || !tab.sender_domains.is_empty()
        }));
        let color = tab
            .map(|tab| presentation.business_tab_color(tab))
            .unwrap_or_else(|| {
                fallback_account_color(
                    i64::try_from(presentation.business_tabs.len()).unwrap_or_default(),
                )
            });
        show_editor_color(&editor, color);
        publish_tab_members(&app, &initial);
        *open_draft.borrow_mut() = Some(initial);
        editor.set_visible(true);
    });

    let weak = app.as_weak();
    let account_draft = Rc::clone(&draft);
    let account_state = Rc::clone(state);
    app.global::<BusinessTabEditorUi>()
        .on_open_account(move |key| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = account_state.borrow();
            let p = &state.account_presentation;
            let id = key
                .as_str()
                .strip_prefix("inbox:")
                .unwrap_or(key.as_str())
                .parse::<i64>()
                .unwrap_or(0);
            let account = if id == 0 {
                flectar_mail_core::models::InboxAccount {
                    id: 0,
                    name: String::new(),
                    icon: None,
                    color: None,
                    account_ids: vec![],
                    address_view_ids: vec![],
                    automatic_profile: None,
                    profile_id: None,
                }
            } else {
                let Some(account) = p.inbox_accounts.iter().find(|account| account.id == id) else {
                    return;
                };
                account.clone()
            };
            let initial = BusinessTabDraft {
                accounts: account.account_ids.iter().copied().collect(),
                address_views: p.inbox_members(&account).into_iter().collect(),
                icon: account.icon.clone(),
                inbox_account: Some(account.clone()),
                ..Default::default()
            };
            let editor = app.global::<BusinessTabEditorUi>();
            editor.set_account_mode(true);
            editor.set_editing_id(if id == 0 {
                "".into()
            } else {
                id.to_string().into()
            });
            editor.set_name(account.name.clone().into());
            editor.set_query(Default::default());
            editor.set_error(Default::default());
            let image = account.icon.as_ref().and_then(|relative| {
                state
                    .core
                    .as_ref()
                    .and_then(|core| load_profile_logo(&core.data_dir().join(relative)))
            });
            editor.set_has_icon(image.is_some());
            editor.set_icon(image.unwrap_or_default());
            editor.set_has_special_rules(false);
            editor.set_recipient_only(false);
            let color = p.saved_inbox_color(&account).unwrap_or_else(|| {
                fallback_account_color(if account.id == 0 {
                    i64::try_from(p.inbox_accounts.len()).unwrap_or_default()
                } else {
                    account.id
                })
            });
            show_editor_color(&editor, color);
            publish_tab_members(&app, &initial);
            *account_draft.borrow_mut() = Some(initial);
            editor.set_visible(true);
        });
    let weak = app.as_weak();
    let delete_state = Rc::clone(state);
    let delete_runtime = Rc::clone(runtime);
    app.global::<InboxAccountsUi>()
        .on_delete_account(move |key| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let Some(id) = key
                .strip_prefix("inbox:")
                .and_then(|id| id.parse::<i64>().ok())
            else {
                return;
            };
            let Some(core) = delete_state.borrow().core.clone() else {
                return;
            };
            match delete_runtime.block_on(core.account_preferences_core().delete_inbox_account(id))
            {
                Ok(settings) => apply_account_presentation_settings(
                    &app,
                    &delete_state,
                    &delete_runtime,
                    &settings,
                ),
                Err(error) => app.set_sync_status(UiMessage::detail(
                    "Could not delete account view: {}",
                    error.to_string(),
                )),
            }
        });
    let weak = app.as_weak();
    let search_draft = Rc::clone(&draft);
    app.global::<BusinessTabEditorUi>().on_search(move |query| {
        let Some(app) = weak.upgrade() else { return };
        app.global::<BusinessTabEditorUi>().set_query(query);
        if let Some(draft) = search_draft.borrow().as_ref() {
            publish_tab_members(&app, draft);
        }
    });
    let weak = app.as_weak();
    let member_draft = Rc::clone(&draft);
    app.global::<BusinessTabEditorUi>()
        .on_toggle_member(move |id, checked| {
            let Some(app) = weak.upgrade() else { return };
            if let Some(draft) = member_draft.borrow_mut().as_mut() {
                draft.set_choice(id.as_str(), checked);
                publish_tab_members(&app, draft);
            }
        });
    let weak = app.as_weak();
    let icon_draft = Rc::clone(&draft);
    let icon_state = Rc::clone(state);
    app.global::<BusinessTabEditorUi>().on_choose_icon(move || {
        let Some(app) = weak.upgrade() else { return };
        if icon_draft.borrow().is_none() {
            return;
        }
        let editor = app.global::<BusinessTabEditorUi>();
        let Some(core) = icon_state.borrow().core.clone() else {
            editor.set_error("Icon changes require local mail data.".into());
            return;
        };
        let Some(source) = choose_custom_icon_path() else {
            return;
        };
        let data_root = core.data_dir();
        match import_icon(
            &data_root,
            &source,
            if editor.get_account_mode() {
                IconKind::Account
            } else {
                IconKind::Tab
            },
        ) {
            Ok(relative) => {
                let Some(image) = load_profile_logo(&data_root.join(&relative)) else {
                    editor.set_error("Could not preview that icon. The draft is unchanged.".into());
                    return;
                };
                if let Some(draft) = icon_draft.borrow_mut().as_mut() {
                    draft.icon = Some(relative);
                }
                editor.set_icon(image);
                editor.set_has_icon(true);
                editor.set_error(Default::default());
            }
            Err(error) => editor.set_error(format!("Could not import icon: {error}").into()),
        }
    });
    let weak = app.as_weak();
    let remove_draft = Rc::clone(&draft);
    app.global::<BusinessTabEditorUi>().on_remove_icon(move || {
        let Some(app) = weak.upgrade() else { return };
        if let Some(draft) = remove_draft.borrow_mut().as_mut() {
            draft.icon = None;
        }
        let editor = app.global::<BusinessTabEditorUi>();
        editor.set_icon(Default::default());
        editor.set_has_icon(false);
        editor.set_error(Default::default());
    });
    let weak = app.as_weak();
    let cancel_draft = Rc::clone(&draft);
    app.global::<BusinessTabEditorUi>().on_cancel(move || {
        discard_business_tab_draft(&cancel_draft);
        if let Some(app) = weak.upgrade() {
            app.global::<BusinessTabEditorUi>().set_visible(false);
        }
    });
    let weak = app.as_weak();
    let save_draft = Rc::clone(&draft);
    let save_state = Rc::clone(state);
    let save_runtime = Rc::clone(runtime);
    app.global::<BusinessTabEditorUi>().on_save(move || {
        let Some(app) = weak.upgrade() else {
            return false;
        };
        let editor = app.global::<BusinessTabEditorUi>();
        let Some(core) = save_state.borrow().core.clone() else {
            editor.set_error("Tab changes require local mail data.".into());
            return false;
        };
        let borrowed = save_draft.borrow();
        let Some(draft) = borrowed.as_ref() else {
            return false;
        };
        let name = editor.get_name().trim().to_string();
        if name.is_empty() {
            editor.set_error(
                if editor.get_account_mode() {
                    "Enter an account name."
                } else {
                    "Enter a tab name."
                }
                .into(),
            );
            return false;
        }
        let result = if let Some(original) = &draft.inbox_account {
            let mut account = original.clone();
            account.name = name;
            account.icon = draft.icon.clone();
            if draft.membership_dirty || account.id == 0 {
                let (mailboxes, addresses) = draft.memberships_for_save();
                account.account_ids = mailboxes.unwrap_or_default();
                account.address_view_ids = addresses.unwrap_or_default();
                account.automatic_profile = None;
            }
            account.color = Some(theme::color_to_hex(editor.get_accent()));
            save_runtime.block_on(core.account_preferences_core().save_inbox_account(account))
        } else {
            let members = (draft.membership_dirty || draft.id.is_none())
                .then(|| draft.named_accounts.iter().copied().collect());
            save_runtime.block_on(core.account_preferences_core().save_account_tab(
                draft.id.clone(),
                name,
                draft.icon.clone(),
                editor.get_show_name(),
                members,
                Some(theme::color_to_hex(editor.get_accent())),
            ))
        };
        drop(borrowed);
        match result {
            Ok(settings) => {
                apply_account_presentation_settings(&app, &save_state, &save_runtime, &settings);
                *save_draft.borrow_mut() = None;
                editor.set_visible(false);
                editor.set_error(Default::default());
                app.set_sync_status(if editor.get_account_mode() {
                    UiMessage::plain("Account saved.")
                } else {
                    UiMessage::plain("Tab saved.")
                });
                true
            }
            Err(error) => {
                editor.set_error(format!("Could not save: {error}").into());
                false
            }
        }
    });

    let weak = app.as_weak();
    let icon_state = Rc::clone(state);
    let icon_runtime = Rc::clone(runtime);
    app.on_choose_account_icon(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let Some(core) = icon_state.borrow().core.clone() else {
            return;
        };
        let Some(source) = choose_custom_icon_path() else {
            return;
        };
        let relative = match import_icon(&core.data_dir(), &source, IconKind::Account) {
            Ok(relative) => relative,
            Err(error) => {
                app.set_sync_status(UiMessage::detail(
                    "Could not import account icon: {}",
                    error,
                ));
                return;
            }
        };
        match icon_runtime.block_on(core.set_account_icon(i64::from(id), Some(relative))) {
            Ok(settings) => {
                apply_account_presentation_settings(&app, &icon_state, &icon_runtime, &settings);
                app.set_sync_status(UiMessage::plain("Account icon saved."));
            }
            Err(error) => {
                app.set_sync_status(UiMessage::detail("Could not save account icon: {}", error))
            }
        }
    });
    let weak = app.as_weak();
    let remove_state = Rc::clone(state);
    let remove_runtime = Rc::clone(runtime);
    app.on_remove_account_icon(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let Some(core) = remove_state.borrow().core.clone() else {
            return;
        };
        match remove_runtime.block_on(core.set_account_icon(i64::from(id), None)) {
            Ok(settings) => {
                apply_account_presentation_settings(
                    &app,
                    &remove_state,
                    &remove_runtime,
                    &settings,
                );
                app.set_sync_status(UiMessage::plain("Account icon removed."));
            }
            Err(error) => app.set_sync_status(UiMessage::detail(
                "Could not remove account icon: {}",
                error,
            )),
        }
    });
}
