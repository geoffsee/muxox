// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

//! Minimal embedded MCP (Model Context Protocol) server.
//!
//! Speaks JSON-RPC 2.0 over the MCP Streamable HTTP transport (POST-only,
//! single JSON response).  Exposes a small set of tools that let MCP-aware
//! agents inspect muxox-managed services, read their captured logs, and start
//! or stop a service by name.

use crate::resources::{self, Resources};
use anyhow::Result;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use muxox_core::app::{self, App, Status};
use muxox_core::config::McpCfg;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Default MCP protocol version returned when the client doesn't specify one.
const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Cap on lines returned by `get_logs` regardless of the requested `tail`.
const MAX_LOG_LINES: usize = 5000;

/// Default number of trailing log lines returned when `tail` is omitted.
const DEFAULT_LOG_TAIL: usize = 200;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {method}"),
            data: None,
        }
    }
    fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            data: None,
        }
    }
}

#[derive(Clone)]
pub struct McpState {
    pub app: Arc<Mutex<App>>,
    /// Latest opted-in resource sample per service index. Empty until a sampler
    /// is running. `None` slots mean "no sample" (stopped, or not yet primed).
    pub resources: Arc<Mutex<Vec<Option<Resources>>>>,
}

pub fn router(state: Arc<McpState>) -> Router {
    Router::new()
        .route("/mcp", post(mcp_post).get(mcp_get))
        .with_state(state)
}

/// Server-initiated SSE streams aren't supported -- muxox only speaks
/// request/response.  Return 405 so well-behaved clients fall back to POST.
async fn mcp_get() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        "muxox MCP server is POST-only; server-initiated SSE is not supported",
    )
}

async fn mcp_post(State(state): State<Arc<McpState>>, Json(req): Json<JsonRpcRequest>) -> Response {
    // JSON-RPC notifications carry no `id` and expect no response body.
    let is_notification = req.id.is_none();
    let id = req.id.clone().unwrap_or(Value::Null);

    let outcome: std::result::Result<Value, JsonRpcError> = match req.method.as_str() {
        "initialize" => Ok(handle_initialize(&req.params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(req.params, &state).await,
        // Notifications we silently accept.
        m if m.starts_with("notifications/") => {
            return StatusCode::ACCEPTED.into_response();
        }
        other => Err(JsonRpcError::method_not_found(other)),
    };

    if is_notification {
        return StatusCode::ACCEPTED.into_response();
    }

    let resp = match outcome {
        Ok(value) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(value),
            error: None,
        },
        Err(err) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(err),
        },
    };

    Json(resp).into_response()
}

fn handle_initialize(params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_PROTOCOL_VERSION)
        .to_string();

    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": "muxox",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "Use `list_services` to discover managed services, `inspect` for one service's status, command, and resource usage, `get_logs` to retrieve captured stdout/stderr, and `start_service` or `stop_service` to start or stop one service by name.",
    })
}

fn handle_tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "list_services",
                "description": "List every service muxox is managing, with status, PID, and current in-memory log size.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "inspect",
                "description": "Return one service's status, pid, command, working directory, log buffer size, and latest resource sample. resources is null when monitoring is off, the service is not running, or the first sample is not ready yet. cpu_percent, memory_bytes, gpu_percent, and gpu_memory_bytes are null when that reading is disabled or unavailable.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "Service name as defined in muxox.toml."
                        }
                    },
                    "required": ["service"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_logs",
                "description": "Return recent log lines captured from a service's stdout/stderr. Lines are returned oldest-first.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "Service name as defined in muxox.toml."
                        },
                        "tail": {
                            "type": "integer",
                            "description": "Maximum number of trailing lines to return. Defaults to 200, capped at 5000.",
                            "minimum": 1,
                            "maximum": 5000
                        },
                        "grep": {
                            "type": "string",
                            "description": "Optional case-sensitive substring filter applied before tailing."
                        }
                    },
                    "required": ["service"],
                    "additionalProperties": false
                }
            },
            {
                "name": "start_service",
                "description": "Start one managed service by name, using the same start path as the web UI. No-op when the service is already Starting or Running. Returns as soon as the start is accepted; status is Starting until the process is up.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "Service name as defined in muxox.toml."
                        }
                    },
                    "required": ["service"],
                    "additionalProperties": false
                }
            },
            {
                "name": "stop_service",
                "description": "Stop one managed service by name, using the same stop path as the web UI. No-op unless the service is Starting or Running. Returns as soon as the stop is accepted; status is Stopping until the process exits.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "Service name as defined in muxox.toml."
                        }
                    },
                    "required": ["service"],
                    "additionalProperties": false
                }
            }
        ]
    })
}

