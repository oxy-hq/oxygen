//! The staff console's share of the previews surface: the compile boundary's
//! operator face (`/api/admin/compiles/*`), out of `oxy-app`'s
//! `api::admin::compiles` in the admin split (PR 6).
//!
//! It is an [`AdminNest`], not an `AdminSection`: its guards answer every
//! path it mounts, a wrong method included, as they did when `oxy-app` nested
//! it itself (a path the tree does not match is the outer router's 404).
//! `oxy-app` applies them; this crate only names the capability.

pub mod compiles;

use oxy_app::surface::roles::RouteRole;
use oxy_app::surface::{Action, AdminNest};

/// The compile operator surface, nested at `/api/admin/compiles` behind the
/// staff door, the refusal while acting and `PlatformOperate` — compile history
/// is platform machinery. FleetOk: every handler reads or writes Postgres rows
/// (the revisions tables and the task queue); a working-copy compile is queued
/// only where this process owns one.
pub fn admin_nests() -> Vec<AdminNest> {
    vec![AdminNest {
        prefix: "/compiles",
        capability: Action::PlatformOperate,
        routes: compiles::router(),
        role: RouteRole::FleetOk,
        why: "the compile operator surface reads the revisions tables",
    }]
}
