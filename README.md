# muxox

Run all your dev services from one terminal.

<p align="center">
  <img src="https://github.com/geoffsee/muxox/blob/master/muxox.png?raw=true" width="66%" />
</p>

`muxox` is a cli-based service orchestrator that makes it easy to start, stop, and monitor multiple processes during development—without juggling a bunch of windows or tabs.

- Service orchestration
- Live status
- Log viewer
- Simple config
- Start, stop, restart with quick keys


## Key bindings

- ↑ / ↓: Select a service
- Enter: Start/stop the selected service
- r: Restart the selected service
- q: Quit Muxox


## Quick start
### Install

#### Option 1: Pre-built binaries (recommended)
Download the latest release for your platform from the [releases page](https://github.com/geoffsee/muxox/releases):

- **Linux (x86_64)**: `muxox-x86_64-unknown-linux-gnu.tar.gz`
- **macOS (Intel)**: `muxox-x86_64-apple-darwin.tar.gz`
- **macOS (Apple Silicon)**: `muxox-aarch64-apple-darwin.tar.gz`
- **Windows**: `muxox-x86_64-pc-windows-msvc.exe.zip`

Extract the binary and place it in your PATH.

#### Option 2: Build from source
```bash
cargo install muxox
```
1) Create a muxox.toml file in your project. The fastest way is to scaffold a fully-commented example:
```bash
muxox init
```
Or write one by hand:
```toml
[[service]]
name = "test-stdin"
cmd = "./test-stdin.sh"
cwd = "./example"
log_capacity = 250
interactive = true

[[service]]
name = "example-service-1"
cmd = "bun ./index.ts"
cwd = "example/packages/example-service-1"
log_capacity = 5000
```
2) Run Muxox:
```bash
muxox
```
3) Optional: point to a custom config:
```bash
muxox --config path/to/muxox.toml
```
4) Output raw logs instead of TUI
```bash
muxox --raw
```
5)
## Configuration

Each service supports:

- name: Unique identifier (required)
- cmd: Command to run (required)
- cwd: Working directory (optional, defaults to current dir)
- interactive: Whether the service requires stdin (optional, default: false)
- pty: Whether to allocate a PTY for the service (Unix only, optional, default: false)
- log_capacity: How many log lines to keep in memory (optional, default 2000)
- env_file: Path to a `.env` file whose KEY=VALUE pairs are injected into the service's environment (optional)
- isolation: Per-service sandboxing settings (optional, see below)

Tips:
- Use cwd to run commands from anywhere.
- Pick log_capacity large enough to cover your typical debugging session, but not so large that it eats RAM.

### Isolation

Opt-in per-service sandboxing. All flags default to `false`.

```toml
[[service]]
name = "sandboxed-worker"
cmd = "node worker.js"
cwd = "./packages/worker"
isolation = { process = true, filesystem = true, network = true }
```

| Flag | Description | macOS | Linux | Windows |
|------|-------------|-------|-------|---------|
| `process` | Session-level process isolation | `setsid` | `setsid` | Job Object (`KILL_ON_JOB_CLOSE`) |
| `filesystem` | Restrict writes to service cwd | `sandbox_init` (SBPL) | User + mount namespace (`unshare`) | Not yet supported |
| `network` | Deny outbound network access | `sandbox_init` (SBPL) | User + network namespace (`unshare`) | Not yet supported |

Notes:
- Linux namespace isolation requires unprivileged user namespaces to be enabled (`sysctl kernel.unprivileged_userns_clone=1`).
- macOS filesystem isolation allows writes to the service cwd, `/tmp`, and `/dev`.
- When an isolation flag is requested but not supported on the current platform, the service will either fail to start (Unix) or log a warning and continue (Windows).

## Embedded MCP server

Muxox can expose its service registry and captured logs to MCP-compatible
agents via an embedded JSON-RPC 2.0 server (Model Context Protocol,
[Streamable HTTP transport](https://modelcontextprotocol.io)).  This makes it
trivial for AI coding agents to inspect what your services are doing without
scraping the web UI.

Enable it from `muxox.toml`:

```toml
[mcp]
enabled = true        # default: false
port    = 0           # default: 0 (pick a random available port)
bind    = "127.0.0.1" # default: localhost only
```

When muxox starts (in web mode or with `--raw`), it prints the bound URL:

```
MCP server listening at http://127.0.0.1:54321/mcp
```

Point your MCP client at that URL.  The server advertises two tools:

| Tool | Description |
|------|-------------|
| `list_services` | Returns every service muxox is managing, with `status`, `pid`, and current `log_lines`. |
| `get_logs`      | Returns recent log lines for one service.  Arguments: `service` (required), `tail` (default 200, max 5000), `grep` (optional substring filter). |

The MCP server is **not** started in `--tui` mode (the TUI uses single-thread
state that can't be shared with the HTTP server).  Use the default web mode or
`--raw` if you need MCP access.

## Troubleshooting
- Not working on platform X
  - File an issue with details.
- Command not found
  - Ensure the command in cmd is installed and available on your PATH in the shell that launches Muxox.
- Permission denied
  - Check file permissions or try adjusting the command (e.g., scripts may require execute permission).
- Logs look truncated
  - Increase log_capacity in your muxox.toml to keep more history.
- Colors look off
  - Use a terminal that supports true color and make sure it’s enabled.

## FAQ

- How is this different from a terminal multiplexer like tmux?
  - Muxox focuses on orchestrating processes and their logs with simple controls, not on managing panes or sessions.

- Do I need containers or a specific runtime?
  - No. Muxox runs your local commands directly.

- Can I use it for production?
  - Muxox is designed for development workflows. For production, consider a proper process supervisor or orchestrator.

## Development Scripts and Makefile

You can use either the shell scripts in `scripts/` or the provided `Makefile` to manage common development tasks.

### Using Make

- `make build`: Build the `muxox` binary.
- `make test`: Run all tests.
- `make lint`: Run formatting and linting checks.
- `make run`: Run `muxox` using the example configuration file.
- `make fmt`: Run `cargo fmt --all`.
- `make clean`: Run `cargo clean`.

### Using Scripts Directly

The `scripts/` directory contains:

- `./scripts/build.sh`: Build the `muxox` binary.
- `./scripts/test.sh`: Run all tests.
- `./scripts/lint.sh`: Run formatting and linting checks.
- `./scripts/run-example.sh`: Run `muxox` using the example configuration file.

## Requirements

- Linux, macOS, or Windows
- A terminal with true color support
- All commands referenced in your config must be available in PATH

## License

MIT License

Copyright (c) 2025 Geoff Seemueller