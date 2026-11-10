//! Slack integration tests — OAuth install/state, workspace resolution, user
//! links, uninstall, webhook events, the app-home surface, and a
//! notification's DM finding its person.
//!
//! One binary for the whole domain; see `tests/authz/main.rs` for why. Add a
//! case as a `mod` here rather than a new `tests/*.rs`.
//!
//! This binary is MIXED. `slack_app_home` and `slack_webhook_events` are
//! in-process and run fully parallel; the other seven call `establish_connection()`
//! against the raw shared `OXY_DATABASE_URL` with no per-test database, and are
//! pinned into `serial-db` by `.config/nextest.toml`. They skip when the var is
//! unset, so only CI would ever surface the race. `authz::shared_db_registry`
//! fails the build if that list drifts.

mod slack_app_home;
mod slack_cross_org_collision;
mod slack_oauth_install;
mod slack_oauth_state;
mod slack_resolution;
mod slack_uninstall;
mod slack_user_links;
mod slack_webhook_events;
