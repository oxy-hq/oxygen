//! What `oxy-server`'s long-lived commands (`serve`, `start`, `worker`) take
//! from `oxy-app`: the migrations they run first and the build identity they
//! report. The lower-crate items they reached through here (`http_cache`,
//! `worker::PreaggCacheCtx`, `bundle_cache_budget`) went in decompose RPT
//! (2026-11-24), when `oxy-server` imported the crates that hold them.
//!
//! The counterpart of [`crate::surface`]. Like it, this module holds no code,
//! only re-exports, which is why `tests/routing/app_growth.rs` admits it to
//! the closed top level: it adds no context and shrinks as `oxy-app` does. That module is what an HTTP surface
//! crate (`api-*`) may import; this one is what the binary that hosts the
//! process may. The commands moved up out of `oxy-app`
//! (`internal-docs/domain-boundaries.md`, "End state: no `oxy-app`", step b),
//! and everything they still need from here is listed below, so this file is
//! also the inventory of what the base crate (step c) has to carry.
//!
//! Grouped by the module each item lives in, so a call site reads
//! `host::migrations::run_database_migrations()` and still says where the item
//! is defined.

// The database migrations (`migrations`) went up with `oxy migrate` into
// `oxy-server`'s `cli::commands::migrate` in decompose CLI (2026-11-25), where
// `serve` and `worker` call them directly.

// The fleet role and the route table's declarations (`role_manifest`), the
// boot refusal over the auth provider (`auth_provider_check`), the router
// `serve` mounts with its seams (`router`) and the layers `serve` wraps it in
// (`middleware`) are `oxy-server`'s own since decompose LIFT (2026-11-23): the
// router assembly went up beside the commands that mount it.

// The build identity (`build_identity`: the image tag and the build information
// the build script stamps), with that build script, went up into `oxy-server`
// in decompose BID (2026-11-25): `oxy_server::build_identity`, beside the
// health probes, route discovery and `oxy migrate`'s report that read it. This
// module re-exports nothing any more; decompose DEL deletes it with the crate.
