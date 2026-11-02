//! The supplied application identity, shared by every legacy icon choice.
//!
//! The window icon and the in-app logos bind to the `AppIcon` Slint global,
//! which is all Windows and X11 window managers need. The other places that
//! show the icon are outside Slint's reach:
//!
//! - the tray is a separate Slint compilation unit and gets its image here;
//! - macOS draws the Dock and app switcher icon from the application, not the
//!   window;
//! - Linux launchers, and Wayland taskbars, look the icon up in the icon
//!   theme by the desktop entry's name. A copy in the user's own icon theme
//!   takes precedence over the installed one;
//! - mobile home screens switch to icons bundled with the app, through the
//!   platform layer.
//!
//! Flatpak can neither show a tray nor write to the host's icon theme, and
//! some mobile platforms reserve switching the home screen icon for system
//! apps, so the choice is not offered there.

use crate::{AppIconVariant, AppWindow, tray_ui::FlectarTray};
use flectar_mail_core::models;
use slint::ComponentHandle;
use std::cell::RefCell;

#[cfg(desktop)]
pub(crate) const AVAILABLE: bool = cfg!(not(feature = "flatpak"));
#[cfg(mobile)]
pub(crate) const AVAILABLE: bool = crate::platform::SWITCHES_APP_ICON;

/// Lets Settings describe where this platform shows the icon.
#[cfg(desktop)]
const PLATFORM: &str = crate::window_chrome::PLATFORM;
#[cfg(mobile)]
const PLATFORM: &str = crate::platform::NAME;

thread_local! {
    static TRAY: RefCell<Option<slint::Weak<FlectarTray>>> = const { RefCell::new(None) };
}

pub(crate) fn variant(icon: models::AppIcon) -> AppIconVariant {
    match icon {
        models::AppIcon::Orange => AppIconVariant::Orange,
        models::AppIcon::Graphite => AppIconVariant::Graphite,
        models::AppIcon::Light => AppIconVariant::Light,
        models::AppIcon::Blue => AppIconVariant::Blue,
        models::AppIcon::Green => AppIconVariant::Green,
        models::AppIcon::Purple => AppIconVariant::Purple,
    }
}

pub(crate) fn choice(variant: AppIconVariant) -> models::AppIcon {
    match variant {
        AppIconVariant::Orange => models::AppIcon::Orange,
        AppIconVariant::Graphite => models::AppIcon::Graphite,
        AppIconVariant::Light => models::AppIcon::Light,
        AppIconVariant::Blue => models::AppIcon::Blue,
        AppIconVariant::Green => models::AppIcon::Green,
        AppIconVariant::Purple => models::AppIcon::Purple,
    }
}

/// Show `icon` everywhere the platform displays it. Call it before the window
/// is first shown so the first frame already carries the chosen icon.
pub(crate) fn apply(app: &AppWindow, icon: models::AppIcon) {
    let icon = if AVAILABLE {
        icon
    } else {
        models::AppIcon::default()
    };
    let global = app.global::<crate::AppIcon>();
    global.set_available(AVAILABLE);
    global.set_platform(PLATFORM.into());
    global.set_variant(variant(icon));
    TRAY.with(|tray| {
        if let Some(tray) = tray.borrow().as_ref().and_then(slint::Weak::upgrade) {
            tray.set_image(global.invoke_tray_image(variant(icon)));
        }
    });
    #[cfg(target_os = "macos")]
    dock::show(icon);
    #[cfg(all(linux_desktop, not(feature = "flatpak")))]
    launcher::request(icon);
    #[cfg(mobile)]
    crate::platform::show_app_icon(icon);
}

/// Keep the tray's image in step with the chosen icon.
pub(crate) fn attach_tray(app: &AppWindow, tray: &FlectarTray) {
    let global = app.global::<crate::AppIcon>();
    tray.set_image(global.invoke_tray_image(global.get_variant()));
    TRAY.with(|slot| *slot.borrow_mut() = Some(tray.as_weak()));
}

/// The supplied design for every saved legacy variant.
#[cfg(any(target_os = "macos", all(linux_desktop, not(feature = "flatpak"))))]
fn png_512(_icon: models::AppIcon) -> Option<&'static [u8]> {
    Some(include_bytes!(
        "../resources/app-icon/flectar-mail-masked-512.png"
    ))
}

#[cfg(target_os = "macos")]
mod dock {
    use flectar_mail_core::models::AppIcon;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    use std::cell::Cell;

    thread_local! {
        /// The bundle's icon shows until the application sets another.
        static SHOWN: Cell<AppIcon> = const { Cell::new(AppIcon::Orange) };
    }

