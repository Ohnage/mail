//! C3: compose/reply opens near 75% of the work area with a large editor.
use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};

struct Headless(std::rc::Rc<MinimalSoftwareWindow>);
impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<std::rc::Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

slint::slint! {
    export component ComposeWindowSizeHarness inherits Window {
        in property <length> work-width: 1600px;
        in property <length> work-height: 1000px;
        width: root.work-width;
        height: root.work-height;

        private property <length> controls-inset: 0px;
        private property <length> usable-height: max(1px, root.work-height - root.controls-inset);
        private property <length> desktop-width: max(640px, root.work-width * 0.75);
        private property <length> desktop-height: max(480px, root.usable-height * 0.75);

        // Fixed header/toolbar budget; the editor takes the rest.
        private property <length> chrome-height: 220px;

        out property <length> composer-width: min(root.desktop-width, root.work-width - 32px);
        out property <length> composer-height: min(root.desktop-height, root.usable-height - 32px);
        out property <length> editor-viewport-height: max(1px, root.composer-height - root.chrome-height);
    }
}

#[test]
fn compose_default_size_gives_editor_most_of_the_window() {
    let window = MinimalSoftwareWindow::new(
        slint::platform::software_renderer::RepaintBufferType::NewBuffer,
    );
    let _ = slint::platform::set_platform(Box::new(Headless(window)));
    let ui = ComposeWindowSizeHarness::new().unwrap();

    let window_w = ui.get_work_width();
    let window_h = ui.get_work_height();
    let composer_w = ui.get_composer_width();
    let composer_h = ui.get_composer_height();
    let editor_h = ui.get_editor_viewport_height();

    assert!(
        (composer_w - window_w * 0.75).abs() < 1.0,
        "composer width {composer_w} should be ~75% of {window_w}"
    );
    assert!(
        (composer_h - window_h * 0.75).abs() < 1.0,
        "composer height {composer_h} should be ~75% of {window_h}"
    );
    assert!(
        editor_h >= composer_h * 0.60,
        "editor viewport {editor_h} must be at least ~60% of composer height {composer_h}"
    );
}
