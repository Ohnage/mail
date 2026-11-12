//! C2: programmatic selection refresh must not yank compose scroll to the caret.
use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};

struct Headless(std::rc::Rc<MinimalSoftwareWindow>);
impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<std::rc::Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

slint::slint! {
    export component ComposeCaretRevealHarness inherits Window {
        in-out property <length> content-y: -240px;
        private property <bool> reveal-after-user-input: false;

        public function on-cursor-position-changed() {
            if (!root.reveal-after-user-input) {
                return;
            }
            root.reveal-after-user-input = false;
            // Simplified reveal: snap toward the caret.
            root.content-y = 0px;
        }

        public function programmatic-selection-refresh() {
            root.on-cursor-position-changed();
        }

        public function user-typed() {
            root.reveal-after-user-input = true;
            root.on-cursor-position-changed();
        }
    }
}

#[test]
fn programmatic_selection_refresh_does_not_snap_compose_scroll() {
    let window = MinimalSoftwareWindow::new(
        slint::platform::software_renderer::RepaintBufferType::NewBuffer,
    );
    // Platform may already be set by another headless test in this binary.
    let _ = slint::platform::set_platform(Box::new(Headless(window)));
    let ui = ComposeCaretRevealHarness::new().unwrap();
    assert_eq!(ui.get_content_y(), -240.0);

    ui.invoke_programmatic_selection_refresh();
    assert_eq!(
        ui.get_content_y(),
        -240.0,
        "programmatic text/selection refresh must leave content-y alone"
    );

    ui.invoke_user_typed();
    assert_eq!(
        ui.get_content_y(),
        0.0,
        "typing must still reveal the caret"
    );
}
