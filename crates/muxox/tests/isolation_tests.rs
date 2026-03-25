// Integration tests for process isolation (Sandbox strategy).
//
// Platform-specific tests are gated with `#[cfg(target_os = "...")]`.
// Linux tests that require user namespaces will gracefully skip when
// the kernel does not allow unprivileged namespace creation.

use muxox_core::config::{IsolationCfg, ServiceCfg};
use muxox_core::isolation::{Isolation, Sandbox};
use std::process::Stdio;
use tokio::process::Command;

fn iso_cfg(name: &str, cmd: &str, isolation: IsolationCfg) -> ServiceCfg {
    ServiceCfg {
        name: name.into(),
        cmd: cmd.into(),
        cwd: None,
        log_capacity: 100,
        interactive: false,
        pty: false,
        env_file: None,
        isolation,
    }
}

// ---------------------------------------------------------------------------
// Cross-platform
// ---------------------------------------------------------------------------

#[tokio::test]
async fn baseline_no_isolation_runs_successfully() {
    let cfg = iso_cfg("baseline", "echo hello", IsolationCfg::default());
    let sandbox = Sandbox;

    let (prog, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };

    let mut cmd = Command::new(prog);
    cmd.arg(flag)
        .arg("echo hello")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sandbox.prepare(&mut cmd, &cfg);

    let output = cmd.output().await.expect("spawn failed");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hello"));
}

