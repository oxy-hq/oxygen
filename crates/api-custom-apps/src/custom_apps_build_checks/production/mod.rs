//! Production's own check of a build, and what follows when it fails.
//!
//! **A production attempt.** Where the app's org has the `promote_checks_gate`
//! flag on and the app's `production_smoke` setting is on, every move of
//! production's pointer to another build queues an attempt of that build's
//! checks in the `production` environment (`super::queue::after_production_move`),
//! with one kind: `production_smoke`, "Production opens after promote". The
//! same runner drives it through the same routes, naming `"environment":
//! "production"`; its claim answers what production showed before the move
//! ([`baseline`]). It is never a gate: the build is already live.
//!
//! **A failed `production_smoke`** (the runner found an error a screen did not
//! have before the move, or a screen that was ready and is not now) queues
//! ONE durable task for that attempt ([`failed`], run by [`followup`] on the
//! worker fleet), which does two things, in order:
//!
//! 1. **Rolls production back**, when the app's `auto_rollback` setting is on
//!    ([`rollback`]): to the build production served before, through the
//!    existing rollback path, audited `app.auto_rolled_back`. At most once per
//!    production attempt, only while production still serves the failed
//!    build, never when that build reached production by a rollback itself,
//!    so two bad builds cannot roll each other back in a loop, and never when
//!    the attempt had no baseline from before the move that tells new errors
//!    apart ([`baseline::tells_apart_for`], read by the platform whatever the
//!    runner reported, which with none reports `skipped`).
//! 2. **Tells people** ([`followup::notify`]): the platform notice
//!    `oxy.production_check_failed` (push, email, Slack) to the person who
//!    moved production and the org's owners and admins. It names the app, the
//!    build, the build before it and the new errors (with no baseline, the
//!    errors on production, never called new), and says whether production
//!    was rolled back.
//!
//! **What the task leaves on the check's row** (`detail`, beside the runner's
//! evidence): the move it checked and what the rollback did. The notice is
//! worded from that row alone when it is read or delivered
//! (`oxy-api-notifications` `platform::production_check`), and a report can
//! never set these keys (`runner::report::PLATFORM_KEYS`):
//!
//! ```json
//! "move": { "action": "promote", "by": "<user id>|null", "at": "<time>",
//!           "previous": { "id": "<row id>", "build_id": "<publish id>" } | null,
//!           "baseline": true | false },
//! "rollback": { "rolled_back": true,  "to": { "id": "…", "build_id": "…" }, "at": "<time>" }
//!           | { "rolled_back": false, "reason": "<why>", "at": "<time>" }
//! ```
//!
//! `rollback` is absent where the app does not roll back automatically.

pub mod baseline;
pub mod failed;
pub mod followup;
pub mod moved;
pub mod rollback;

pub use failed::PRODUCTION_FAILED_KIND;
