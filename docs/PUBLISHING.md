# Publishing Instructions for Muxox

This document outlines the steps to publish the Muxox crate to crates.io and test the published version.

## Prerequisites

1. A crates.io account
2. Cargo installed on your system
3. The Muxox repository cloned locally

## Step 1: Obtain a Crates.io API Token

1. Log in to [crates.io](https://crates.io)
2. Go to your Account Settings (click on your username in the top right, then "Account Settings")
3. Under the "API Tokens" section, create a new token
   - You can create a token with limited scope for added security (e.g., only publish rights)
4. Copy the generated token (it will only be shown once)

## Step 2: Log in to Crates.io via Cargo

```bash
cargo login <your-api-token>
```

This command stores your API token locally so Cargo can authenticate with crates.io.

## Step 3: Publish the Crate

From the repository root:

```bash
cargo publish -p muxox
```

If you want to verify everything one last time before publishing:

```bash
cargo publish -p muxox --dry-run
```

## Step 4: Test the Published Crate

After successfully publishing, test that the crate can be installed and works correctly:

1. Create a new directory outside of your project
   ```bash
   mkdir test-muxox && cd test-muxox
   ```

2. Install the crate
   ```bash
   cargo install muxox
   ```

3. Create a test configuration file
   ```bash
   cat > muxox.toml << 'EOF'
   [[service]]
   name = "test-service"
   cmd = "echo 'Hello from muxox!' && sleep 5"
   
   [[service]]
   name = "another-service"
   cmd = "echo 'Service 2 starting...' && sleep 10"
   EOF
   ```

4. Run muxox
   ```bash
   muxox
   ```

5. Verify that the TUI loads correctly and services can be started

## Troubleshooting

- **Name already taken**: If the name "muxox" is already taken on crates.io, you'll need to choose a different name and update it in the Cargo.toml file.
- **Version conflict**: If you need to republish, increment the version number in Cargo.toml first.
- **Dependency issues**: Ensure all dependencies are compatible with the crates.io ecosystem.

## Future Updates

To publish updates:

1. Make your changes
2. Update the version number in Cargo.toml (following [semantic versioning](https://semver.org/))
3. Run tests
4. Publish using the same command: `cargo publish -p muxox`