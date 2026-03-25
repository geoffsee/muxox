#[cfg(test)]
mod tests {
    use crate::app::{App, AppMsg, ServiceState, Status, apply_msg};
    use crate::config::ServiceCfg;
    use crate::isolation::default_isolation;
    use std::sync::Arc;
    use tokio::sync::{Mutex, mpsc};

    fn test_cfg(name: &str) -> ServiceCfg {
        ServiceCfg {
            name: name.into(),
            cmd: "echo hi".into(),
            cwd: None,
            log_capacity: 100,
            interactive: false,
            pty: false,
            env_file: None,
        }
    }

    fn test_app(names: &[&str]) -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        App {
            services: names
                .iter()
                .map(|n| ServiceState::new(test_cfg(n)))
                .collect(),
            selected: 0,
            log_offset_from_end: 0,
            tx,
            input_mode: false,
            input_buffer: String::new(),
            isolation: default_isolation(),
        }
    }

    // --- apply_msg: Started ---

    #[test]
    fn apply_started_sets_running() {
        let mut app = test_app(&["svc"]);
        assert_eq!(app.services[0].status, Status::Stopped);

        apply_msg(&mut app, AppMsg::Started(0));
        assert_eq!(app.services[0].status, Status::Running);
    }

    // --- apply_msg: Stopped ---

    #[test]
    fn apply_stopped_clears_process_state() {
        let mut app = test_app(&["svc"]);
        app.services[0].status = Status::Running;
        app.services[0].pid = Some(1234);

        apply_msg(&mut app, AppMsg::Stopped(0, 0));
        assert_eq!(app.services[0].status, Status::Stopped);
        assert!(app.services[0].pid.is_none());
        assert!(app.services[0].child.is_none());
        assert!(app.services[0].stdin_tx.is_none());
        assert!(app.services[0].stdin_writer.is_none());
    }

    // --- apply_msg: Log ---

    #[test]
    fn apply_log_appends_line() {
        let mut app = test_app(&["svc"]);
        apply_msg(&mut app, AppMsg::Log(0, "hello".into()));
        apply_msg(&mut app, AppMsg::Log(0, "world".into()));
        assert_eq!(app.services[0].log.len(), 2);
        assert_eq!(app.services[0].log[0], "hello");
        assert_eq!(app.services[0].log[1], "world");
    }

    #[test]
    fn apply_log_respects_capacity() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App {
            services: vec![ServiceState::new(ServiceCfg {
                name: "svc".into(),
                cmd: "x".into(),
                cwd: None,
                log_capacity: 3,
                interactive: false,
                pty: false,
                env_file: None,
            })],
            selected: 0,
            log_offset_from_end: 0,
            tx,
            input_mode: false,
            input_buffer: String::new(),
            isolation: default_isolation(),
        };

        for i in 0..5 {
            apply_msg(&mut app, AppMsg::Log(0, format!("line {i}")));
        }
        assert_eq!(app.services[0].log.len(), 3);
        assert_eq!(app.services[0].log[0], "line 2");
        assert_eq!(app.services[0].log[2], "line 4");
    }

    // --- apply_msg: ChildSpawned ---

    #[test]
    fn apply_child_spawned_stores_pid() {
        let mut app = test_app(&["svc"]);
        apply_msg(&mut app, AppMsg::ChildSpawned(0, 9999));
        assert_eq!(app.services[0].pid, Some(9999));
    }

    // --- apply_msg: StdinReady ---

    #[test]
    fn apply_stdin_ready_stores_sender() {
        let mut app = test_app(&["svc"]);
        let (sender, _) = mpsc::channel(1);
        apply_msg(&mut app, AppMsg::StdinReady(0, sender));
        assert!(app.services[0].stdin_tx.is_some());
    }

    // --- apply_msg: StdinWriterReady ---

    #[tokio::test]
    async fn apply_stdin_writer_ready_stores_writer() {
        let mut app = test_app(&["svc"]);
        // Use a dummy ChildStdin via spawning a real process
        let mut child = tokio::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let writer = Arc::new(Mutex::new(stdin));

        apply_msg(&mut app, AppMsg::StdinWriterReady(0, writer));
        assert!(app.services[0].stdin_writer.is_some());

        child.kill().await.ok();
    }

    // --- apply_msg: AbortedAll ---

    #[test]
    fn apply_aborted_all_is_noop() {
        let mut app = test_app(&["svc"]);
        app.services[0].status = Status::Running;
        apply_msg(&mut app, AppMsg::AbortedAll);
        // AbortedAll does not change service state — handled by the main loop
        assert_eq!(app.services[0].status, Status::Running);
    }

    // --- Status::as_str ---

    #[test]
    fn status_display_strings() {
        assert_eq!(Status::Stopped.as_str(), "Stopped");
        assert_eq!(Status::Starting.as_str(), "Starting");
        assert_eq!(Status::Running.as_str(), "Running");
        assert_eq!(Status::Stopping.as_str(), "Stopping");
    }

    // --- State machine transitions ---

    #[test]
    fn full_lifecycle_stopped_to_running_to_stopped() {
        let mut app = test_app(&["svc"]);
        assert_eq!(app.services[0].status, Status::Stopped);

        apply_msg(&mut app, AppMsg::Started(0));
        assert_eq!(app.services[0].status, Status::Running);

        apply_msg(&mut app, AppMsg::ChildSpawned(0, 42));
        assert_eq!(app.services[0].pid, Some(42));

        apply_msg(&mut app, AppMsg::Stopped(0, 0));
        assert_eq!(app.services[0].status, Status::Stopped);
        assert_eq!(app.services[0].pid, None);
    }

    // --- Multiple services ---

    #[test]
    fn messages_target_correct_service() {
        let mut app = test_app(&["alpha", "bravo", "charlie"]);

        apply_msg(&mut app, AppMsg::Started(1));
        apply_msg(&mut app, AppMsg::Log(2, "charlie log".into()));

        assert_eq!(app.services[0].status, Status::Stopped);
        assert_eq!(app.services[1].status, Status::Running);
        assert_eq!(app.services[2].status, Status::Stopped);
        assert_eq!(app.services[2].log.len(), 1);
        assert!(app.services[0].log.is_empty());
    }
}