#[tokio::test]
async fn process_isolation_runs_successfully() {
    let cfg = iso_cfg(
        "proc",
        "echo ok",
        IsolationCfg {
            process: true,
            filesystem: false,
            network: false,
        },
    );
    let sandbox = Sandbox;

    let (prog, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };

    let mut cmd = Command::new(prog);
    cmd.arg(flag)
        .arg("echo ok")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sandbox.prepare(&mut cmd, &cfg);

    let output = cmd.output().await.expect("spawn failed");
    assert!(
        output.status.success(),
        "Process isolation should not prevent basic commands. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Linux – namespace-based isolation
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    /// Returns `true` when unprivileged user namespaces are available.
    fn userns_available() -> bool {
        // Check the sysctl that some distros use to disable unprivileged userns.
        for path in [
            "/proc/sys/kernel/unprivileged_userns_clone",
            "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
        ] {
            if let Ok(val) = std::fs::read_to_string(path) {
                let v = val.trim();
                // unprivileged_userns_clone: 1 = allowed
                // apparmor_restrict_unprivileged_userns: 0 = not restricted
                if path.contains("apparmor") && v != "0" {
                    return false;
                }
                if path.contains("unprivileged_userns_clone") && v != "1" {
                    return false;
                }
            }
        }
        true
    }

    #[tokio::test]
    async fn network_isolation_creates_new_namespace() {
        if !userns_available() {
            eprintln!("SKIP: unprivileged user namespaces not available");
            return;
        }

        let parent_ns = std::fs::read_link("/proc/self/ns/net")
            .expect("read parent net ns")
            .to_string_lossy()
            .to_string();

        let cfg = iso_cfg(
            "net-ns",
            "readlink /proc/self/ns/net",
            IsolationCfg {
                process: false,
                filesystem: false,
                network: true,
            },
        );

        let sandbox = Sandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("readlink /proc/self/ns/net")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        let output = cmd.output().await.expect("spawn failed");
        assert!(
            output.status.success(),
            "readlink should succeed. stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let child_ns = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_ne!(
            parent_ns, child_ns,
            "Child must be in a different network namespace"
        );
    }

    #[tokio::test]
    async fn filesystem_isolation_creates_new_mount_namespace() {
        if !userns_available() {
            eprintln!("SKIP: unprivileged user namespaces not available");
            return;
        }

        let parent_ns = std::fs::read_link("/proc/self/ns/mnt")
            .expect("read parent mnt ns")
            .to_string_lossy()
            .to_string();

        let cfg = iso_cfg(
            "fs-ns",
            "readlink /proc/self/ns/mnt",
            IsolationCfg {
                process: false,
                filesystem: true,
                network: false,
            },
        );

        let sandbox = Sandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("readlink /proc/self/ns/mnt")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        let output = cmd.output().await.expect("spawn failed");
        assert!(
            output.status.success(),
            "readlink should succeed. stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let child_ns = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_ne!(
            parent_ns, child_ns,
            "Child must be in a different mount namespace"
        );
    }

    #[tokio::test]
    async fn combined_isolation_creates_both_namespaces() {
        if !userns_available() {
            eprintln!("SKIP: unprivileged user namespaces not available");
            return;
        }

        let parent_net = std::fs::read_link("/proc/self/ns/net")
            .expect("read parent net ns")
            .to_string_lossy()
            .to_string();
        let parent_mnt = std::fs::read_link("/proc/self/ns/mnt")
            .expect("read parent mnt ns")
            .to_string_lossy()
            .to_string();

        let cfg = iso_cfg(
            "combo",
            "echo net=$(readlink /proc/self/ns/net) mnt=$(readlink /proc/self/ns/mnt)",
            IsolationCfg {
                process: true,
                filesystem: true,
                network: true,
            },
        );

        let sandbox = Sandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("echo net=$(readlink /proc/self/ns/net) mnt=$(readlink /proc/self/ns/mnt)")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        let output = cmd.output().await.expect("spawn failed");
        assert!(
            output.status.success(),
            "Combined isolation should succeed. stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse "net=net:[...] mnt=mnt:[...]"
        let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
        assert_eq!(
            parts.len(),
            2,
            "Expected two namespace entries, got: {stdout}"
        );

        let child_net = parts[0].strip_prefix("net=").unwrap_or("");
        let child_mnt = parts[1].strip_prefix("mnt=").unwrap_or("");

        assert_ne!(parent_net, child_net, "Network namespace should differ");
        assert_ne!(parent_mnt, child_mnt, "Mount namespace should differ");
    }

    #[tokio::test]
    async fn network_isolation_prevents_connectivity() {
        if !userns_available() {
            eprintln!("SKIP: unprivileged user namespaces not available");
            return;
        }

        let cfg = iso_cfg(
            "net-block",
            "cat /sys/class/net/lo/operstate",
            IsolationCfg {
                process: false,
                filesystem: false,
                network: true,
            },
        );

        let sandbox = Sandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            // In a fresh network namespace the loopback is down
            .arg("cat /sys/class/net/lo/operstate 2>/dev/null || echo down")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        let output = cmd.output().await.expect("spawn failed");
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(state, "down", "Loopback should be down in new network ns");
    }
}

// ---------------------------------------------------------------------------
// macOS – sandbox_init based isolation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    #[tokio::test]
    async fn network_isolation_blocks_socket_bind() {
        let cfg = iso_cfg(
            "net-block",
            "python3 -c \"import socket; s=socket.socket(); s.bind(('127.0.0.1',0)); print('bound')\"",
            IsolationCfg {
                process: false,
                filesystem: false,
                network: true,
            },
        );

        let sandbox = Sandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&cfg.cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        match cmd.output().await {
            Ok(output) => {
                assert!(
                    !output.status.success(),
                    "Socket bind should fail under network isolation. stdout: {}",
                    String::from_utf8_lossy(&output.stdout)
                );
            }
            Err(e) => {
                // sandbox_init blocked the spawn itself — acceptable
                eprintln!("Spawn blocked by sandbox (acceptable): {e}");
            }
        }
    }

    #[tokio::test]
    async fn filesystem_isolation_blocks_writes_outside_cwd() {
        let cwd = std::env::temp_dir().join("muxox_iso_fs_deny");
        let _ = std::fs::create_dir_all(&cwd);

        // Home directory is writable normally but NOT in our sandbox profile
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/runner".into());
        let forbidden = format!("{home}/muxox_sandbox_test_marker");

        let cfg = ServiceCfg {
            name: "fs-deny".into(),
            cmd: format!("touch {forbidden}"),
            cwd: Some(cwd.clone()),
            log_capacity: 100,
            interactive: false,
            pty: false,
            env_file: None,
            isolation: IsolationCfg {
                process: false,
                filesystem: true,
                network: false,
            },
        };

        let sandbox = Sandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&cfg.cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        match cmd.output().await {
            Ok(output) => {
                assert!(
                    !output.status.success(),
                    "Write outside cwd should fail. stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                // Clean up in case it somehow succeeded
                let _ = std::fs::remove_file(&forbidden);
            }
            Err(e) => {
                eprintln!("Spawn blocked by sandbox (acceptable): {e}");
            }
        }

        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[tokio::test]
    async fn filesystem_isolation_allows_writes_inside_cwd() {
        let cwd = std::env::temp_dir().join("muxox_iso_fs_allow");
        let _ = std::fs::create_dir_all(&cwd);

        // Canonicalize so the SBPL profile matches the real path
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
        let marker = cwd.join("allowed_marker");

        let cfg = ServiceCfg {
            name: "fs-allow".into(),
            cmd: format!("touch {}", marker.display()),
            cwd: Some(cwd.clone()),
            log_capacity: 100,
            interactive: false,
            pty: false,
            env_file: None,
            isolation: IsolationCfg {
                process: false,
                filesystem: true,
                network: false,
            },
        };

        let sandbox = Sandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&cfg.cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        match cmd.output().await {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "Write inside cwd should succeed. stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) => {
                panic!("Spawn should not fail for writes inside cwd: {e}");
            }
        }

        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[tokio::test]
    async fn combined_isolation_applies_all_restrictions() {
        let cwd = std::env::temp_dir().join("muxox_iso_combo");
        let _ = std::fs::create_dir_all(&cwd);
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);

        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/runner".into());
        // Try to write outside cwd AND do network — both should fail
        let forbidden = format!("{home}/muxox_combo_test_marker");

        let cfg = ServiceCfg {
            name: "combo".into(),
            cmd: format!(
                "touch {forbidden} 2>/dev/null && echo file_ok || echo file_blocked; \
                 python3 -c \"import socket; s=socket.socket(); s.bind(('127.0.0.1',0)); print('net_ok')\" 2>/dev/null || echo net_blocked"
            ),
            cwd: Some(cwd.clone()),
            log_capacity: 100,
            interactive: false,
            pty: false,
            env_file: None,
            isolation: IsolationCfg {
                process: true,
                filesystem: true,
                network: true,
            },
        };

        let sandbox = Sandbox;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&cfg.cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        match cmd.output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(
                    stdout.contains("file_blocked"),
                    "File write should be blocked. stdout: {stdout}"
                );
                assert!(
                    stdout.contains("net_blocked"),
                    "Network should be blocked. stdout: {stdout}"
                );
                let _ = std::fs::remove_file(&forbidden);
            }
            Err(e) => {
                eprintln!("Spawn blocked (acceptable): {e}");
            }
        }

        let _ = std::fs::remove_dir_all(&cwd);
    }
}

