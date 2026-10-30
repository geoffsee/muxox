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

Install the Muxox agent skill globally for Codex, Cursor, Claude, Vibe, Grok,
Agy, Hermes, OpenCode, and Cline:

```bash
muxox install-skill
```

Use `--client codex,cursor` to select clients, `--dry-run` to preview changes,
or `--force` to replace an existing skill. Existing files that differ are left
alone (printed as Skipped) unless you pass `--force`. Hermes installs under
`~/skills/muxox`; other clients use their usual `~/.…/skills/muxox` paths.

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

## Updates

Muxox checks for a newer stable version in the background on each ordinary
launch. When one is available, it shows a web banner, a TUI notice, or one line
on stderr in raw mode. Checks have a short deadline, stay quiet offline, and
never delay service startup or install anything automatically.

```bash
muxox update
```

The update command runs before loading configuration or starting services:

- **Cargo:** checks crates.io and runs Cargo with the tracked installation root,
  features, profile, and target. Git, path, custom-registry, and legacy untracked
  Cargo installations must use their original Cargo install command.
- **npm, pnpm, Yarn, Bun:** checks the manager's configured registry and runs that
  same manager. Global installs retain their installation scope. For a local
  install, run the command from the project or workspace that declares Muxox;
  its manifest and lockfile may be upgraded to the latest stable version across
  the previous version range. The native process exits before the manager runs.
- **Downloaded executable:** use `muxox update --standalone` to explicitly allow
  replacement of an untracked binary. Muxox checks GitHub Releases, verifies the
  asset's SHA-256 digest and executable version, and replaces the running binary.
  This override is rejected for recognized package-manager installations.

Checks compare the running binary's version and never offer a downgrade.
Missing installation receipts, ambiguous owners, and temporary npx/dlx/bunx
installs require the original package-manager command; Muxox explains this when
an explicit update is requested. Package-manager output and failures are shown.

Older JavaScript wrappers must first be upgraded or reinstalled with their
original package manager to receive the new launcher and installation receipt.
Publishing a GitHub release alone does not make it available to Cargo or
JavaScript package-manager installations.

## Configuration

Top-level options:

- name: Display name for the web UI header and page title (optional; default `muxox`)
- muxox_version: Semver range the running muxox binary must satisfy (optional)
- hooks: Global lifecycle hooks run once at boot / shutdown (optional; see below)
- mcp: Embedded MCP server settings (optional; see below)

Each service supports:

- name: Unique identifier (required)
- cmd: Command to run (required)
- cwd: Working directory (optional, defaults to current dir). Relative paths resolve from the config file's directory.
- interactive: Whether the service requires stdin (optional, default: false)
- pty: Whether to allocate a PTY for the service (Unix only, optional, default: false)
- log_capacity: How many log lines to keep in memory (optional, default 2000)
- env_file: Path to a `.env` file whose KEY=VALUE pairs are injected into the service's environment (optional)
- isolation: Per-service sandboxing settings (optional, see below)
- hooks: Per-service lifecycle hooks (optional, see below)
- monitor: Opt-in CPU, memory, and GPU sampling (optional, see below)

Tips:
- Use cwd to run commands from anywhere.
- Pick log_capacity large enough to cover your typical debugging session, but not so large that it eats RAM.

### Lifecycle hooks

Optional shell commands around start/stop. Declare a top-level `[hooks]` table for once-per-boot / once-per-shutdown commands, and/or `hooks` on each `[[service]]`.

| Phase | Service | Global |
|-------|---------|--------|
| `before_start` | Before spawning the service | Once before any service starts |
| `after_start` | After successful spawn | Once after every boot start has settled |
| `before_stop` | Before explicit stop/kill | Once before shutdown kills |
| `after_stop` | After the process exits | Once after shutdown kills |

Non-zero hook exits log a warning and continue. Service hooks inherit that service's `cwd` and `env_file` and write into that service's log pane. Global hooks run with the config file's directory as cwd, also appear in every service pane (prefixed with `[hooks]`), and on muxox's stderr. `before_stop` does not run on natural process exit; `after_stop` always does when the process exits. Global hooks do not re-run on individual service restart.

```toml
[hooks]
before_start = "./scripts/boot-pre.sh"
after_stop   = "./scripts/shutdown-post.sh"

[[service]]
name = "api"
cmd = "bun run dev"
cwd = "./packages/api"
hooks = { before_start = "./scripts/api-pre.sh", after_stop = "./scripts/api-cleanup.sh" }
```

### Resource monitoring

Off until a service opts in. Inside the table, `cpu` and `memory` default to `true` and `gpu` defaults to `false`. The header and `inspect` omit a reading that is off. GPU stays out of the scan unless some running service sets `gpu = true`.

```toml
[[service]]
name = "api"
cmd = "bun run dev"
monitor = { cpu = true, memory = true, gpu = false }
```

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

Muxox can expose its service registry, captured logs, and start/stop controls
to MCP-compatible agents via an embedded JSON-RPC 2.0 server (Model Context
Protocol, [Streamable HTTP transport](https://modelcontextprotocol.io)).  This
makes it trivial for AI coding agents to inspect and control your services
without scraping the web UI.

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

Point your MCP client at that URL.  The server advertises five tools:

| Tool | Description |
|------|-------------|
| `list_services` | Returns every service muxox is managing, with `status`, `pid`, and current `log_lines`. |
| `inspect`       | Returns one service's status, pid, command, working directory, and latest resource sample.  Argument: `service` (required).  `resources` is null when monitoring is off, the service is not running, or the first sample is not ready yet.  Each of `cpu_percent`, `memory_bytes`, `gpu_percent`, and `gpu_memory_bytes` is null when that reading is disabled or unavailable. |
| `get_logs`      | Returns recent log lines for one service.  Arguments: `service` (required), `tail` (default 200, max 5000), `grep` (optional substring filter). |
| `start_service` | Starts one service by name.  Argument: `service` (required).  No-op when that service is already starting or running. |
| `stop_service`  | Stops one service by name.  Argument: `service` (required).  No-op unless that service is starting or running. |

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
