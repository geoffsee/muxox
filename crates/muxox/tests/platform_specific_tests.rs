// Platform specific tests
// These tests are conditionally compiled based on the target platform

#[cfg(unix)]
mod unix_tests {
    // Test Unix-specific functionality
    // Since we can't directly access internal functions from integration tests,
    // we test platform-specific behavior instead

    #[test]
    fn test_unix_shell_detection() {
        // This is a simple test to verify that we're on a Unix platform
        // The actual shell_program function is private in main.rs
        assert!(cfg!(unix), "This test should only run on Unix platforms");

        // We can test that standard Unix directories exist
        assert!(
            std::path::Path::new("/bin/sh").exists()
                || std::path::Path::new("/usr/bin/sh").exists(),
            "Expected to find a shell at /bin/sh or /usr/bin/sh on Unix"
        );
    }

    #[test]
    fn test_unix_signals() {
        // We can test that nix/signal functionality works as expected
        use nix::sys::signal::{SigSet, Signal};

        // Create a signal set and verify basic operations
        let mut set = SigSet::empty();
        set.add(Signal::SIGTERM);
        assert!(set.contains(Signal::SIGTERM));
        assert!(!set.contains(Signal::SIGINT));
    }
}

#[cfg(windows)]
mod windows_tests {
    #[test]
    fn test_windows_platform() {
        assert!(
            cfg!(windows),
            "This test should only run on Windows platforms"
        );
    }
}

// Cross-platform tests that should work on any platform
#[test]
fn test_process_creation() {
    use std::process::Command;

    // A simple command that should work on any platform
    let output = if cfg!(windows) {
        Command::new("cmd").args(["/C", "echo hello"]).output()
    } else {
        Command::new("sh").args(["-c", "echo hello"]).output()
    };

    // Verify we can create processes
    assert!(output.is_ok(), "Should be able to create a basic process");

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // On Windows, echo adds CRLF, on Unix just LF
        let expected = if cfg!(windows) {
            "hello\r\n"
        } else {
            "hello\n"
        };
        assert!(stdout.contains("hello"), "Expected 'hello' in output");
        // Optionally, do a more precise check with the expected output format
        assert!(
            stdout.trim() == "hello" || stdout == expected,
            "Output should be exactly 'hello' with optional newline formatting"
        );
    }
}

// Test for the environment-dependent features
#[test]
fn test_environment_detection() {
    if cfg!(unix) {
        // Unix environment checks
        assert!(
            std::path::Path::new("/").exists(),
            "Root directory should exist on Unix"
        );
    } else if cfg!(windows) {
        // Windows environment checks
        assert!(
            std::path::Path::new("C:\\").exists() || std::path::Path::new("D:\\").exists(),
            "Expected to find C: or D: drive on Windows"
        );
    }
}
