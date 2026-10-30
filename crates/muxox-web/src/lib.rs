pub mod mcp;
mod resources;
mod ui;
mod web;
mod ws_proto;

#[cfg(test)]
mod ws_proto_tests;

pub use mcp::run_mcp_server;
pub use web::{empty_resource_slots, run_web_mode, spawn_resource_sampler};
