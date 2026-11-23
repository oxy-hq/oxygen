//! Route-role (pod placement) tests for the surface crates, one module per
//! surface.
//!
//! Each installs the real router's declarations beside the surface's own
//! (`oxy_server::role_manifest::install_route_declarations_for_tests_with`), then
//! asks the same `classify` the request path asks, so a surface's rows are
//! proven against the router's patterns. They lived in each surface's own test
//! group until decompose TR (2026-11-23), which moved them here: this crate
//! already depends on `oxy-app` and on every surface, so five surfaces
//! (airway, documents, frontline, operating-graph, tenancy) could drop their
//! `oxy-app` dev-dependency.
//!
//! The declaration registry is a process-wide once-cell. nextest runs each test
//! in its own process, so each test installs its own set. None of these opens a
//! database, so `.config/nextest.toml` carves `route_roles::` out of this
//! binary's `db-per-test` group, as the surfaces' own overrides did.

mod airway;
mod custom_apps;
mod documents;
mod experiment;
mod frontline;
mod operating_graph;
mod tenancy_admin;
mod tenancy_onboarding;
mod tenancy_partner_console;