async fn handle_tools_call(
    params: Value,
    state: &McpState,
) -> std::result::Result<Value, JsonRpcError> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| JsonRpcError::invalid_params("missing 'name' parameter"))?;
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);

    match name {
        "list_services" => list_services_tool(state).await,
        "inspect" => inspect_tool(args, state).await,
        "get_logs" => get_logs_tool(args, state).await,
        "start_service" => service_action_tool(args, state, ServiceAction::Start).await,
        "stop_service" => service_action_tool(args, state, ServiceAction::Stop).await,
        other => Err(JsonRpcError::invalid_params(format!(
            "unknown tool: {other}"
        ))),
    }
}

async fn list_services_tool(state: &McpState) -> std::result::Result<Value, JsonRpcError> {
    let app = state.app.lock().await;
    let services: Vec<Value> = app
        .services
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            json!({
                "index": idx,
                "name": s.cfg.name,
                "status": s.status.as_str(),
                "pid": s.pid,
                "log_lines": s.log.len(),
                "log_capacity": s.cfg.log_capacity,
                "interactive": s.cfg.interactive,
            })
        })
        .collect();

    let pretty = serde_json::to_string_pretty(&services).unwrap_or_else(|_| "[]".to_string());

    Ok(json!({
        "content": [{ "type": "text", "text": pretty }],
        "structuredContent": { "services": services },
    }))
}

async fn inspect_tool(args: Value, state: &McpState) -> std::result::Result<Value, JsonRpcError> {
    let service_name = require_service_name(&args)?;
    let app = state.app.lock().await;
    let idx = find_service(&app, &service_name)?;
    let svc = &app.services[idx];
    let slots = state.resources.lock().await;
    let sampled = slots.get(idx).copied().flatten();
    let resources = if matches!(svc.status, Status::Running | Status::Stopping) {
        svc.cfg
            .monitor
            .as_ref()
            .filter(|cfg| cfg.enabled())
            .and_then(|cfg| sampled.and_then(|sample| sample.mask(cfg.cpu, cfg.memory, cfg.gpu)))
    } else {
        None
    };
    let pid = svc.pid;
    let status = svc.status.as_str();
    let cwd = svc.cfg.cwd.as_ref().map(|path| path.display().to_string());
    let resource_text = match resources {
        Some(sample) => resources::format_resources(sample),
        None => "n/a".to_string(),
    };
    let pid_text = pid
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    let text = format!("{service_name}: {status} pid={pid_text} resources={resource_text}");
    let resources_json = resources.map(|sample| {
        json!({
            "cpu_percent": sample.cpu_percent,
            "memory_bytes": sample.memory_bytes,
            "gpu_percent": sample.gpu_percent,
            "gpu_memory_bytes": sample.gpu_memory_bytes,
        })
    });

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": {
            "service": {
                "index": idx,
                "name": svc.cfg.name,
                "status": status,
                "pid": pid,
                "cmd": svc.cfg.cmd,
                "cwd": cwd,
                "interactive": svc.cfg.interactive,
                "pty": svc.cfg.pty,
                "log_lines": svc.log.len(),
                "log_capacity": svc.cfg.log_capacity,
                "isolation": {
                    "process": svc.cfg.isolation.process,
                    "filesystem": svc.cfg.isolation.filesystem,
                    "network": svc.cfg.isolation.network,
                },
                "resources": resources_json,
            }
        },
    }))
}

fn require_service_name(args: &Value) -> std::result::Result<String, JsonRpcError> {
    args.get("service")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| JsonRpcError::invalid_params("missing 'service' argument"))
}

