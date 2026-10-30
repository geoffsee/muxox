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
            isolation: Default::default(),
            hooks: Default::default(),
            monitor: None,
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
            update_notice: None,
            hooks: Default::default(),
            config_dir: None,
            settle_tx: None,
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
                isolation: Default::default(),
                hooks: Default::default(),
                monitor: None,
            })],
            selected: 0,
            log_offset_from_end: 0,
            tx,
            input_mode: false,
            input_buffer: String::new(),
            isolation: default_isolation(),
            update_notice: None,
            hooks: Default::default(),
            config_dir: None,
            settle_tx: None,
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

    #[test]
    fn apply_spawn_settled_is_noop() {
        let mut app = test_app(&["svc"]);
        app.services[0].status = Status::Starting;
        apply_msg(&mut app, AppMsg::SpawnSettled(0));
        assert_eq!(app.services[0].status, Status::Starting);
    }

    // --- Lifecycle hooks ---

    #[tokio::test]
    async fn start_service_runs_before_start_hook() {
        use crate::app::start_service;
        use crate::config::HooksCfg;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut cfg = test_cfg("hooked");
        cfg.cmd = "echo main-cmd".into();
        cfg.hooks = HooksCfg {
            before_start: Some("echo before-ran".into()),
            after_start: Some("echo after-start-ran".into()),
            before_stop: None,
            after_stop: Some("echo after-stop-ran".into()),
        };
        let mut app = App {
            services: vec![ServiceState::new(cfg)],
            selected: 0,
            log_offset_from_end: 0,
            tx,
            input_mode: false,
            input_buffer: String::new(),
            isolation: default_isolation(),
            update_notice: None,
            hooks: Default::default(),
            config_dir: None,
            settle_tx: None,
        };

        start_service(0, &mut app);

        let mut saw_before = false;
        let mut saw_after_start = false;
        let mut saw_after_stop = false;
        let mut saw_settled = false;
        let mut saw_stopped = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(AppMsg::Log(_, line))) => {
                    if line.contains("before-ran") {
                        saw_before = true;
                    }
                    if line.contains("after-start-ran") {
                        saw_after_start = true;
                    }
                    if line.contains("after-stop-ran") {
                        saw_after_stop = true;
                    }
                }
                Ok(Some(AppMsg::SpawnSettled(0))) => saw_settled = true,
                Ok(Some(AppMsg::Stopped(0, _))) => {
                    saw_stopped = true;
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }

        assert!(saw_before, "before_start hook output missing");
        assert!(saw_after_start, "after_start hook output missing");
        assert!(saw_settled, "SpawnSettled missing");
        assert!(saw_after_stop, "after_stop hook output missing");
        assert!(saw_stopped, "Stopped missing");
    }

    #[tokio::test]
    async fn start_all_runs_global_hooks_around_services() {
        use crate::app::start_all;
        use crate::config::HooksCfg;

        let marker = std::env::temp_dir().join(format!("muxox_hook_order_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut cfg = test_cfg("svc");
        cfg.cmd = "echo service".into();
        let mut app = App {
            services: vec![ServiceState::new(cfg)],
            selected: 0,
            log_offset_from_end: 0,
            tx,
            input_mode: false,
            input_buffer: String::new(),
            isolation: default_isolation(),
            update_notice: None,
            hooks: HooksCfg {
                before_start: Some(format!("echo boot-pre >> {}", marker.display())),
                after_start: Some(format!("echo boot-post >> {}", marker.display())),
                before_stop: None,
                after_stop: None,
            },
            config_dir: None,
            settle_tx: None,
        };

        start_all(&mut app).await;

        let logs: Vec<String> = app.services[0].log.iter().cloned().collect();
        assert!(
            logs.iter()
                .any(|l| l.contains("[hooks]") && l.contains("boot-pre")),
            "global before_start should appear in service logs: {logs:?}"
        );
        assert!(
            logs.iter()
                .any(|l| l.contains("[hooks]") && l.contains("boot-post")),
            "global after_start should appear in service logs: {logs:?}"
        );

        // Drain until settled service stops so the test doesn't leak a task.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(AppMsg::Stopped(_, _))) => break,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }

        let contents = std::fs::read_to_string(&marker).expect("marker written");
        let pre = contents.find("boot-pre").expect("boot-pre present");
        let post = contents.find("boot-post").expect("boot-post present");
        assert!(
            pre < post,
            "global before_start should run before after_start"
        );
        let _ = std::fs::remove_file(&marker);
    }
}
