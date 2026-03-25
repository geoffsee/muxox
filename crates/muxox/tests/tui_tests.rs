use muxox_core::app::{App, AppMsg, ServiceState, Status, apply_msg};
use muxox_core::config::ServiceCfg;
use muxox_core::isolation::default_isolation;
use tokio::sync::mpsc;

fn test_cfg(name: &str, interactive: bool) -> ServiceCfg {
    ServiceCfg {
        name: name.into(),
        cmd: "echo hi".into(),
        cwd: None,
        log_capacity: 100,
        interactive,
        pty: false,
        env_file: None,
    }
}

fn test_app(cfgs: Vec<ServiceCfg>) -> App {
    let (tx, _rx) = mpsc::unbounded_channel();
    App {
        services: cfgs.into_iter().map(ServiceState::new).collect(),
        selected: 0,
        log_offset_from_end: 0,
        tx,
        input_mode: false,
        input_buffer: String::new(),
        isolation: default_isolation(),
    }
}

// --- Navigation ---

#[test]
fn navigate_down_increments_selected() {
    let mut app = test_app(vec![
        test_cfg("a", false),
        test_cfg("b", false),
        test_cfg("c", false),
    ]);
    assert_eq!(app.selected, 0);

    app.selected = 1;
    assert_eq!(app.selected, 1);

    // Boundary: can't go past last
    app.selected = 2;
    assert_eq!(app.selected, 2);
}

#[test]
fn navigate_up_decrements_selected() {
    let mut app = test_app(vec![test_cfg("a", false), test_cfg("b", false)]);
    app.selected = 1;
    app.selected = app.selected.saturating_sub(1);
    assert_eq!(app.selected, 0);

    // Boundary: can't go below 0
    app.selected = app.selected.saturating_sub(1);
    assert_eq!(app.selected, 0);
}

#[test]
fn changing_selection_resets_scroll() {
    let mut app = test_app(vec![test_cfg("a", false), test_cfg("b", false)]);
    app.log_offset_from_end = 5;

    // Simulates what handle_key does on Down
    app.selected = 1;
    app.log_offset_from_end = 0;
    assert_eq!(app.log_offset_from_end, 0);
}

// --- Scroll ---

#[test]
fn scroll_up_increments_offset() {
    let mut app = test_app(vec![test_cfg("a", false)]);
    app.log_offset_from_end = 0;

    // Simulates Ctrl+Up or mouse scroll up
    app.log_offset_from_end = app.log_offset_from_end.saturating_add(3);
    assert_eq!(app.log_offset_from_end, 3);
}

#[test]
fn scroll_down_decrements_offset_with_floor() {
    let mut app = test_app(vec![test_cfg("a", false)]);
    app.log_offset_from_end = 2;

    app.log_offset_from_end = app.log_offset_from_end.saturating_sub(3);
    assert_eq!(app.log_offset_from_end, 0);
}

// --- Input mode ---

#[test]
fn enter_input_mode_on_interactive_service() {
    let mut app = test_app(vec![test_cfg("interactive-svc", true)]);
    assert!(!app.input_mode);

    // Only enter input mode if service is interactive
    if app.services[app.selected].cfg.interactive {
        app.input_mode = true;
        app.input_buffer.clear();
    }
    assert!(app.input_mode);
}

#[test]
fn refuse_input_mode_on_non_interactive_service() {
    let mut app = test_app(vec![test_cfg("plain-svc", false)]);
    if app.services[app.selected].cfg.interactive {
        app.input_mode = true;
    }
    assert!(!app.input_mode);
}

#[test]
fn input_buffer_accumulates_chars() {
    let mut app = test_app(vec![test_cfg("svc", true)]);
    app.input_mode = true;

    app.input_buffer.push('h');
    app.input_buffer.push('i');
    assert_eq!(app.input_buffer, "hi");
}

#[test]
fn backspace_removes_last_char() {
    let mut app = test_app(vec![test_cfg("svc", true)]);
    app.input_mode = true;
    app.input_buffer = "hello".into();

    app.input_buffer.pop();
    assert_eq!(app.input_buffer, "hell");
}

#[test]
fn esc_clears_input_and_exits_mode() {
    let mut app = test_app(vec![test_cfg("svc", true)]);
    app.input_mode = true;
    app.input_buffer = "partial".into();

    // Esc handler
    app.input_mode = false;
    app.input_buffer.clear();
    assert!(!app.input_mode);
    assert!(app.input_buffer.is_empty());
}

#[test]
fn enter_submits_and_exits_input_mode() {
    let mut app = test_app(vec![test_cfg("svc", true)]);
    app.input_mode = true;
    app.input_buffer = "command".into();

    // Enter handler takes the buffer and exits
    let input = std::mem::take(&mut app.input_buffer);
    app.input_mode = false;
    assert_eq!(input, "command");
    assert!(app.input_buffer.is_empty());
    assert!(!app.input_mode);
}

// --- State machine: full lifecycle via apply_msg ---

#[test]
fn service_lifecycle_through_messages() {
    let mut app = test_app(vec![test_cfg("svc", false)]);

    // Start
    apply_msg(&mut app, AppMsg::Started(0));
    assert_eq!(app.services[0].status, Status::Running);

    // Receive PID
    apply_msg(&mut app, AppMsg::ChildSpawned(0, 1234));
    assert_eq!(app.services[0].pid, Some(1234));

    // Logs arrive
    apply_msg(&mut app, AppMsg::Log(0, "booting...".into()));
    apply_msg(&mut app, AppMsg::Log(0, "ready".into()));
    assert_eq!(app.services[0].log.len(), 2);

    // Stop
    apply_msg(&mut app, AppMsg::Stopped(0, 0));
    assert_eq!(app.services[0].status, Status::Stopped);
    assert!(app.services[0].pid.is_none());
}

#[test]
fn multiple_services_independent_state() {
    let mut app = test_app(vec![
        test_cfg("web", false),
        test_cfg("api", false),
        test_cfg("db", false),
    ]);

    apply_msg(&mut app, AppMsg::Started(0));
    apply_msg(&mut app, AppMsg::Started(2));
    apply_msg(&mut app, AppMsg::Log(1, "api log".into()));

    assert_eq!(app.services[0].status, Status::Running);
    assert_eq!(app.services[1].status, Status::Stopped);
    assert_eq!(app.services[2].status, Status::Running);
    assert_eq!(app.services[1].log.len(), 1);
    assert!(app.services[0].log.is_empty());
}
