//! Opt-in screenshots of account branding using synthetic data only.
//! Run this test alone with FLECTAR_BRANDING_SCREENSHOTS set to an output folder.
//! No application startup, mail source, credentials, or persistent settings run.
use super::*;
use slint::{
    Rgb8Pixel,
    platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};

struct BrandingHeadless(Rc<MinimalSoftwareWindow>);
impl Platform for BrandingHeadless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn preview_icon() -> slint::Image {
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(64, 40);
    for (index, pixel) in pixels
        .make_mut_bytes()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let (x, y) = (index % 64, index / 64);
        pixel.copy_from_slice(if (6..58).contains(&x) && (6..34).contains(&y) {
            if x < 28 {
                &[38, 120, 190, 255]
            } else {
                &[32, 160, 130, 150]
            }
        } else {
            &[0, 0, 0, 0]
        });
    }
    slint::Image::from_rgba8(pixels)
}

fn accounts(icon: &slint::Image) -> Vec<AccountRow> {
    (1..=120)
        .map(|id| AccountRow {
            id,
            drag_key: format!("synthetic-account-{id}").into(),
            name: format!("Operations {id:03} - a deliberately long account name").into(),
            email: format!("operations-{id:03}@mail.example").into(),
            provider: "generic".into(),
            mail_protocol: "imap".into(),
            status: "idle".into(),
            initials: "OP".into(),
            avatar: icon.clone(),
            avatar_small: icon.clone(),
            has_avatar: true,
            has_custom_icon: id % 2 == 1,
            incoming_security: "tls".into(),
            smtp_security: "tls".into(),
            mail_history: "all".into(),
            sent_copy_policy: "auto".into(),
            ..Default::default()
        })
        .collect()
}

fn alias_choices(icon: &slint::Image) -> Vec<InboxViewChoiceRow> {
    [
        (
            1001,
            "team@community.example",
            "Source: operations-001@mail.example",
        ),
        (
            1002,
            "help@community.example",
            "Source account not connected: missing-source@mail.example",
        ),
        (
            1003,
            "contact@project.example",
            "Searches all connected accounts",
        ),
    ]
    .into_iter()
    .map(|(id, address, detail)| InboxViewChoiceRow {
        key: format!("alias:{id}").into(),
        kind: InboxViewKind::Alias,
        name: address.into(),
        detail: detail.into(),
        initials: "AI".into(),
        avatar: icon.clone(),
        has_avatar: true,
        checked: false,
        color: Default::default(),
    })
    .collect()
}

fn members(accounts: &[AccountRow], query: &str, empty_selection: bool) -> Vec<InboxViewChoiceRow> {
    let mut choices = accounts
        .iter()
        .map(|account| InboxViewChoiceRow {
            key: format!("account:{}", account.id).into(),
            kind: InboxViewKind::Account,
            name: account.name.clone(),
            detail: account.email.clone(),
            initials: account.initials.clone(),
            avatar: account.avatar_small.clone(),
            has_avatar: true,
            checked: !empty_selection && account.id <= 2,
            color: Default::default(),
        })
        .collect::<Vec<_>>();
    let icon = accounts
        .first()
        .map(|account| account.avatar.clone())
        .unwrap_or_default();
    choices.extend(alias_choices(&icon));
    let query = query.to_lowercase();
    choices.retain(|choice| {
        query.is_empty()
            || choice.name.to_lowercase().contains(&query)
            || choice.detail.to_lowercase().contains(&query)
    });
    choices
}

fn scroll(app: &AppWindow, width: u32, height: u32, distance: f32) {
    // Stay outside the nested account checklist so the settings page scrolls.
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(width as f32 - 12.0, height as f32 - 110.0),
        delta_x: 0.0,
        delta_y: -distance,
    });
}

fn capture(
    app: &AppWindow,
    window: &MinimalSoftwareWindow,
    width: u32,
    height: u32,
    scale: f32,
    path: Option<&std::path::Path>,
) {
    let (physical_width, physical_height) = (
        (width as f32 * scale) as u32,
        (height as f32 * scale) as u32,
    );
    app.window()
        .set_size(slint::PhysicalSize::new(physical_width, physical_height));
    let mut pixels = vec![Rgb8Pixel::default(); (physical_width * physical_height) as usize];
    let frames = if path.is_some() { 3 } else { 1 };
    for _ in 0..frames {
        slint::platform::update_timers_and_animations();
        if path.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
        app.window().request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, physical_width as usize);
        });
    }
    let Some(path) = path else { return };
    let bytes = pixels
        .iter()
        .flat_map(|pixel| [pixel.r, pixel.g, pixel.b])
        .collect();
    image::RgbImage::from_raw(physical_width, physical_height, bytes)
        .unwrap()
        .save(path)
        .unwrap();
}

