//! The middlewares only this router layers (`protected`, `entry`), up from
//! `oxy-app`'s `server::api::middlewares` with the router assembly (decompose
//! LIFT, 2026-11-23). The request guards and the workspace middleware they sit
//! beside are lower crates' (`oxy_app_core::guards`,
//! `oxy_workspace_runtime::request`).

pub mod api_key_query;
pub mod app_grant_scope;
pub mod app_publish_token_scope;
pub mod local_context;
pub mod request_id;
pub mod timeout;
pub mod token_grant_scope;
pub mod token_usage;
