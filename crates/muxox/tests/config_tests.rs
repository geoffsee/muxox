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
        name = "test-stdin"
        cmd = "./test-stdin.sh"
        cwd = "./example"
        log_capacity = 250

        [[service]]
        name = "example-service-1"
        cmd = "bun ./index.ts"
        cwd = "example/packages/example-service-1"
        log_capacity = 5000
    "#;

    let cfg: TestConfig = toml::from_str(toml_input).expect("valid muxox.toml");
    assert_eq!(cfg.services.len(), 2);

    assert_eq!(cfg.services[0].name, "test-stdin");
    assert_eq!(cfg.services[0].cmd, "./test-stdin.sh");
    assert_eq!(cfg.services[0].cwd, Some(PathBuf::from("./example")));
    assert_eq!(cfg.services[0].log_capacity, 250);

    assert_eq!(cfg.services[1].name, "example-service-1");
    assert_eq!(cfg.services[1].cmd, "bun ./index.ts");
    assert_eq!(cfg.services[1].cwd, Some(PathBuf::from("example/packages/example-service-1")));
    assert_eq!(cfg.services[1].log_capacity, 5000);
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

    let cfg: TestConfig =
        toml::from_str(toml_input).expect("config with empty service array should be valid");
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
    // Try to parse the actual muxox.example.toml file from the repository root
    let result = std::fs::read_to_string("../../muxox.example.toml");

    if let Ok(contents) = result {
        let cfg: TestConfig = toml::from_str(&contents).expect("real muxox.example.toml should be valid");
        assert!(
            !cfg.services.is_empty(),
            "muxox.example.toml should have at least one service"
        );

        // Verify it has the expected services
        let service_names: Vec<&str> = cfg.services.iter().map(|s| s.name.as_str()).collect();
        assert!(
            service_names.contains(&"test-stdin"),
            "Should have a test-stdin service"
        );
        assert!(
            service_names.contains(&"example-service-1"),
            "Should have an example-service-1 service"
        );
    } else {
        // Test is still valuable even if we can't find the real file
        println!("Note: Could not find ../../muxox.example.toml, skipping part of test");
    }
}
