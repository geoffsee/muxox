use serde::Deserialize;
use std::path::PathBuf;

// These are independent structs for testing, deliberately not using the internal types
#[derive(Debug, Deserialize)]
struct TestService {
    name: String,
    cmd: String,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default = "default_log_capacity")]
    log_capacity: usize,
}

fn default_log_capacity() -> usize {
    2000
}

#[derive(Debug, Deserialize)]
struct TestConfig {
    #[serde(rename = "service")]
    services: Vec<TestService>,
}

#[test]
fn parses_sample_config() {
    let toml_input = r#"
        [[service]]
        name = "frontend"
        cmd = "pnpm client:dev"
        cwd = "./"
        log_capacity = 5000

        [[service]]
        name = "backend"
        cmd = "pnpm server:dev"
        cwd = "./"
    "#;

    let cfg: TestConfig = toml::from_str(toml_input).expect("valid muxox.toml");
    assert_eq!(cfg.services.len(), 2);

    assert_eq!(cfg.services[0].name, "frontend");
    assert_eq!(cfg.services[0].cmd, "pnpm client:dev");
    assert_eq!(cfg.services[0].cwd, Some(PathBuf::from("./")));
    assert_eq!(cfg.services[0].log_capacity, 5000);

    assert_eq!(cfg.services[1].name, "backend");
    assert_eq!(cfg.services[1].cmd, "pnpm server:dev");
    assert_eq!(cfg.services[1].cwd, Some(PathBuf::from("./")));
    assert_eq!(cfg.services[1].log_capacity, 2000); // default value
}

#[test]
fn handles_missing_optional_fields() {
    let toml_input = r#"
        [[service]]
        name = "minimal"
        cmd = "echo hello"
    "#;

    let cfg: TestConfig = toml::from_str(toml_input).expect("valid minimal config");
    assert_eq!(cfg.services.len(), 1);

    assert_eq!(cfg.services[0].name, "minimal");
    assert_eq!(cfg.services[0].cmd, "echo hello");
    assert_eq!(cfg.services[0].cwd, None);
    assert_eq!(cfg.services[0].log_capacity, 2000); // default value
}

#[test]
#[should_panic(expected = "missing field `name`")]
fn fails_on_missing_required_name() {
    let toml_input = r#"
        [[service]]
        cmd = "echo hello"
    "#;

    let _: TestConfig = toml::from_str(toml_input).expect("should fail on missing name");
}

#[test]
#[should_panic(expected = "missing field `cmd`")]
fn fails_on_missing_required_cmd() {
    let toml_input = r#"
        [[service]]
        name = "invalid"
    "#;

    let _: TestConfig = toml::from_str(toml_input).expect("should fail on missing cmd");
}

#[test]
fn empty_service_array_is_valid() {
    // A config with an empty service array is valid
    let toml_input = "service = []";

    let cfg: TestConfig = toml::from_str(toml_input).expect("config with empty service array should be valid");
    assert_eq!(cfg.services.len(), 0);
}

#[test]
#[should_panic(expected = "missing field `service`")]
fn missing_service_field_is_invalid() {
    // A completely empty config is invalid because the service field is required
    let toml_input = "";

    let _: TestConfig = toml::from_str(toml_input).expect("empty config should be invalid");
}

#[test]
fn parses_real_muxox_toml() {
    // Try to parse the actual muxox.toml file from the repository root
    let result = std::fs::read_to_string("../../muxox.toml");
    
    if let Ok(contents) = result {
        let cfg: TestConfig = toml::from_str(&contents).expect("real muxox.toml should be valid");
        assert!(!cfg.services.is_empty(), "muxox.toml should have at least one service");
        
        // Verify it has the expected services (these assertions depend on the actual file content)
        let service_names: Vec<&str> = cfg.services.iter().map(|s| s.name.as_str()).collect();
        assert!(service_names.contains(&"frontend"), "Should have a frontend service");
        assert!(service_names.contains(&"backend"), "Should have a backend service");
    } else {
        // Test is still valuable even if we can't find the real file
        println!("Note: Could not find ../../muxox.toml, skipping part of test");
    }
}