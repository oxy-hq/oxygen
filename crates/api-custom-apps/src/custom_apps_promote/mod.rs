//! `oxyc promote` and `oxyc rollback` for custom apps, and the one function
//! that moves production's pointer for every writer
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §5.5a, the
//! Phase 3 rulings; the living contract is
//! `internal-docs/custom-app-sandboxes.md` §5–§6).
//!
//! **Two promote paths, nothing else:** a sandbox's build to staging (a
//! pointer move whose `staging / promote` event is what later admits the build
//! to production), and staging's to production. Production only ever serves a
//! build staging has served (invariant 1), checked inside the transaction that
//! moves production's pointer, under a row lock on `apps` — and every writer
//! of that pointer goes through [`production::move_production`] (or, for a
//! promoting publish that already holds the lock, [`production::move_locked`]):
//! these routes, the console's *Make Live* and rollback, *promote latest*,
//! the partner console's promote and batch publish. Each applies the target
//! build's pending migrations to production before the pointer moves.
//!
//! | Module | Owns |
//! | --- | --- |
//! | [`resolve`] | the request bodies; which environment moves, to which build, from what |
//! | [`plan`] | what a dry run shows, and the act answers |
//! | [`blockers`] | which facts refuse the act (pure) |
//! | [`second_person`] | the four-eyes rule: who counts as the build's publisher |
//! | [`functions_diff`], [`schedules`] | what changes for functions and production's schedules |
//! | [`production`] | the one production-pointer function |
//! | [`target`] | which build a production move is aimed at, named under the row lock |
//! | [`unpublish`] | taking production's build away, under the same lock |
//! | [`staging`] | staging's pointer moves, and their queued migrations |
//! | [`act`] | plan → refuse → move → audit |
//! | [`audit`] | the audit rows of a production move, shared by the single and the batch handlers |
//! | [`handlers`] | the two routes |
//! | [`error`] | every refusal's status and code |

pub mod act;
pub mod audit;
pub mod blockers;
pub mod error;
pub mod functions_diff;
pub mod handlers;
pub mod plan;
pub mod production;
pub mod resolve;
pub mod schedules;
pub mod second_person;
pub mod staging;
pub mod target;
pub mod unpublish;

pub use audit::{AuditedBy, MoveAudit, MoveRow};
pub use error::{Blocker, PromoteError};
pub use production::{
    MigrationsApplied, ProductionMove, move_locked, move_production, move_production_audited,
};
pub use target::BuildRef;
