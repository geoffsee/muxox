// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller

//! Embedded files installed by `muxox install-skill`.

pub const SKILL_MD: &str = r###"---
name: muxox
description: >-
  Inspect Muxox-managed local development services and diagnose failures using
  Muxox MCP tools list_services and get_logs. Use for service status, PIDs,
  logs, startup failures, crash loops, port conflicts, and unhealthy workers.
  Do not use it to start, stop, or restart services.
---

# Muxox

Use Muxox's embedded MCP server to inspect managed services and captured logs.

## Workflow

1. Confirm Muxox is running in web or `--raw` mode, not `--tui`.
2. Confirm `[mcp] enabled = true` in `muxox.toml` and use its printed `/mcp` URL.
3. Call `list_services` first.
4. Call `get_logs` for one implicated service at a time, using the smallest
   useful `tail` and a case-sensitive literal `grep` when appropriate.
5. After a user-operated restart or code/config change, call `list_services`
   again and verify with focused logs.

MCP is read-only: it exposes only `list_services` and `get_logs`. Lifecycle
changes remain a Muxox UI, TUI, or CLI concern.

Read `references/mcp-tools.md` for schemas and `references/setup.md` for setup.

Never reproduce secrets, tokens, cookies, passwords, private URLs, or PII from
logs. Redact sensitive values and report only sanitized excerpts.
"###;

pub const MCP_TOOLS_MD: &str = r###"# Muxox MCP tools

Muxox exposes a Streamable HTTP MCP endpoint at `http://{bind}:{port}/mcp`.
The tools are read-only.

## `list_services`

Call with no arguments. It returns each service's `name`, `status`, `pid`,
`log_lines`, `log_capacity`, and `interactive` fields.

## `get_logs`

Required argument: exact `service` name. Optional arguments are `tail` (1–5000,
default 200) and case-sensitive substring `grep`, which filters before tailing.

Recommended call order: `list_services`, then focused `get_logs` calls.
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
  short_description: "Inspect Muxox services and logs via MCP"
  default_prompt: "Use $muxox to inspect service status and diagnose local development failures with list_services and get_logs."
dependencies:
  tools:
    - type: "mcp"
      value: "muxox"
      description: "Muxox Streamable HTTP MCP for read-only service status and logs."
      transport: "streamable_http"
      url: "http://127.0.0.1:54321/mcp"
"###;
