# muxox

Run all your dev services from one terminal.

`muxox` is a CLI-based service orchestrator that makes it easy to start, stop, and monitor multiple processes during development—without juggling a bunch of windows or tabs.

## Features

- Service orchestration
- Live status
- Log viewer
- Simple config
- Start, stop, restart with quick keys

## Installation

```bash
npm install -g muxox
```

Or use with npx:

```bash
npx muxox
```

## Quick Start

1) Create a `muxox.toml` file in your project:

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

2) Run muxox:

```bash
muxox
```

3) Optional: point to a custom config:

```bash
muxox --config path/to/muxox.toml
```

4) Output raw logs instead of TUI:

```bash
muxox --raw
```

## Key Bindings

- ↑ / ↓: Select a service
- Enter: Start/stop the selected service
- r: Restart the selected service
- q: Quit Muxox

## Configuration

Each service supports:

- **name**: Unique identifier (required)
- **cmd**: Command to run (required)
- **cwd**: Working directory (optional, defaults to current dir)
- **log_capacity**: How many log lines to keep in memory (optional, default 2000)

### Tips

- Use `cwd` to run commands from anywhere.
- Pick `log_capacity` large enough to cover your typical debugging session, but not so large that it eats RAM.

## Platform Support

This package automatically downloads and installs the appropriate pre-built binary for your platform:

- Linux (x86_64)
- macOS (Intel and Apple Silicon)
- Windows (x86_64)

## Troubleshooting

- **Not working on platform X**: File an issue with details.
- **Command not found**: Ensure the command in `cmd` is installed and available on your PATH.
- **Permission denied**: Check file permissions or try adjusting the command.
- **Logs look truncated**: Increase `log_capacity` in your `muxox.toml`.
- **Colors look off**: Use a terminal that supports true color.

## Requirements

- Node.js 14.0.0 or higher (for npm installation)
- A terminal with true color support
- All commands referenced in your config must be available in PATH

## License

MIT License

Copyright (c) 2025 Geoff Seemueller

## More Information

For more details, visit the [GitHub repository](https://github.com/geoffsee/muxox).
