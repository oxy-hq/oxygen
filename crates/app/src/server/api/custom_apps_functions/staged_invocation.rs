//! Whether a function invocation is reading a branch — a custom-app staging
//! pin or a workspace preview — and so may not start production work (I9).
//!
//! `ctx.airway.run` seeds a real Airway run on the worker fleet: production's
//! lease, cursor and destination. No staging or preview path invokes a function
//! today; this keeps it so if one ever does. Two signals, either enough:
//!
//! * the task-local staging pin (`custom_apps_staging_pin::with_staging_pin`,
//!   which a preview request is served under too), captured when the host is
//!   built — host calls run on the isolate thread, where the task-local is not
//!   in scope;
//! * the project context itself being built at a `staging` revision, which is
//!   how a pinned invocation reads the branch at all.
//!
//! A failed lookup refuses: this guards a write, so "could not tell" is no.
//!
//! What a branch-reading run may do is `env_policy::EnvPolicy::decide_on_branch`;
//! this only says whether the run reads one, and why.

use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

/// Why this invocation is a staging or preview one, or `None` for a live one.
/// `captured_pin` is the run's `EnvPolicy::semantic_pin` — the build's pin, or
/// one already in scope where the host was built.
pub(super) async fn staged_reason(
    db: &DatabaseConnection,
    captured_pin: Option<Uuid>,
    context_revision: Option<Uuid>,
) -> Option<String> {
    let pin =
        captured_pin.or_else(crate::server::api::custom_apps_staging_pin::current_staging_pin);
    if let Some(pin) = pin {
        return Some(format!("it is pinned to staging revision {pin}"));
    }
    let revision = context_revision?;
    match entity::revisions::Entity::find_by_id(revision)
        .one(db)
        .await
    {
        Ok(Some(rev)) if rev.kind == "staging" => {
            Some(format!("it reads staging revision {revision}"))
        }
        Ok(_) => None,
        Err(e) => Some(format!(
            "whether revision {revision} is a staging revision could not be checked: {e}"
        )),
    }
}
