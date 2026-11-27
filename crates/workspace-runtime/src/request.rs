//! The workspace request: what `/api/{workspace_id}/…` runs before a handler.
//!
//! Down from `oxy-app` on 2026-11-22 (decompose W7), and re-exported there at
//! the old paths (`server::api::middlewares::{workspace_context,
//! workspace_extractor}`, `server::factory_access`, `server::previews::read_only`):
//!
//! * [`workspace_context`] — the workspace middleware: it authorizes the
//!   caller, pins the revision the request reads, and builds the
//!   `OxyProjectContext` the handler is handed.
//! * [`workspace_extractor`] — the workspace row it leaves behind, as a handler
//!   takes it.
//! * [`factory_access`] — the Factory's route lists and the gate the
//!   middleware applies to them. Always enforced.
//! * [`preview_read_only`] — the axum adapter of the preview read-only guard
//!   the middleware calls on a request pinned to a preview; the table is
//!   `oxy_previews::read_only`.
//! * `preview_pin` — the preview pin as the middleware applies it, with the
//!   staff check it hands `oxy_previews::pin::for_request`.

pub mod factory_access;
mod preview_pin;
pub mod preview_read_only;
pub mod workspace_context;
pub mod workspace_extractor;
