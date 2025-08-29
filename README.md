# muxox

Run all your dev services from one terminal.

<p align="center">
  <img src="https://github.com/seemueller-io/muxox/blob/master/muxox.png?raw=true" width="66%" />
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
```bash
cargo install muxox
```
1) Create a muxox.toml file in your project:
```toml
[[service]]
name = "frontend"
cmd = "pnpm client:dev"
cwd = "./"
log_capacity = 5000

[[service]]
name = "backend"
cmd = "pnpm server:dev"
cwd = "./"
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
- log_capacity: How many log lines to keep in memory (optional, default 2000)

Tips:
- Use cwd to run commands from anywhere.
- Pick log_capacity large enough to cover your typical debugging session, but not so large that it eats RAM.

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

## Requirements

- Unix-like OS (Linux, macOS)
- A terminal with true color support
- All commands referenced in your config must be available in PATH

## License

MIT License

Copyright (c) 2025 Geoff Seemueller