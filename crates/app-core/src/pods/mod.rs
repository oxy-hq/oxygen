//! Pod placement: which pod may answer a request, and the relay to the one
//! that may.
//!
//! - [`role_manifest`]: the process role and the route declarations the router
//!   installs, which `classify` reads.
//! - [`role_router`]: the router where a route's pod is a type
//!   ([`IdeState`] / [`FleetState`]), whose declarations feed `role_manifest`.
//! - [`role_middleware`]: the layer that refuses (421) or relays an `IdeOnly`
//!   route on a pod with no working copy.
//! - [`ide_proxy`]: the relay itself, to the Factory (`OXY_IDE_UPSTREAM`).
//! - [`servability`]: whether a pod with no working copy can serve a workspace.
//!
//! Moved down from `oxy-app`'s `server::{role_manifest, role_middleware,
//! ide_proxy, servability}` and `server::router::role_router` (decompose G3),
//! and re-exported at those paths. The test installer that builds the real
//! router to read its declarations stays in `oxy-app` beside the router.

use crate::AppState;

pub mod ide_proxy;
pub mod role_manifest;
pub mod role_middleware;
pub mod role_router;
pub mod servability;

/// The two halves of the split. A route's role is a permission — `IdeState`
/// means "this pod has a working copy, so a handler here may reach for one" —
/// and the compiler enforces it: `WorkspaceManagerWorkingCopy` resolves only from
/// `IdeState`, so a handler that asks for a working copy cannot be mounted on a
/// fleet route. `AppState` comes from either, so `State<AppState>` handlers are
/// unaffected.
#[derive(Clone)]
pub struct IdeState(pub AppState);

#[derive(Clone)]
pub struct FleetState(pub AppState);

impl axum::extract::FromRef<IdeState> for AppState {
    fn from_ref(state: &IdeState) -> Self {
        state.0.clone()
    }
}

impl axum::extract::FromRef<FleetState> for AppState {
    fn from_ref(state: &FleetState) -> Self {
        state.0.clone()
    }
}
