use std::process::Command;

#[test]
fn update_help_and_ownership_errors_need_no_config_or_logger() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir
        .path()
        .join(if cfg!(windows) { "muxox.exe" } else { "muxox" });
    std::fs::copy(env!("CARGO_BIN_EXE_muxox"), &binary).unwrap();
    let help = Command::new(&binary)
        .args(["update", "--help"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--standalone"));
    let result = Command::new(&binary)
        .arg("update")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Installation ownership is unknown"));
    assert!(!dir.path().join("muxox-debug.log").exists());
}
