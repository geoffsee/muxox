pub mod mcp;
mod ui;
mod web;
mod ws_proto;

#[cfg(test)]
mod ws_proto_tests;

pub use mcp::run_mcp_server;
pub use web::run_web_mode;
