//! `oxy-server`'s integration tests — one binary, per the workspace rule (a new
//! top-level `tests/*.rs` is another full link). Add a case as a `mod`.
//!
//! They live in the composition root because they drive the API **as served**:
//! `oxy-app`'s routes together with the extracted surface crates (`/orgs` and the
//! rest of tenancy is `oxy-api-tenancy`'s), behind the one `/api` auth stack.
//! Only this crate depends on both — which is also why the notification tests
//! that span the custom-app and notifications surfaces live here, and billing's
//! past-due notice, which spans billing's and notifications'. So are the
//! surfaces' route-role tests (`route_roles`), which install the real router's
//! declarations beside each surface's own.
//!
//! `token_auth` owns a database cloned from the per-run template, using
//! `oxy-app`'s harness included by path rather than copied (it owns the
//! template database and the stray-database sweep), so `.config/nextest.toml`
//! puts this binary in `db-per-test`.

#[path = "../../../app/tests/common/mod.rs"]
mod common;
#[path = "../../../api-notifications/tests/integration/platform_notices/fixture.rs"]
mod notices_fixture;
#[path = "../../../api-notifications/tests/integration/notification_categories/fixture.rs"]
mod notification_fixture;
#[path = "../../../app/tests/common/oltp_fx.rs"]
mod oltp_fx;

// Notifications driven through a custom app: the console's "Test notifications"
// (`oxy-api-custom-apps`, reading the catalogue through the port the
// notifications surface implements) and a notification's function action (the
// notifications surface running an app's function). Each needs both surfaces,
// and a surface may not depend on another, so they are the composition root's.
mod console_test_notification;
// Builds on the demo seed (`oxy_server::dev::seed`), compiled only with
// `dev-tools` (on by default) since decompose SD.
#[cfg(feature = "dev-tools")]
mod notification_function_action;
// Billing's past-due notice (decompose B2): `oxy-api-billing` sends it, and
// the notifications surface delivers it, honours (or here, refuses) a mute and
// renders it in the inbox.
mod billing_past_due_notice;
// `oxy-route-catalog`'s tests that build the real router (decompose RT): no
// database, carved out of `db-per-test` in `.config/nextest.toml`.
mod route_catalog;
// The surfaces' route-role tests (decompose TR): the real router's declarations
// beside each surface's, which only a crate depending on both can install.
mod route_roles;
// `oxy-app`'s routing tests that build the real router, up with it (decompose
// LIFT): no database, carved out of `db-per-test` like the two above.
mod routing;
// Local mode's router (decompose LIFT, from `oxy-app`'s `platform` group): it
// builds `api_router(..)`, the unscoped `cleanup_stale_runs` writer, so it is
// pinned into `serial-db` and out of `db-per-test`.
mod local_mode_router;
mod token_auth;