fn find_service(app: &App, service_name: &str) -> std::result::Result<usize, JsonRpcError> {
    app.services
        .iter()
        .position(|s| s.cfg.name == service_name)
        .ok_or_else(|| JsonRpcError::invalid_params(format!("service not found: {service_name}")))
}

async fn get_logs_tool(args: Value, state: &McpState) -> std::result::Result<Value, JsonRpcError> {
    let service_name = args
        .get("service")
        .and_then(|v| v.as_str())
        .ok_or_else(|| JsonRpcError::invalid_params("missing 'service' argument"))?
        .to_string();

    let tail = args
        .get("tail")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_LOG_TAIL)
        .clamp(1, MAX_LOG_LINES);

    let grep = args
        .get("grep")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let app = state.app.lock().await;
    let svc = app
        .services
        .iter()
        .find(|s| s.cfg.name == service_name)
        .ok_or_else(|| {
            JsonRpcError::invalid_params(format!("service not found: {service_name}"))
        })?;

    let filtered: Vec<&str> = match &grep {
        Some(needle) => svc
            .log
            .iter()
            .filter(|line| line.contains(needle.as_str()))
            .map(|s| s.as_str())
            .collect(),
        None => svc.log.iter().map(|s| s.as_str()).collect(),
    };

    let total_after_filter = filtered.len();
    let start = total_after_filter.saturating_sub(tail);
    let returned = &filtered[start..];
    let text = returned.join("\n");

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": {
            "service": svc.cfg.name,
            "status": svc.status.as_str(),
            "lines_returned": returned.len(),
            "total_lines_after_filter": total_after_filter,
            "total_lines": svc.log.len(),
            "grep": grep,
        },
    }))
}

enum ServiceAction {
    Start,
    Stop,
}

/// Start or stop one service by name via [`app::start_service`] / [`app::stop_service`].
async fn service_action_tool(
    args: Value,
    state: &McpState,
    action: ServiceAction,
) -> std::result::Result<Value, JsonRpcError> {
    let service_name = args
        .get("service")
        .and_then(|v| v.as_str())
        .ok_or_else(|| JsonRpcError::invalid_params("missing 'service' argument"))?
        .to_string();

    let mut app = state.app.lock().await;
    let idx = app
        .services
        .iter()
        .position(|s| s.cfg.name == service_name)
        .ok_or_else(|| {
            JsonRpcError::invalid_params(format!("service not found: {service_name}"))
        })?;

    let previous = app.services[idx].status;
    match action {
        ServiceAction::Start => app::start_service(idx, &mut app),
        ServiceAction::Stop => app::stop_service(idx, &mut app),
    }
    let status = app.services[idx].status;
    let pid = app.services[idx].pid;
    let changed = previous != status;
    let text = if changed {
        format!(
            "{service_name}: {} -> {}",
            previous.as_str(),
            status.as_str()
        )
    } else {
        format!("{service_name}: {} (unchanged)", status.as_str())
    };

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": {
            "service": service_name,
            "index": idx,
            "previous_status": previous.as_str(),
            "status": status.as_str(),
            "changed": changed,
            "pid": pid,
        },
    }))
}

