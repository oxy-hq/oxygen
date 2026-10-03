//! Org teams and the app-access control plane they feed — the behavior, not
//! the routes.
//!
//! Three contexts edit or read this data: tenancy (the org's own settings and
//! the partner console), custom apps (the Oxy admin panel) and frontline
//! (enrolling a worker with app grants). Each brings its own authority gate and
//! a thin handler; the shared behavior lives here, below all of them, so none
//! has to reach into another's surface for it.
//!
//! - [`service`] — read/write access, list teams. Nothing here decides access:
//!   callers gate first, then call.
//! - [`audit`] — the append-only rows every write leaves in the org's log.
//! - [`dto`] — the wire types, notably the `kind: "user" | "team"` grant union.

pub mod audit;
pub mod dto;
pub mod service;
