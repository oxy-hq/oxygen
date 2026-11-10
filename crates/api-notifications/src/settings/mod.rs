//! Notification settings: what a person chose, at four levels, and the one
//! function that turns it into "does this go out" ([`decide::decide`]).
//!
//! 1. **The person** — pause, quiet hours, the timezone they are read in.
//! 2. **Each node of a source's tree** — source, category path, kind: per
//!    channel on, off or inherit, and a level floor. The nearest explicit
//!    setting wins, so a kind can be on under a category that is off.
//! 3. **Each device** — enabled, its own floor, and the OS state the app
//!    reports (`super::devices`).
//! 4. **Critical** overrides the first two, never a device that is off or
//!    blocked by its OS.
//!
//! `GET`/`PATCH /api/notifications/settings` ([`handlers`]) read and write
//! the whole document ([`document`], [`patch`]); delivery reads the same
//! storage ([`store`]) and records [`decide::decide`]'s reason. Design:
//! `internal-docs/custom-app-notifications.md`, "Settings".

pub mod decide;
pub mod document;
pub mod handlers;
pub mod model;
pub mod patch;
mod patch_nodes;
pub mod quiet;
pub mod reconcile;
pub mod store;
pub mod summary;
pub mod summary_task;
