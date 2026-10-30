// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller

//! Embedded files installed by `muxox install-skill`.

pub const SKILL_MD: &str = r###"---
name: muxox
description: >-
  Inspect and control Muxox-managed local development services using Muxox MCP
  tools list_services, inspect, get_logs, start_service, and stop_service.
  Use for service status, PIDs, CPU, memory, and GPU, logs, startup failures, crash
  loops, port conflicts, and unhealthy workers.
---

# Muxox

Use Muxox's embedded MCP server to inspect managed services, read captured
logs, and start or stop a service by name.

## Workflow

1. Confirm Muxox is running in web or `--raw` mode, not `--tui`.
2. Confirm `[mcp] enabled = true` in `muxox.toml` and use its printed `/mcp` URL.
3. Call `list_services` first.
4. Call `inspect` with the exact service name for status, pid, command, and
   the latest resource sample. `resources` is null when that service has no
   `monitor` table, until a sample exists about a second after the process is
   running, and when the service is stopped. `cpu_percent`, `memory_bytes`,
   `gpu_percent`, and `gpu_memory_bytes` are null when that reading is
   disabled or unavailable. macOS reports GPU usage only.
5. Call `get_logs` for one implicated service at a time, using the smallest
   useful `tail` and a case-sensitive literal `grep` when appropriate.
6. Call `start_service` or `stop_service` with the exact service name when the
   user asks to start or stop that service. Then call `list_services` again
   and verify with focused logs.

Read `references/mcp-tools.md` for schemas and `references/setup.md` for setup.

Never reproduce secrets, tokens, cookies, passwords, private URLs, or PII from
logs. Redact sensitive values and report only sanitized excerpts.
"###;

pub const MCP_TOOLS_MD: &str = r###"# Muxox MCP tools

Muxox exposes a Streamable HTTP MCP endpoint at `http://{bind}:{port}/mcp`.

## `list_services`

Call with no arguments. It returns each service's `name`, `status`, `pid`,
`log_lines`, `log_capacity`, and `interactive` fields.

## `inspect`

Required argument: exact `service` name. Returns that service's `status`,
`pid`, `cmd`, `cwd`, log buffer size, isolation flags, and `resources`.
`resources` is `{ cpu_percent, memory_bytes, gpu_percent, gpu_memory_bytes }`
for the service process and its children. It is null when that service has no
`monitor` table, when the service is not running, or when the first sample is
not ready yet. Each field is null when that reading is disabled or unavailable.
CPU is the percent of one core since the previous sample. GPU percent is that
tree's share of all GPUs. macOS reports GPU usage only. `monitor` defaults
`cpu` and `memory` to true and `gpu` to false.

## `get_logs`

Required argument: exact `service` name. Optional arguments are `tail` (1–5000,
default 200) and case-sensitive substring `grep`, which filters before tailing.

## `start_service`

Required argument: exact `service` name. Starts that service through the same
path as the web UI. A service that is already `Starting` or `Running` is left
unchanged. The call returns as soon as the start is accepted; status is
`Starting` until the process is up.

## `stop_service`

Required argument: exact `service` name. Stops that service through the same
path as the web UI. A service that is not `Starting` or `Running` is left
unchanged. The call returns as soon as the stop is accepted; status is
`Stopping` until the process exits.

Recommended call order: `list_services`, then `inspect` or a focused
`get_logs`, `start_service`, or `stop_service` call.
"###;

pub const SETUP_MD: &str = r###"# Muxox MCP setup

Enable the embedded server in `muxox.toml`:

```toml
[mcp]
enabled = true
port = 54321
bind = "127.0.0.1"
```

Configure the agent's MCP client with the printed URL, normally
`http://127.0.0.1:54321/mcp`. Muxox must run in web or `--raw` mode; `--tui`
does not start MCP. Keep the bind address localhost-only unless exposure is
intentional.
"###;

pub const AGENTS_OPENAI_YAML: &str = r###"interface:
  display_name: "Muxox"
  short_description: "Inspect and control Muxox services via MCP"
  default_prompt: "Use $muxox to inspect service status and resource usage, read logs, and start or stop services with list_services, inspect, get_logs, start_service, and stop_service."
dependencies:
  tools:
    - type: "mcp"
      value: "muxox"
      description: "Muxox Streamable HTTP MCP for service status, logs, and start/stop."
      transport: "streamable_http"
      url: "http://127.0.0.1:54321/mcp"
"###;