#[test]
#[ignore = "writes synthetic UI screenshots; set FLECTAR_BRANDING_SCREENSHOTS and run alone"]
fn branding_surfaces_screenshots() {
    let out = std::path::PathBuf::from(
        std::env::var_os("FLECTAR_BRANDING_SCREENSHOTS")
            .expect("set FLECTAR_BRANDING_SCREENSHOTS to an output directory"),
    );
    std::fs::create_dir_all(&out).unwrap();
    let filter = |name: &str| {
        std::env::var(name)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<std::collections::HashSet<String>>()
    };
    let layouts = filter("FLECTAR_BRANDING_LAYOUTS");
    let states = filter("FLECTAR_BRANDING_STATES");
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(BrandingHeadless(window.clone()))).unwrap();
    let app = AppWindow::new().unwrap();
    app.global::<MotionSettings>().set_enabled(false);
    app.global::<WindowChrome>().set_platform("windows".into());
    app.global::<WindowChrome>().set_integrated(true);
    app.set_startup_ready(true);
    app.set_startup_hydrated(true);
    app.set_active_view("mail".into());
    app.set_settings_tab("Accounts".into());
    let icon = preview_icon();
    let accounts = accounts(&icon);
    app.set_connected_accounts(ModelRc::new(VecModel::from(accounts.clone())));
    app.set_address_inboxes(ModelRc::new(VecModel::from(alias_choices(&icon))));
    app.global::<InboxAccountsUi>()
        .set_accounts(ModelRc::new(VecModel::from(
            ["Design Studio", "Customer Support", "Community Services"]
                .iter()
                .enumerate()
                .map(|(i, name)| InboxViewChoiceRow {
                    key: format!("inbox:-{}", i + 1).into(),
                    kind: InboxViewKind::NamedAccount,
                    name: (*name).into(),
                    detail: "2 mailboxes · 14 addresses".into(),
                    initials: "DS".into(),
                    avatar: icon.clone(),
                    has_avatar: true,
                    checked: i != 2,
                    color: slint::Color::from_rgb_u8(0x3b, 0x82, 0xf6),
                })
                .collect::<Vec<_>>(),
        )));

    app.set_business_tabs(ModelRc::new(VecModel::from(
        (1..=5)
            .map(|id| BusinessTabRow {
                id: format!("synthetic-tab-{id}").into(),
                name: format!("Operations and community services - deliberately long tab {id}")
                    .into(),
                show_name: id == 1 || id == 4,
                initials: format!("G{id}").into(),
                color: slint::Color::from_rgb_u8(38, 120, 190),
                logo: icon.clone(),
                has_logo: id != 3 && id != 4,
                active: id == 1,
            })
            .collect::<Vec<_>>(),
    )));
    app.global::<TabsUi>().set_rows(ModelRc::new(VecModel::from(
        (1..=6)
            .map(|id| TabRow {
                id,
                workspace: "mail".into(),
                title: format!("Synthetic workspace {id}").into(),
                active: id == 1,
            })
            .collect::<Vec<_>>(),
    )));
    app.show().unwrap();

    for (width, height, scale) in [
        (1320, 900, 1.0),
        (1000, 700, 1.0),
        (800, 700, 1.0),
        (320, 700, 1.0),
        (1320, 900, 2.0),
    ] {
        let layout = format!("{width}x{height}-{}x", scale as u32);
        if !layouts.is_empty() && !layouts.contains(&layout) {
            continue;
        }
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        for theme in ["light", "dark"] {
            app.set_theme_mode(theme.into());
            let prefix = format!("{theme}-{width}x{height}-{}x", scale as u32);
            let draw = |name: &str| {
                let path = out.join(format!("{prefix}-{name}.png"));
                let save = states.is_empty() || states.contains(name);
                capture(
                    &app,
                    &window,
                    width,
                    height,
                    scale,
                    save.then_some(path.as_path()),
                )
            };
            if width >= 980 {
                app.set_settings_open(false);
                app.set_sidebar_rows(ModelRc::new(VecModel::from(
                    alias_choices(&icon)
                        .into_iter()
                        .enumerate()
                        .map(|(index, view)| SidebarRow {
                            key: view.key.clone(),
                            kind: SidebarRowKind::AddressInbox,
                            open: true,
                            show_row: true,
                            reveal_row: true,
                            mailbox: MailboxRow {
                                account_id: -1,
                                folder_id: -1,
                                label: view.name,
                                detail: view.detail,
                                scope: format!("AddressInbox:{}", 1001 + index).into(),
                                avatar_image: view.avatar,
                                has_avatar: true,
                                is_selectable: true,
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .collect::<Vec<_>>(),
                )));
                app.set_selected_scope("AddressInbox:1002".into());
                draw("alias-sidebar-unconnected");
                let named = app.get_business_tabs().iter().collect::<Vec<_>>();
                let compact = named
                    .iter()
                    .cloned()
                    .map(|mut tab| {
                        tab.show_name = false;
                        tab
                    })
                    .collect::<Vec<_>>();
                app.set_business_tabs(ModelRc::new(VecModel::from(compact)));
                draw("group-strip-icon-only");
                app.window().dispatch_event(WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(221.0, 22.0),
                });
                draw("group-strip-icon-tooltip");
                app.window().dispatch_event(WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(10.0, 80.0),
                });
                app.set_business_tabs(ModelRc::new(VecModel::from(named)));
                draw("group-strip-mixed-names");
            }
            // Recreate only the declarative settings view to reset its scroll position.
            app.set_settings_open(false);
            app.global::<BusinessTabEditorUi>().set_visible(false);
            app.set_settings_open(true);
            draw("accounts-top");
            scroll(&app, width, height, -1_000_000.0);
            draw("accounts-top");
            scroll(&app, width, height, 650.0);
            draw("receiving-only-inbox-form");
            scroll(&app, width, height, 600.0);
            draw("account-icons");

            app.set_settings_open(false);
            let editor = app.global::<BusinessTabEditorUi>();
            editor.set_visible(true);
            editor.set_editing_id("synthetic-tab-1".into());
            editor.set_name("Operations and community services - independently branded tab".into());
            editor.set_show_name(false);
            editor.set_icon(icon.clone());
            editor.set_has_icon(true);
            editor.set_has_special_rules(true);
            editor.set_recipient_only(true);
            editor.set_error(Default::default());
            editor.set_query(Default::default());
            editor.set_selected_count(2);
            editor.set_members(ModelRc::new(VecModel::from(members(&accounts, "", false))));
            app.set_settings_open(true);
            draw("editor-before-scroll");
            scroll(&app, width, height, 400.0);
            draw("editor-many-accounts");
            scroll(&app, width, height, 300.0);
            draw("editor-save-cancel");

            editor.set_query("Operations 099".into());
            editor.set_selected_count(0);
            editor.set_members(ModelRc::new(VecModel::from(members(
                &accounts,
                "Operations 099",
                true,
            ))));
            draw("editor-search-empty-selection");
            editor.set_query("missing-source@mail.example".into());
            editor.set_members(ModelRc::new(VecModel::from(members(
                &accounts,
                "missing-source@mail.example",
                true,
            ))));
            draw("editor-alias-source-not-connected");
            editor.set_query("No matching accounts".into());
            editor.set_members(ModelRc::new(VecModel::from(
                Vec::<InboxViewChoiceRow>::new(),
            )));
            draw("editor-no-results");
            editor.set_account_mode(false);
            editor.set_name("Parent business".into());
            editor.set_query(Default::default());
            editor.set_recipient_only(false);
            editor.set_has_special_rules(false);
            editor.set_members(app.global::<InboxAccountsUi>().get_accounts());
            editor.set_selected_count(2);
            scroll(&app, width, height, -1_000_000.0);
            scroll(&app, width, height, 650.0);
            draw("named-tab-editor");
            editor.set_account_mode(true);
            editor.set_name("Design Studio".into());
            editor.set_members(ModelRc::new(VecModel::from(members(&accounts, "", false))));
            scroll(&app, width, height, -1_000_000.0);
            scroll(&app, width, height, 400.0);
            draw("named-account-editor");
            editor.set_account_mode(false);
        }
    }
    app.hide().unwrap();
    eprintln!("Branding screenshots: {}", out.display());
}
