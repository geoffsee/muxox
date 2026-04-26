// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

//! Minimal embedded MCP (Model Context Protocol) server.
//!
//! Speaks JSON-RPC 2.0 over the MCP Streamable HTTP transport (POST-only,
//! single JSON response).  Exposes a small set of tools that let MCP-aware
//! agents inspect muxox-managed services and read their captured logs.

use anyhow::Result;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use muxox_core::app::App;
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
        "instructions": "Use `list_services` to discover managed services and `get_logs` to retrieve their captured stdout/stderr.",
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
        "get_logs" => get_logs_tool(args, state).await,
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

/// Bind and serve the MCP server.  Runs forever; intended to be spawned as a
/// background task.
pub async fn run_mcp_server(cfg: McpCfg, app: Arc<Mutex<App>>) -> Result<()> {
    let state = Arc::new(McpState { app });
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
    use muxox_core::config::ServiceCfg;
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
        }))
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
        assert!(names.contains(&"get_logs"));
    }

    #[tokio::test]
    async fn list_services_tool_returns_all_services() {
        let app = make_app(vec![("alpha", vec!["a1", "a2"]), ("beta", vec!["b1"])]);
        let state = McpState { app };
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
        let state = McpState { app };
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
        let state = McpState { app };
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
        let state = McpState { app };
        let err = get_logs_tool(json!({ "service": "missing" }), &state)
            .await
            .unwrap_err();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("missing"));
    }

    #[tokio::test]
    async fn get_logs_tool_missing_service_arg_errors() {
        let app = make_app(vec![("svc", vec!["x"])]);
        let state = McpState { app };
        let err = get_logs_tool(json!({}), &state).await.unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn handle_tools_call_dispatches_known_tools() {
        let app = make_app(vec![("svc", vec!["x"])]);
        let state = McpState { app };
        let v = handle_tools_call(json!({ "name": "list_services", "arguments": {} }), &state)
            .await
            .unwrap();
        assert!(v["structuredContent"]["services"].is_array());
    }

    #[tokio::test]
    async fn handle_tools_call_unknown_tool_errors() {
        let app = make_app(vec![("svc", vec!["x"])]);
        let state = McpState { app };
        let err = handle_tools_call(json!({ "name": "nope" }), &state)
            .await
            .unwrap_err();
        assert_eq!(err.code, -32602);
    }
}