/// Bind and serve the MCP server.  Runs forever; intended to be spawned as a
/// background task.
pub async fn run_mcp_server(
    cfg: McpCfg,
    app: Arc<Mutex<App>>,
    resources: Arc<Mutex<Vec<Option<Resources>>>>,
) -> Result<()> {
    let state = Arc::new(McpState { app, resources });
    let router = router(state);

    let addr: SocketAddr = format!("{}:{}", cfg.bind, cfg.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    println!("MCP server listening at http://{actual}/mcp");

    axum::serve(listener, router).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use muxox_core::app::{App, AppMsg, ServiceState, Status};
    use muxox_core::config::{MonitorCfg, ServiceCfg};
    use muxox_core::isolation::default_isolation;
    use tokio::sync::mpsc;

    fn make_app(services: Vec<(&str, Vec<&str>)>) -> Arc<Mutex<App>> {
        let (tx, _rx) = mpsc::unbounded_channel::<AppMsg>();
        let mut states = Vec::new();
        for (name, logs) in services {
            let mut s = ServiceState::new(ServiceCfg {
                name: name.to_string(),
                cmd: "true".into(),
                cwd: None,
                log_capacity: 100,
                interactive: false,
                pty: false,
                env_file: None,
                isolation: Default::default(),
                hooks: Default::default(),
                monitor: None,
            });
            s.status = Status::Running;
            for l in logs {
                s.push_log(l);
            }
            states.push(s);
        }
        Arc::new(Mutex::new(App {
            services: states,
            selected: 0,
            log_offset_from_end: 0,
            tx,
            input_mode: false,
            input_buffer: String::new(),
            isolation: default_isolation(),
            update_notice: None,
            hooks: Default::default(),
            config_dir: None,
            settle_tx: None,
        }))
    }

    fn test_state(app: Arc<Mutex<App>>) -> McpState {
        McpState {
            app,
            resources: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[test]
    fn initialize_echoes_protocol_version() {
        let v = handle_initialize(&json!({ "protocolVersion": "2025-06-18" }));
        assert_eq!(v["protocolVersion"], "2025-06-18");
        assert_eq!(v["serverInfo"]["name"], "muxox");
    }

    #[test]
    fn initialize_falls_back_to_default_protocol_version() {
        let v = handle_initialize(&json!({}));
        assert_eq!(v["protocolVersion"], DEFAULT_PROTOCOL_VERSION);
    }

    #[test]
    fn tools_list_advertises_known_tools() {
        let v = handle_tools_list();
        let names: Vec<&str> = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"list_services"));
        assert!(names.contains(&"inspect"));
        assert!(names.contains(&"get_logs"));
        assert!(names.contains(&"start_service"));
        assert!(names.contains(&"stop_service"));
    }

    #[tokio::test]
    async fn list_services_tool_returns_all_services() {
        let app = make_app(vec![("alpha", vec!["a1", "a2"]), ("beta", vec!["b1"])]);
        let state = test_state(app);
        let result = list_services_tool(&state).await.unwrap();
        let services = result["structuredContent"]["services"].as_array().unwrap();
        assert_eq!(services.len(), 2);
        assert_eq!(services[0]["name"], "alpha");
        assert_eq!(services[0]["log_lines"], 2);
        assert_eq!(services[1]["name"], "beta");
        assert_eq!(services[1]["log_lines"], 1);
    }

    #[tokio::test]
    async fn get_logs_tool_returns_tail() {
        let app = make_app(vec![("svc", vec!["one", "two", "three", "four", "five"])]);
        let state = test_state(app);
        let result = get_logs_tool(json!({ "service": "svc", "tail": 2 }), &state)
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "four\nfive");
        assert_eq!(result["structuredContent"]["lines_returned"], 2);
        assert_eq!(result["structuredContent"]["total_lines"], 5);
    }

    #[tokio::test]
    async fn get_logs_tool_grep_filters_then_tails() {
        let app = make_app(vec![(
            "svc",
            vec!["error: a", "info: b", "error: c", "warn: d", "error: e"],
        )]);
        let state = test_state(app);
        let result = get_logs_tool(
            json!({ "service": "svc", "tail": 2, "grep": "error" }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(result["content"][0]["text"], "error: c\nerror: e");
        assert_eq!(result["structuredContent"]["total_lines_after_filter"], 3);
        assert_eq!(result["structuredContent"]["lines_returned"], 2);
    }

    #[tokio::test]
    async fn get_logs_tool_unknown_service_errors() {
        let app = make_app(vec![("svc", vec!["x"])]);
        let state = test_state(app);
        let err = get_logs_tool(json!({ "service": "missing" }), &state)
            .await
            .unwrap_err();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("missing"));
    }

    #[tokio::test]
    async fn get_logs_tool_missing_service_arg_errors() {
        let app = make_app(vec![("svc", vec!["x"])]);
        let state = test_state(app);
        let err = get_logs_tool(json!({}), &state).await.unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn handle_tools_call_dispatches_known_tools() {
        let app = make_app(vec![("svc", vec!["x"])]);
        let state = test_state(app);
        let v = handle_tools_call(json!({ "name": "list_services", "arguments": {} }), &state)
            .await
            .unwrap();
        assert!(v["structuredContent"]["services"].is_array());
    }

    #[tokio::test]
    async fn start_service_tool_starts_stopped_service() {
        let app = make_app(vec![("alpha", vec![])]);
        {
            let mut guard = app.lock().await;
            guard.services[0].status = Status::Stopped;
            guard.services[0].cfg.cmd = "true".into();
        }
        let state = test_state(Arc::clone(&app));
        let result =
            service_action_tool(json!({ "service": "alpha" }), &state, ServiceAction::Start)
                .await
                .unwrap();
        assert_eq!(result["content"][0]["text"], "alpha: Stopped -> Starting");
        assert_eq!(result["structuredContent"]["status"], "Starting");
        assert_eq!(result["structuredContent"]["previous_status"], "Stopped");
        assert_eq!(result["structuredContent"]["changed"], true);
        assert_eq!(result["structuredContent"]["index"], 0);
        assert_eq!(app.lock().await.services[0].status, Status::Starting);
    }

    #[tokio::test]
    async fn start_service_tool_is_noop_when_already_running() {
        let app = make_app(vec![("alpha", vec![])]);
        let state = test_state(app);
        let result =
            service_action_tool(json!({ "service": "alpha" }), &state, ServiceAction::Start)
                .await
                .unwrap();
        assert_eq!(result["content"][0]["text"], "alpha: Running (unchanged)");
        assert_eq!(result["structuredContent"]["changed"], false);
        assert_eq!(result["structuredContent"]["status"], "Running");
    }

    #[tokio::test]
    async fn start_service_tool_targets_the_first_matching_name() {
        let app = make_app(vec![("dup", vec![]), ("dup", vec![])]);
        {
            let mut guard = app.lock().await;
            guard.services[1].status = Status::Stopped;
        }
        let state = test_state(Arc::clone(&app));
        let result = service_action_tool(json!({ "service": "dup" }), &state, ServiceAction::Start)
            .await
            .unwrap();
        assert_eq!(result["structuredContent"]["index"], 0);
        assert_eq!(result["structuredContent"]["changed"], false);
        let guard = app.lock().await;
        assert_eq!(guard.services[0].status, Status::Running);
        assert_eq!(guard.services[1].status, Status::Stopped);
    }

    #[tokio::test]
    async fn stop_service_tool_stops_running_service() {
        let app = make_app(vec![("alpha", vec![])]);
        {
            let mut guard = app.lock().await;
            // Nonexistent pid: terminate signals this id, not a live process.
            guard.services[0].pid = Some(i32::MAX as u32);
        }
        let state = test_state(Arc::clone(&app));
        let result =
            service_action_tool(json!({ "service": "alpha" }), &state, ServiceAction::Stop)
                .await
                .unwrap();
        assert_eq!(result["content"][0]["text"], "alpha: Running -> Stopping");
        assert_eq!(result["structuredContent"]["status"], "Stopping");
        assert_eq!(result["structuredContent"]["changed"], true);
        assert_eq!(app.lock().await.services[0].status, Status::Stopping);
    }

    #[tokio::test]
    async fn stop_service_tool_is_noop_when_already_stopped() {
        let app = make_app(vec![("alpha", vec![])]);
        {
            app.lock().await.services[0].status = Status::Stopped;
        }
        let state = test_state(Arc::clone(&app));
        let result =
            service_action_tool(json!({ "service": "alpha" }), &state, ServiceAction::Stop)
                .await
                .unwrap();
        assert_eq!(result["content"][0]["text"], "alpha: Stopped (unchanged)");
        assert_eq!(result["structuredContent"]["changed"], false);
        assert_eq!(app.lock().await.services[0].status, Status::Stopped);
    }

    #[tokio::test]
    async fn service_action_tool_unknown_service_errors() {
        let app = make_app(vec![("alpha", vec![])]);
        let state = test_state(app);
        let err = service_action_tool(
            json!({ "service": "missing" }),
            &state,
            ServiceAction::Start,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("missing"));
    }

    #[tokio::test]
    async fn service_action_tool_missing_service_arg_errors() {
        let app = make_app(vec![("alpha", vec![])]);
        let state = test_state(app);
        let err = service_action_tool(json!({}), &state, ServiceAction::Stop)
            .await
            .unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn handle_tools_call_dispatches_start_and_stop() {
        let app = make_app(vec![("alpha", vec![])]);
        app.lock().await.services[0].pid = Some(i32::MAX as u32);
        let state = test_state(app);
        let started = handle_tools_call(
            json!({ "name": "start_service", "arguments": { "service": "alpha" } }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(started["structuredContent"]["changed"], false);
        let stopped = handle_tools_call(
            json!({ "name": "stop_service", "arguments": { "service": "alpha" } }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(stopped["structuredContent"]["status"], "Stopping");
    }

    #[tokio::test]
    async fn inspect_tool_returns_details_and_resource_sample() {
        let app = make_app(vec![("alpha", vec!["line"])]);
        {
            let mut guard = app.lock().await;
            guard.services[0].pid = Some(42);
            guard.services[0].cfg.cmd = "sleep 30".into();
            guard.services[0].cfg.cwd = Some("packages/alpha".into());
            guard.services[0].cfg.isolation.process = true;
            guard.services[0].cfg.monitor = Some(MonitorCfg {
                cpu: true,
                memory: true,
                gpu: false,
            });
        }
        let state = McpState {
            app,
            resources: Arc::new(Mutex::new(vec![Some(Resources {
                cpu_percent: Some(1.5),
                memory_bytes: Some(2048),
                gpu_percent: None,
                gpu_memory_bytes: None,
            })])),
        };
        let result = inspect_tool(json!({ "service": "alpha" }), &state)
            .await
            .unwrap();
        let service = &result["structuredContent"]["service"];
        assert_eq!(service["index"], 0);
        assert_eq!(service["name"], "alpha");
        assert_eq!(service["status"], "Running");
        assert_eq!(service["pid"], 42);
        assert_eq!(service["cmd"], "sleep 30");
        assert_eq!(service["cwd"], "packages/alpha");
        assert_eq!(service["log_lines"], 1);
        assert_eq!(service["isolation"]["process"], true);
        assert_eq!(service["resources"]["cpu_percent"], 1.5);
        assert_eq!(service["resources"]["memory_bytes"], 2048);
        assert!(service["resources"]["gpu_percent"].is_null());
        assert!(service["resources"]["gpu_memory_bytes"].is_null());
        assert_eq!(
            result["content"][0]["text"],
            "alpha: Running pid=42 resources=1.5% · 2.0 KB"
        );
    }

    #[tokio::test]
    async fn inspect_tool_appends_gpu_when_present() {
        let app = make_app(vec![("alpha", vec![])]);
        {
            let mut guard = app.lock().await;
            guard.services[0].pid = Some(42);
            guard.services[0].cfg.monitor = Some(MonitorCfg {
                cpu: true,
                memory: true,
                gpu: true,
            });
        }
        let state = McpState {
            app,
            resources: Arc::new(Mutex::new(vec![Some(Resources {
                cpu_percent: Some(1.5),
                memory_bytes: Some(2048),
                gpu_percent: Some(12.0),
                gpu_memory_bytes: Some(256 * 1024 * 1024),
            })])),
        };
        let result = inspect_tool(json!({ "service": "alpha" }), &state)
            .await
            .unwrap();
        let resources = &result["structuredContent"]["service"]["resources"];
        assert_eq!(resources["gpu_percent"], 12.0);
        assert_eq!(resources["gpu_memory_bytes"], 256 * 1024 * 1024);
        assert_eq!(
            result["content"][0]["text"],
            "alpha: Running pid=42 resources=1.5% · 2.0 KB · GPU 12.0% · 256.0 MB"
        );
    }

    #[tokio::test]
    async fn inspect_tool_hides_stale_resources_when_stopped() {
        let app = make_app(vec![("alpha", vec![])]);
        app.lock().await.services[0].status = Status::Stopped;
        let state = McpState {
            app,
            resources: Arc::new(Mutex::new(vec![Some(Resources {
                cpu_percent: Some(9.0),
                memory_bytes: Some(100),
                gpu_percent: Some(3.0),
                gpu_memory_bytes: Some(64),
            })])),
        };
        let result = inspect_tool(json!({ "service": "alpha" }), &state)
            .await
            .unwrap();
        assert!(result["structuredContent"]["service"]["resources"].is_null());
        assert_eq!(
            result["content"][0]["text"],
            "alpha: Stopped pid=none resources=n/a"
        );
    }

    #[tokio::test]
    async fn inspect_tool_hides_resources_when_monitoring_is_off() {
        let app = make_app(vec![("alpha", vec![])]);
        app.lock().await.services[0].pid = Some(42);
        let state = McpState {
            app,
            resources: Arc::new(Mutex::new(vec![Some(Resources {
                cpu_percent: Some(1.5),
                memory_bytes: Some(2048),
                gpu_percent: Some(12.0),
                gpu_memory_bytes: None,
            })])),
        };
        let result = inspect_tool(json!({ "service": "alpha" }), &state)
            .await
            .unwrap();
        assert!(result["structuredContent"]["service"]["resources"].is_null());
        assert_eq!(
            result["content"][0]["text"],
            "alpha: Running pid=42 resources=n/a"
        );
    }

    #[tokio::test]
    async fn inspect_tool_returns_only_the_readings_that_are_enabled() {
        let app = make_app(vec![("alpha", vec![])]);
        {
            let mut guard = app.lock().await;
            guard.services[0].pid = Some(42);
            guard.services[0].cfg.monitor = Some(MonitorCfg {
                cpu: false,
                memory: true,
                gpu: false,
            });
        }
        let state = McpState {
            app,
            resources: Arc::new(Mutex::new(vec![Some(Resources {
                cpu_percent: Some(1.5),
                memory_bytes: Some(2048),
                gpu_percent: Some(12.0),
                gpu_memory_bytes: Some(512),
            })])),
        };
        let result = inspect_tool(json!({ "service": "alpha" }), &state)
            .await
            .unwrap();
        let resources = &result["structuredContent"]["service"]["resources"];
        assert!(resources["cpu_percent"].is_null());
        assert_eq!(resources["memory_bytes"], 2048);
        assert!(resources["gpu_percent"].is_null());
        assert!(resources["gpu_memory_bytes"].is_null());
        assert_eq!(
            result["content"][0]["text"],
            "alpha: Running pid=42 resources=2.0 KB"
        );
    }

    #[tokio::test]
    async fn inspect_tool_reports_no_sample_yet() {
        let app = make_app(vec![("alpha", vec![])]);
        app.lock().await.services[0].pid = Some(7);
        let state = test_state(app);
        let result = inspect_tool(json!({ "service": "alpha" }), &state)
            .await
            .unwrap();
        assert!(result["structuredContent"]["service"]["resources"].is_null());
        assert_eq!(
            result["content"][0]["text"],
            "alpha: Running pid=7 resources=n/a"
        );
    }

    #[tokio::test]
    async fn inspect_tool_unknown_service_and_missing_arg_error() {
        let app = make_app(vec![("alpha", vec![])]);
        let state = test_state(app);
        let missing = inspect_tool(json!({ "service": "nope" }), &state)
            .await
            .unwrap_err();
        assert_eq!(missing.code, -32602);
        assert!(missing.message.contains("nope"));
        let no_arg = inspect_tool(json!({}), &state).await.unwrap_err();
        assert_eq!(no_arg.code, -32602);
    }

    #[tokio::test]
    async fn handle_tools_call_dispatches_inspect() {
        let app = make_app(vec![("alpha", vec![])]);
        let state = test_state(app);
        let result = handle_tools_call(
            json!({ "name": "inspect", "arguments": { "service": "alpha" } }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["service"]["name"], "alpha");
    }

    #[tokio::test]
    async fn handle_tools_call_unknown_tool_errors() {
        let app = make_app(vec![("svc", vec!["x"])]);
        let state = test_state(app);
        let err = handle_tools_call(json!({ "name": "nope" }), &state)
            .await
            .unwrap_err();
        assert_eq!(err.code, -32602);
    }
}