    pub(super) fn show(icon: AppIcon) {
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };
        if SHOWN.get() == icon {
            return;
        }
        let image = match super::png_512(icon) {
            Some(png) => match NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png)) {
                Some(image) => Some(image),
                None => {
                    eprintln!("could not decode the {} Dock icon", icon.as_str());
                    return;
                }
            },
            None => None,
        };
        // SAFETY: AppKit documents nil as restoring the bundle's icon.
        unsafe {
            NSApplication::sharedApplication(main_thread).setApplicationIconImage(image.as_deref());
        }
        SHOWN.set(icon);
    }
}

/// Copies of the chosen icon in the user's hicolor icon theme, under the name
/// the desktop entry uses. Only files whose contents are one of the bundled
/// variants are ever replaced or removed, so an icon the user put there
/// themselves is left alone.
#[cfg(all(linux_desktop, not(feature = "flatpak")))]
mod launcher {
    use flectar_mail_core::models::AppIcon;
    use std::{
        io::{self, Write},
        path::{Path, PathBuf},
        sync::{Mutex, PoisonError},
        time::SystemTime,
    };

    /// Where the theme looks for the installed icon, matching the scalable
    /// and 512 px copies the packages install.
    const SLOTS: [&str; 2] = [
        "scalable/apps/com.flectar.mail.svg",
        "512x512/apps/com.flectar.mail.png",
    ];

    /// The latest icon to write; a single change in flight applies whichever
    /// icon was chosen last.
    static REQUESTED: Mutex<Option<AppIcon>> = Mutex::new(None);
    static WRITING: Mutex<()> = Mutex::new(());

