//! Slack Socket Mode: persistent WebSocket connection for event delivery.
//! Opt-in via OXY_SLACK_APP_LEVEL_TOKEN. See internal-docs/2026-04-21-universal-slack-bot-design.md.
mod client;
mod envelope;

pub use client::run_socket_loop;
