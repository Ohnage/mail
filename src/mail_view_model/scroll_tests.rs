//! Scroll position across a background list refresh and a body refresh.
use super::*;
use slint::platform::{
    Platform, WindowAdapter,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn settle(window: &MinimalSoftwareWindow) {
    let mut pixels = vec![slint::Rgb8Pixel::default(); 1280 * 900];
    for _ in 0..8 {
        slint::platform::update_timers_and_animations();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 1280);
        });
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn on_its_own_thread(test: impl FnOnce() + Send + 'static) {
    // Slint's platform is one per thread. A fresh thread keeps this render
    // proof independent of whichever other UI test the runner placed first.
    std::thread::Builder::new()
        .name("mail-scroll".into())
        .spawn(test)
        .expect("scroll test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

fn harness() -> (
    AppWindow,
    Rc<MinimalSoftwareWindow>,
    tokio::runtime::Runtime,
) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let app = AppWindow::new().unwrap();
    app.set_startup_ready(true);
    app.set_startup_hydrated(true);
    app.set_active_view("mail".into());
    app.set_group_mail_by_date(false);
    app.global::<MotionSettings>().set_enabled(false);
    app.set_connected_accounts(ModelRc::new(VecModel::from(vec![AccountRow {
        id: 1,
        ..Default::default()
    }])));
    app.window().set_size(slint::PhysicalSize::new(1280, 900));
    app.show().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    (app, window, runtime)
}

fn inbox_message(id: i32, when: i64) -> MailMessage {
    let mut message = super::tests::message(id);
    message.folder = "Inbox".into();
    message.subject = format!("Message {id}");
    message.preview = format!("Preview {id}");
    message.date_ms = when;
    message.body_pending = false;
    message.html = Some(format!("<p>Message {id}</p>"));
    message.text = Some(message.preview.clone());
    message
}

#[test]
fn repeated_refresh_keeps_a_scrolled_list_on_the_same_message() {
    on_its_own_thread(repeated_refresh_keeps_a_scrolled_list_on_the_same_message_body);
}

fn repeated_refresh_keeps_a_scrolled_list_on_the_same_message_body() {
    let (app, window, runtime) = harness();
    let dir = tempfile::tempdir().unwrap();
    let (favicons, _) = bounded_ui_channel();
    let (avatars, _) = bounded_ui_channel();
    let now = chrono::Local::now().timestamp_millis();
    let mut state = InboxState::empty(
        None,
        UiSender::new(favicons, UiWake::new(app.as_weak(), |_| {})),
        None,
        UiSender::new(avatars, UiWake::new(app.as_weak(), |_| {})),
        false,
        false,
        WarmStartCacheWriter::spawn(&runtime, dir.path().join("warm.json")),
    );
    state.using_core = true;
    state.messages = (1..=80)
        .map(|id| inbox_message(id, now - i64::from(id)))
        .collect();
    state.selected_id = Some(8);
    state.preview_closed = false;
    app.set_emails(ModelRc::from(state.email_rows.clone()));
    app.set_mail_list_entries(ModelRc::from(state.mail_list_entries.clone()));
    let state = Rc::new(RefCell::new(state));
    render_current(&app, &state, &runtime).unwrap();
    settle(&window);

    let row = app.global::<TabsUi>().get_list_row_height();
    assert!(
        row > 1.0,
        "the message list has to lay out before a refresh can preserve it, height {row}"
    );
    let hold = row * 12.0;
    let ui = app.global::<TabsUi>();
    ui.set_list_hold(hold);
    ui.set_list_hold_request(ui.get_list_hold_request().wrapping_add(1));
    settle(&window);
    let offset = app.global::<TabsUi>().get_list_offset();
    assert!(
        offset > row * 8.0,
        "the list should be scrolled down before the refresh, offset {offset} row {row}"
    );

    let selected = state.borrow().selected_id;
    state
        .borrow_mut()
        .messages
        .insert(0, inbox_message(100, now + 1));
    refresh_rows_only(&app, &state, &runtime);
    let immediate = app.global::<TabsUi>().get_list_offset();
    settle(&window);

    let after = app.global::<TabsUi>().get_list_offset();
    let hold_after = app.global::<TabsUi>().get_list_hold();
    assert!(
        after > row * 8.0,
        "inserting a newer message must not scroll the list back to the top, offset {after} hold {hold_after}"
    );
    assert!(
        (after - (offset + row)).abs() < row * 0.75,
        "the same row should stay under the viewport, before {offset} immediate {immediate} after {after} row {row} hold {hold_after}"
    );
    assert_eq!(state.borrow().selected_id, selected);
    assert_eq!(app.global::<EmailReader>().get_message_id(), 8);
}

#[test]
fn refreshing_the_open_message_does_not_jump_the_reader_to_the_top() {
    on_its_own_thread(refreshing_the_open_message_does_not_jump_the_reader_to_the_top_body);
}

fn refreshing_the_open_message_does_not_jump_the_reader_to_the_top_body() {
    let (app, window, runtime) = harness();
    let dir = tempfile::tempdir().unwrap();
    let (favicons, _) = bounded_ui_channel();
    let (avatars, _) = bounded_ui_channel();
    let now = chrono::Local::now().timestamp_millis();
    let mut state = InboxState::empty(
        None,
        UiSender::new(favicons, UiWake::new(app.as_weak(), |_| {})),
        None,
        UiSender::new(avatars, UiWake::new(app.as_weak(), |_| {})),
        false,
        false,
        WarmStartCacheWriter::spawn(&runtime, dir.path().join("warm.json")),
    );
    state
        .email_renderer
        .borrow_mut()
        .configure_resources(runtime.handle().clone(), false)
        .unwrap();
    let mut open = inbox_message(1, now);
    open.html = Some(
        "<html><body style='margin:0;padding:20px'><h2>Open</h2><div style='height:4000px'></div><p>End of the open message.</p></body></html>"
            .into(),
    );
    let older = inbox_message(2, now - 1);
    state.messages = vec![open.clone(), older];
    state.selected_id = Some(1);
    state.preview_closed = false;
    state.conversation_owner_id = Some(1);
    state.conversation_messages = vec![open.clone()];
    state.conversation_selected_index = 0;
    app.set_emails(ModelRc::from(state.email_rows.clone()));
    app.set_mail_list_entries(ModelRc::from(state.mail_list_entries.clone()));
    let state = Rc::new(RefCell::new(state));
    render_current(&app, &state, &runtime).unwrap();
    settle(&window);
    let document = app.get_email_content_aspect() * app.get_email_viewport_width();
    assert!(
        document > app.get_email_viewport_height() * 2.0,
        "the open message has to be taller than the viewport, document {document}"
    );
    app.set_email_scroll_y(-1500.0);
    app.invoke_email_scroll(1500.0, app.get_email_viewport_height());
    settle(&window);
    let scrolled = app.get_email_scroll_y();
    assert!(
        scrolled < -400.0,
        "the reader should be scrolled down before the refresh, scroll {scrolled}"
    );

    {
        let mut state = state.borrow_mut();
        let open = state
            .messages
            .iter_mut()
            .find(|message| message.id == 1)
            .expect("the open message stays in the list");
        open.body_pending = true;
        open.html = None;
    }
    render_current(&app, &state, &runtime).unwrap();
    settle(&window);
    let during_refresh = app.get_email_scroll_y();
    assert!(
        (during_refresh - scrolled).abs() < 2.0,
        "a pending body refresh must keep the reader where it was, before {scrolled} after {during_refresh}"
    );
    assert_eq!(app.global::<EmailReader>().get_message_id(), 1);

    let moved = inbox_message(99, now + 5);
    {
        let mut state = state.borrow_mut();
        let open = state
            .messages
            .iter_mut()
            .find(|message| message.id == 1)
            .expect("the open message stays in the list");
        open.body_pending = false;
        open.html = Some(
            "<html><body style='margin:0;padding:20px'><h2>Updated</h2><div style='height:4000px'></div><p>Still the same message.</p></body></html>"
                .into(),
        );
        let open = open.clone();
        state.rendered_id = Some(1);
        state.conversation_owner_id = Some(1);
        state.conversation_messages = vec![moved, open];
        state.conversation_selected_index = 1;
    }
    render_current(&app, &state, &runtime).unwrap();
    settle(&window);
    let after_reorder = app.get_email_scroll_y();
    assert!(
        after_reorder < -400.0,
        "moving the same message inside its thread must not jump to the top, scroll {after_reorder}"
    );
    assert_eq!(app.global::<EmailReader>().get_message_id(), 1);

    state.borrow_mut().conversation_selected_index = 0;
    render_current(&app, &state, &runtime).unwrap();
    settle(&window);
    assert_eq!(
        app.get_email_scroll_y(),
        0.0,
        "opening a different message starts at the top"
    );
    assert_eq!(app.global::<EmailReader>().get_message_id(), 99);
    assert_ne!(
        app.global::<EmailReader>().get_message_id(),
        1,
        "the previous message body is not left in place"
    );
}
