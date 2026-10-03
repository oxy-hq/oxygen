//! Org **teams**, and the app-access control plane they feed.
//!
//! The enforcement engine for restricted apps shipped in m20260722 — `apps.visibility`,
//! `app_members`, `Ring::AppAccess`, `Ring::AppAdmin` — but no control surface came
//! with it, so nothing in the product could ever write those rows. This module is
//! the missing half, built around the unit an org admin actually thinks in: a named
//! team, not a per-app list of people.
//!
//! - [`service`], [`audit`], [`dto`] — the behavior, the audit rows and the wire
//!   types, which live in the `oxy-tenancy` domain crate (custom apps and
//!   frontline use them too). `service` here adds only the cache-flushing
//!   `write_access` wrapper.
//! - [`handlers`] — the org's team roster (`/orgs/{id}/teams/*`).
//! - [`app_access`] — one app's visibility + grants
//!   (`/orgs/{id}/apps/{id}/access`).
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
pub mod handlers;
pub mod service;

pub use oxy_tenancy::org_teams::{audit, dto};