    pub(super) fn request(icon: AppIcon) {
        // Tests must not rewrite the developer's own launcher icon.
        if cfg!(test) {
            return;
        }
        *REQUESTED.lock().unwrap_or_else(PoisonError::into_inner) = Some(icon);
        let spawned = std::thread::Builder::new()
            .name("launcher-icon".into())
            .spawn(|| {
                let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
                let Some(icon) = REQUESTED
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                else {
                    return;
                };
                if let Err(error) = hicolor_dir().and_then(|theme| sync(&theme, icon)) {
                    eprintln!("could not update the launcher icon: {error}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("could not update the launcher icon: {error}");
        }
    }

    fn hicolor_dir() -> io::Result<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".local/share"))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "neither XDG_DATA_HOME nor HOME is set",
                )
            })?;
        Ok(data.join("icons/hicolor"))
    }

    fn files(_icon: AppIcon) -> Option<[&'static [u8]; 2]> {
        Some([
            include_bytes!("../resources/app-icon/flectar-mail-masked.svg"),
            super::png_512(_icon)?,
        ])
    }

    fn bundled(slot: usize, contents: &[u8]) -> bool {
        use sha2::{Digest, Sha256};

        if files(AppIcon::Orange).is_some_and(|files| files[slot] == contents) {
            return true;
        }
        // Recognize only exact copies written by older releases. This lets
        // upgrades replace the former logo without overwriting a user's art.
        // See resources/app-icon/legacy-managed-sha256.json for provenance.
        const LEGACY: [&str; 10] = [
            "fcb37a3dfb6fd95212180edc46f53d0cf3efcc574b81f0f779d15db006281d93",
            "97cc02dea90b067ecbee5fbda4a9eae09b6f699ad629bf0139bb34992deba128",
            "7111b5d0ff84a61af71201fe256ad6ffe3f23d46d5cd618cf5e8139124ea15de",
            "9d0cfdf7cd02b89d6c38fafa86ad3429e382bffc73e6dd5cad22141ddd97696e",
            "b20d24ba619806bcfbf6ae3236398e24a3b583926e8c5bac8bf8dc22fb2d4b62",
            "0b44e16c8327b1e9f153d235040fbe79d3efad03a547d9fe2d44b132c59f5b55",
            "b2ea27324833e2834a5d8b0b25e3bd8f55567eeae7e85a9518be6dc4868a86b3",
            "e0d8b84b35e1a71a94ac31a5a4687d0275095724506454c69149cd978b26857e",
            "2e874d0c60f2e6269a5d9f0c02ef671c780f9bddf8bffe99fcab10f7080e4b4b",
            "8bf84b413d7649e5edbf04e4eaec20bb4fe7b888bf2f5fed8454713d04db2c69",
        ];
        let digest = format!("{:x}", Sha256::digest(contents));
        LEGACY.contains(&digest.as_str())
    }

    /// Make `theme` show `icon`. Returns whether anything changed.
    pub(super) fn sync(theme: &Path, icon: AppIcon) -> io::Result<bool> {
        let wanted = files(icon);
        let mut changed = false;
        for (slot, relative) in SLOTS.into_iter().enumerate() {
            let path = theme.join(relative);
            let current = match std::fs::read(&path) {
                Ok(contents) => Some(contents),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            if current
                .as_deref()
                .is_some_and(|contents| !bundled(slot, contents))
            {
                eprintln!("keeping the user's own icon at {}", path.display());
                continue;
            }
            match (wanted, current) {
                (Some(files), current) if current.as_deref() != Some(files[slot]) => {
                    write(&path, files[slot])?;
                    changed = true;
                }
                (None, Some(_)) => {
                    std::fs::remove_file(&path)?;
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            // GTK, GNOME Shell and KDE notice theme changes, and ignore a
            // stale icon cache, by the theme directory's modification time.
            std::fs::File::open(theme)?.set_modified(SystemTime::now())?;
        }
        Ok(changed)
    }

    fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("the icon path has no parent"))?;
        std::fs::create_dir_all(directory)?;
        // Replace atomically so the theme never sees a truncated icon.
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(contents)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn contents(theme: &Path, slot: usize) -> Option<Vec<u8>> {
            std::fs::read(theme.join(SLOTS[slot])).ok()
        }

        #[test]
        fn every_saved_choice_uses_the_supplied_identity() {
            let directory = tempfile::tempdir().unwrap();
            let theme = directory.path().join("icons/hicolor");
            assert!(sync(&theme, AppIcon::Orange).unwrap());
            let expected = files(AppIcon::Orange).unwrap();
            for icon in AppIcon::ALL {
                assert_eq!(files(icon), Some(expected));
                assert!(!sync(&theme, icon).unwrap());
                for (slot, file) in expected.into_iter().enumerate() {
                    assert_eq!(contents(&theme, slot).as_deref(), Some(file));
                }
            }
        }

        #[test]
        fn the_users_own_icon_is_never_replaced() {
            let directory = tempfile::tempdir().unwrap();
            let theme = directory.path().join("icons/hicolor");
            let own = theme.join(SLOTS[0]);
            std::fs::create_dir_all(own.parent().unwrap()).unwrap();
            std::fs::write(&own, b"<svg>mine</svg>").unwrap();
            assert!(sync(&theme, AppIcon::Blue).unwrap());
            assert_eq!(
                contents(&theme, 0).as_deref(),
                Some(&b"<svg>mine</svg>"[..])
            );
            assert_eq!(
                contents(&theme, 1).as_deref(),
                Some(files(AppIcon::Blue).unwrap()[1])
            );
            assert!(!sync(&theme, AppIcon::Orange).unwrap());
            assert_eq!(
                contents(&theme, 0).as_deref(),
                Some(&b"<svg>mine</svg>"[..])
            );
        }

        #[test]
        fn changes_mark_the_theme_directory_as_modified() {
            let directory = tempfile::tempdir().unwrap();
            let theme = directory.path().join("icons/hicolor");
            for relative in SLOTS {
                std::fs::create_dir_all(theme.join(relative).parent().unwrap()).unwrap();
            }
            let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
            let modified = || std::fs::metadata(&theme).unwrap().modified().unwrap();
            std::fs::File::open(&theme)
                .unwrap()
                .set_modified(old)
                .unwrap();
            assert!(sync(&theme, AppIcon::Green).unwrap());
            assert!(modified() > old);
            std::fs::File::open(&theme)
                .unwrap()
                .set_modified(old)
                .unwrap();
            assert!(!sync(&theme, AppIcon::Orange).unwrap());
            assert_eq!(modified(), old);
        }
    }
}

#[cfg(all(test, desktop))]
mod tests {
    use super::*;
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use std::rc::Rc;

    struct Headless(Rc<MinimalSoftwareWindow>);
    impl slint::platform::Platform for Headless {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn legacy_choices_share_the_supplied_images() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window))).unwrap();
        let app = AppWindow::new().unwrap();
        let global = app.global::<crate::AppIcon>();
        let expected = global.invoke_window_image(AppIconVariant::Orange);
        let expected_tray = global.invoke_tray_image(AppIconVariant::Orange);
        let expected_logo = global.invoke_logo(AppIconVariant::Orange);
        for icon in models::AppIcon::ALL {
            assert_eq!(choice(variant(icon)), icon);
            assert_eq!(global.invoke_window_image(variant(icon)), expected);
            assert_eq!(global.invoke_tray_image(variant(icon)), expected_tray);
            assert_eq!(global.invoke_logo(variant(icon)), expected_logo);
        }
        assert_eq!((expected.size().width, expected.size().height), (256, 256));
        assert_eq!(
            (expected_tray.size().width, expected_tray.size().height),
            (64, 64)
        );
        apply(&app, models::AppIcon::Graphite);
        assert_eq!(global.get_available(), AVAILABLE);
        assert_eq!(
            global.get_variant(),
            if AVAILABLE {
                AppIconVariant::Graphite
            } else {
                AppIconVariant::Orange
            }
        );
    }
}
