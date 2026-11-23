//! `oxy-route-catalog`'s tests that need the real router, up from that crate's
//! unit tests in decompose RT (2026-11-23). The generated table stays there, with
//! the tests that read it against lower crates; these read it against what only
//! `oxy-app`'s router holds — its role declarations (`factory_routes`, `describe`) and the
//! token-grant middleware's tables (`token_grant_scope`) — and `oxy-server` is
//! the one crate that depends on both. They open no database, so
//! `.config/nextest.toml` carves `route_catalog::` out of this binary's
//! `db-per-test` group.

mod describe;
mod factory_routes;
mod token_grant_scope;
