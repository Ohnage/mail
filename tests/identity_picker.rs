//! Long sender lists use a scrolling searchable picker instead of the
//! platform combo box, which only shows a few rows.
use slint::platform::{
    Platform, WindowAdapter, WindowEvent,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{cell::RefCell, rc::Rc};

slint::slint! {
    import { IdentitySelect } from "../ui/components/settings-components.slint";

    export component Harness inherits Window {
        in-out property <[string]> choices;
        in-out property <string> selected-label;
        in-out property <string> query;
        callback query-edited(string);
        callback picked(int);
        out property <length> scroll-offset: picker.scroll-offset;

        picker := IdentitySelect {
            x: 200px;
            y: 40px;
            width: 210px;
            choices: root.choices;
            selected-label: root.selected-label;
            query <=> root.query;
            query-edited(value) => { root.query-edited(value); }
            picked(index) => { root.picked(index); }
        }

        public function open() { picker.open-popup(); }
    }
}

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn paint(window: &MinimalSoftwareWindow, ui: &Harness) {
    slint::platform::update_timers_and_animations();
    ui.window().request_redraw();
    window.draw_if_needed(|renderer| {
        let mut pixels = vec![slint::Rgb8Pixel::default(); 800 * 700];
        renderer.render(&mut pixels, 800);
    });
}

#[test]
fn hundreds_of_addresses_can_be_searched_and_scrolled() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let ui = Harness::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(800, 700));
    let all = (0..500)
        .map(|index| SharedString::from(format!("alias{index}@example.com")))
        .collect::<Vec<_>>();
    let catalog = Rc::new(RefCell::new(all.clone()));
    ui.set_choices(ModelRc::new(VecModel::from(all)));
    ui.set_selected_label("alias0@example.com".into());
    ui.show().unwrap();

    let ui_filter = ui.as_weak();
    ui.on_query_edited(move |query| {
        let Some(ui_filter) = ui_filter.upgrade() else {
            return;
        };
        let query = query.trim().to_lowercase();
        let matched = catalog
            .borrow()
            .iter()
            .filter(|label| query.is_empty() || label.to_lowercase().contains(query.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        ui_filter.set_choices(ModelRc::new(VecModel::from(matched)));
    });
    let picked = Rc::new(RefCell::new(None));
    let picked_slot = picked.clone();
    let ui_pick = ui.as_weak();
    ui.on_picked(move |index| {
        let Some(ui_pick) = ui_pick.upgrade() else {
            return;
        };
        let label = usize::try_from(index)
            .ok()
            .and_then(|index| ui_pick.get_choices().row_data(index));
        *picked_slot.borrow_mut() = label.map(|label| label.to_string());
    });

    ui.invoke_open();
    paint(&window, &ui);
    assert_eq!(ui.get_choices().row_count(), 500);

    // Arrow keys move through the whole list, and the popup follows.
    for _ in 0..12 {
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::DownArrow.into(),
        });
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: slint::platform::Key::DownArrow.into(),
        });
    }
    paint(&window, &ui);
    assert!(
        ui.get_scroll_offset() < 0.0,
        "the highlight has to scroll past the first screen, offset {}",
        ui.get_scroll_offset()
    );

    for character in "alias499@example.com".chars() {
        let text = SharedString::from(character.to_string());
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }
    paint(&window, &ui);
    assert_eq!(
        ui.get_choices().row_count(),
        1,
        "search should keep one address"
    );
    assert_eq!(
        ui.get_choices().row_data(0).unwrap().as_str(),
        "alias499@example.com"
    );

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Return.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: slint::platform::Key::Return.into(),
    });
    assert_eq!(picked.borrow().as_deref(), Some("alias499@example.com"));
}
