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
    for (index, pixel) in pixels.make_mut_bytes().chunks_exact_mut(4).enumerate() {
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

fn members(accounts: &[AccountRow], query: &str, empty_selection: bool) -> Vec<ProfileMemberRow> {
    accounts
        .iter()
        .filter(|account| query.is_empty() || account.name.contains(query))
        .map(|account| ProfileMemberRow {
            id: account.id,
            name: account.name.clone(),
            email: account.email.clone(),
            initials: account.initials.clone(),
            avatar: account.avatar_small.clone(),
            has_avatar: true,
            checked: !empty_selection && account.id <= 2,
            other_profile: Default::default(),
        })
        .collect()
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
    path: &std::path::Path,
) {
    let (physical_width, physical_height) = (
        (width as f32 * scale) as u32,
        (height as f32 * scale) as u32,
    );
    app.window()
        .set_size(slint::PhysicalSize::new(physical_width, physical_height));
    let mut pixels = vec![Rgb8Pixel::default(); (physical_width * physical_height) as usize];
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(std::time::Duration::from_millis(60));
        app.window().request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, physical_width as usize);
        });
    }
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
    app.set_business_tabs(ModelRc::new(VecModel::from(
        (1..=3)
            .map(|id| BusinessTabRow {
                id: format!("synthetic-tab-{id}").into(),
                name: format!("Operations and community services - deliberately long tab {id}")
                    .into(),
                color: slint::Color::from_rgb_u8(38, 120, 190),
                logo: icon.clone(),
                has_logo: true,
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
        (800, 700, 1.0),
        (320, 700, 1.0),
        (1320, 900, 2.0),
    ] {
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        for theme in ["light", "dark"] {
            app.set_theme_mode(theme.into());
            let prefix = format!("{theme}-{width}x{height}-{}x", scale as u32);
            let draw = |name: &str| {
                capture(
                    &app,
                    &window,
                    width,
                    height,
                    scale,
                    &out.join(format!("{prefix}-{name}.png")),
                )
            };
            // Recreate only the declarative settings view to reset its scroll position.
            app.set_settings_open(false);
            app.global::<BusinessTabEditorUi>().set_visible(false);
            app.set_settings_open(true);
            draw("accounts-top");
            scroll(&app, width, height, 700.0);
            draw("account-icons");

            app.set_settings_open(false);
            let editor = app.global::<BusinessTabEditorUi>();
            editor.set_visible(true);
            editor.set_editing_id("synthetic-tab-1".into());
            editor.set_name("Operations and community services - independently branded tab".into());
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
            editor.set_query("No matching accounts".into());
            editor.set_members(ModelRc::new(VecModel::from(Vec::<ProfileMemberRow>::new())));
            draw("editor-no-results");
        }
    }
    app.hide().unwrap();
    eprintln!("Branding screenshots: {}", out.display());
}