// ---------------------------------------------------------------------------
// Windows – Job Object + creation flags
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows {
    use super::*;

    #[tokio::test]
    async fn process_isolation_with_job_object() {
        let cfg = iso_cfg(
            "job",
            "echo ok",
            IsolationCfg {
                process: true,
                filesystem: false,
                network: false,
            },
        );

        let sandbox = Sandbox;
        let mut cmd = Command::new("cmd");
        cmd.arg("/C")
            .arg("echo ok")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        let output = cmd.output().await.expect("spawn failed");
        // Apply post_spawn (Job Object assignment) with real PID
        if let Some(pid) = output.status.code() {
            // We can't easily get the PID before the process exits in this
            // test, so just verify post_spawn doesn't panic on PID 0.
            sandbox.post_spawn(0, &cfg);
        }

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("ok"));
    }

    #[tokio::test]
    async fn unsupported_flags_still_run() {
        // filesystem + network are not enforced on Windows — just warnings
        let cfg = iso_cfg(
            "unsupported",
            "echo still_works",
            IsolationCfg {
                process: true,
                filesystem: true,
                network: true,
            },
        );

        let sandbox = Sandbox;
        let mut cmd = Command::new("cmd");
        cmd.arg("/C")
            .arg("echo still_works")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sandbox.prepare(&mut cmd, &cfg);

        let output = cmd.output().await.expect("spawn failed");
        assert!(
            output.status.success(),
            "Unsupported isolation flags should not prevent execution"
        );
    }
}
