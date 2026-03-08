use axum::extract::ws::Message;
use serde::{Deserialize, Serialize};

/// Messages sent from the server to connected web clients.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum WsOutbound {
    Init { services: Vec<ServiceInfo> },
    Log { idx: usize, line: String },
    Status { idx: usize, status: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub name: String,
    pub status: String,
    pub logs: Vec<String>,
    pub interactive: bool,
}

/// Messages received from web clients.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum WsInbound {
    Command {
        idx: usize,
        command: String,
        #[serde(default)]
        input: Option<String>,
    },
}

impl WsOutbound {
    /// Encode as a binary WebSocket frame (JSON bytes).
    pub fn to_message(&self) -> Message {
        let bytes = serde_json::to_vec(self).expect("WsOutbound serialization cannot fail");
        Message::Binary(bytes)
    }
}

impl WsInbound {
    /// Decode a WebSocket message (text or binary) into an inbound command.
    pub fn from_message(msg: Message) -> Option<Self> {
        match msg {
            Message::Binary(bytes) => serde_json::from_slice(&bytes).ok(),
            Message::Text(text) => serde_json::from_str(&text).ok(),
            _ => None,
        }
    }
}
