//! Org **teams**, and the app-access control plane they feed.
//!
//! The enforcement engine for restricted apps shipped in m20260722 — `apps.visibility`,
//! `app_members`, `Ring::AppAccess`, `Ring::AppAdmin` — but no control surface came
//! with it, so nothing in the product could ever write those rows. This module is
//! the missing half, built around the unit an org admin actually thinks in: a named
//! team, not a per-app list of people.
//!
//! - [`service`] — the surface-independent behavior (read/write access, list teams).
//! - [`audit`] — the append-only rows every write here leaves in the org's log.
//! - [`handlers`] — the org's team roster (`/orgs/{id}/teams/*`).
//! - [`app_access`] — one app's visibility + grants
//!   (`/orgs/{id}/apps/{id}/access`).
//! - [`dto`] — the wire types, notably the `kind: "user" | "team"` grant union.
//!
//! Everything here is gated by `Action::AppAccessManage`: an org officer, Oxy staff,
//! or a `manage_apps` partner. All routes are `FleetOk` — pure Postgres, no
//! filesystem, no git.
//!
//! Two OTHER surfaces edit the same data through [`service`] with their own gates,
//! because they cannot use these routes: `/admin/*` is closed while an operator
//! holds an assume-role session (and org routes require one), and the partner
//! console is capability-scoped rather than membership-scoped. See
//! `admin::apps::access` and `partner_console::app_access`.

pub mod app_access;
// `pub` for the extracted `oxy-api-frontline` crate: enrolling a worker with
// app grants records the same `app.access_changed` event these routes do.
pub mod audit;
pub mod dto;
pub mod handlers;
pub mod service;
