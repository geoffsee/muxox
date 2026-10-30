#[cfg(test)]
mod tests {
    use crate::ws_proto::{ServiceInfo, WsInbound, WsOutbound};
    use axum::body::Bytes;
    use axum::extract::ws::Message;

    // --- WsOutbound serialization ---

    #[test]
    fn init_message_serializes_with_type_tag() {
        let msg = WsOutbound::Init {
            update: None,
            services: vec![ServiceInfo {
                name: "web".into(),
                status: "Running".into(),
                logs: vec!["line 1".into()],
                interactive: false,
                resources: None,
            }],
        };
        let ws_msg = msg.to_message();
        let bytes = match ws_msg {
            Message::Binary(b) => b,
            _ => panic!("expected Binary"),
        };
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["type"], "Init");
        assert_eq!(json["services"][0]["name"], "web");
        assert_eq!(json["services"][0]["status"], "Running");
        assert!(json["services"][0]["logs"].is_array());
    }

    #[test]
    fn log_message_serializes_correctly() {
        let msg = WsOutbound::Log {
            idx: 2,
            line: "hello".into(),
        };
        let ws_msg = msg.to_message();
        let bytes = match ws_msg {
            Message::Binary(b) => b,
            _ => panic!("expected Binary"),
        };
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["type"], "Log");
        assert_eq!(json["idx"], 2);
        assert_eq!(json["line"], "hello");
    }

    #[test]
    fn status_message_serializes_correctly() {
        let msg = WsOutbound::Status {
            idx: 0,
            status: "Stopped".into(),
        };
        let ws_msg = msg.to_message();
        let bytes = match ws_msg {
            Message::Binary(b) => b,
            _ => panic!("expected Binary"),
        };
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["type"], "Status");
        assert_eq!(json["status"], "Stopped");
    }

    // --- WsInbound deserialization ---

    #[test]
    fn command_from_text_message() {
        let json = r#"{"type":"Command","idx":1,"command":"start"}"#;
        let msg = Message::Text(json.into());
        let cmd = WsInbound::from_message(msg).unwrap();
        match cmd {
            WsInbound::Command {
                idx,
                command,
                input,
            } => {
                assert_eq!(idx, 1);
                assert_eq!(command, "start");
                assert!(input.is_none());
            }
        }
    }

    #[test]
    fn command_from_binary_message() {
        let json = br#"{"type":"Command","idx":0,"command":"stop"}"#;
        let msg = Message::Binary(Bytes::from_static(json));
        let cmd = WsInbound::from_message(msg).unwrap();
        match cmd {
            WsInbound::Command { idx, command, .. } => {
                assert_eq!(idx, 0);
                assert_eq!(command, "stop");
            }
        }
    }

    #[test]
    fn stdin_command_with_input() {
        let json = r#"{"type":"Command","idx":0,"command":"stdin","input":"hello\n"}"#;
        let msg = Message::Text(json.into());
        let cmd = WsInbound::from_message(msg).unwrap();
        match cmd {
            WsInbound::Command {
                idx,
                command,
                input,
            } => {
                assert_eq!(idx, 0);
                assert_eq!(command, "stdin");
                assert_eq!(input.unwrap(), "hello\n");
            }
        }
    }

    #[test]
    fn command_input_defaults_to_none() {
        let json = r#"{"type":"Command","idx":0,"command":"start"}"#;
        let msg = Message::Text(json.into());
        let cmd = WsInbound::from_message(msg).unwrap();
        match cmd {
            WsInbound::Command { input, .. } => {
                assert!(input.is_none());
            }
        }
    }

    #[test]
    fn invalid_json_returns_none() {
        let msg = Message::Text("not json".into());
        assert!(WsInbound::from_message(msg).is_none());
    }

    #[test]
    fn ping_message_returns_none() {
        let msg = Message::Ping(Bytes::new());
        assert!(WsInbound::from_message(msg).is_none());
    }

    #[test]
    fn close_message_returns_none() {
        let msg = Message::Close(None);
        assert!(WsInbound::from_message(msg).is_none());
    }

    // --- Round-trip ---

    #[test]
    fn init_with_multiple_services() {
        let msg = WsOutbound::Init {
            update: None,
            services: vec![
                ServiceInfo {
                    name: "web".into(),
                    status: "Running".into(),
                    logs: vec![],
                    interactive: false,
                    resources: Some(crate::resources::Resources {
                        cpu_percent: Some(4.0),
                        memory_bytes: Some(48 * 1024 * 1024),
                        gpu_percent: Some(12.0),
                        gpu_memory_bytes: None,
                    }),
                },
                ServiceInfo {
                    name: "api".into(),
                    status: "Stopped".into(),
                    logs: vec!["err".into()],
                    interactive: true,
                    resources: None,
                },
            ],
        };
        let ws_msg = msg.to_message();
        let bytes = match ws_msg {
            Message::Binary(b) => b,
            _ => panic!("expected Binary"),
        };
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["services"].as_array().unwrap().len(), 2);
        assert_eq!(json["services"][1]["interactive"], true);
        assert_eq!(json["services"][0]["resources"]["cpu_percent"], 4.0);
        assert_eq!(
            json["services"][0]["resources"]["memory_bytes"],
            48 * 1024 * 1024
        );
        assert_eq!(json["services"][0]["resources"]["gpu_percent"], 12.0);
        assert!(json["services"][0]["resources"]["gpu_memory_bytes"].is_null());
        assert!(json["services"][1].get("resources").is_none());
    }

    #[test]
    fn resources_message_round_trips_sample_and_clear() {
        let sample = WsOutbound::Resources {
            idx: 1,
            resources: Some(crate::resources::Resources {
                cpu_percent: Some(1.5),
                memory_bytes: Some(2048),
                gpu_percent: None,
                gpu_memory_bytes: None,
            }),
        };
        let json = serde_json::to_value(&sample).unwrap();
        assert_eq!(json["type"], "Resources");
        assert_eq!(json["idx"], 1);
        assert_eq!(json["resources"]["memory_bytes"], 2048);

        let clear = WsOutbound::Resources {
            idx: 1,
            resources: None,
        };
        let json = serde_json::to_value(&clear).unwrap();
        assert_eq!(json["type"], "Resources");
        assert!(json["resources"].is_null());
    }
}

#[test]
fn update_notice_survives_initial_and_live_serialization() {
    use crate::ws_proto::WsOutbound;
    use muxox_core::updates::UpdateNotice;
    let notice = UpdateNotice {
        current_version: "1.0.0".into(),
        latest_version: "2.0.0".into(),
    };
    let initial = serde_json::to_value(WsOutbound::Init {
        services: vec![],
        update: Some(notice.clone()),
    })
    .unwrap();
    let live = serde_json::to_value(WsOutbound::UpdateAvailable { update: notice }).unwrap();
    assert_eq!(initial["update"], live["update"]);
    assert_eq!(initial["update"]["latest_version"], "2.0.0");
    assert_eq!(live["type"], "UpdateAvailable");
}
