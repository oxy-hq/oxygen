//! The audit rows of a production move, built in one place so the single
//! handlers and the batch handlers cannot write different ones.
//!
//! Three rows, each named by what the person did:
//!
//! | Action | Written for | Target |
//! | --- | --- | --- |
//! | `app.published` | the console's *Make Live* | the app |
//! | `app.unpublished` | the console's *Unpublish* | the app |
//! | `app.deleted` | the console's *Delete* (one or many) | the app, which no longer exists |
//! | `app.promoted` / `app.rolled_back` | `oxyc promote` / `oxyc rollback`, the console's rollback and promote latest | the app's environment, with `{from, build, previous}` |
//!
//! A single handler records its row after its move has committed
//! ([`audit::record_best_effort`]): the move stands either way. A batch
//! handler asks the move to record the row itself ([`AuditedBy::Move`]), in
//! the transaction holding the app's row lock, so a move that is refused or
//! rolled back leaves no row, and a row that cannot be written leaves no move.

use entity::apps;
use oxy_app_core::audit::{AuditEntry, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::blockers::Act;

/// Who records the audit row of a move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditedBy {
    /// The move, in its own transaction.
    Move,
    /// The caller: it writes its own row, after the move (the single
    /// handlers), or a different one (the partner console's `partner.app.*`).
    Caller,
}

/// Which row a production move writes in its own transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveRow {
    /// `app.published`: the console's *Make Live*.
    Published,
    /// `app.promoted`: production moved to a build, as `oxyc promote` does.
    Promoted,
}

/// The row a production move writes in its own transaction, and for whom.
#[derive(Debug, Clone, Copy)]
pub struct MoveAudit<'a> {
    pub actor: &'a RequestActor,
    pub row: MoveRow,
}

impl<'a> MoveAudit<'a> {
    /// `Some` when the move is to record `row` ([`AuditedBy::Move`]).
    pub fn of(by: AuditedBy, actor: &'a RequestActor, row: MoveRow) -> Option<Self> {
        (by == AuditedBy::Move).then_some(Self { actor, row })
    }
}

/// The row of one pointer move by promote or rollback. `moved` is
/// `(app, environment)`, `builds` is `(build, previous)`.
pub fn promoted_entry(
    actor: &RequestActor,
    act: Act,
    moved: (&apps::Model, &AppEnvironment),
    from: Option<String>,
    builds: (Uuid, Option<Uuid>),
    broken: &[String],
) -> AuditEntry {
    let (app, environment) = moved;
    let action = match act {
        Act::Promote => "app.promoted",
        Act::Rollback => "app.rolled_back",
    };
    AuditEntry::for_request(actor, action)
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_environment",
            format!("{}/{environment}", app.id),
            format!("{}/{environment}", app.slug),
        )
        .environment(environment.name())
        .metadata({
            let mut metadata = serde_json::json!({
                "from": from,
                "build": builds.0,
                "previous": builds.1,
            });
            if !broken.is_empty() {
                metadata["broken_listeners"] = serde_json::json!(broken);
            }
            metadata
        })
}

/// The row of a Make Live. Publishing puts an app in front of a customer's
/// users; who did it is the question an incident asks first.
pub fn published_entry(actor: &RequestActor, app: &apps::Model) -> AuditEntry {
    AuditEntry::for_request(actor, "app.published")
        .org(app.org_id)
        .target("app", app.id.to_string(), app.name.clone())
}

/// The row of an unpublish: taking a customer's app down is at least as
/// auditable as putting it up.
pub fn unpublished_entry(actor: &RequestActor, app: &apps::Model) -> AuditEntry {
    AuditEntry::for_request(actor, "app.unpublished")
        .org(app.org_id)
        .target("app", app.id.to_string(), app.name.clone())
}

/// The row of an app's deletion: the app, its builds and its usage facts are
/// gone afterwards, so the row is what says it existed and who removed it. Its
/// slug and workspace go in the metadata because the target no longer resolves.
pub fn deleted_entry(actor: &RequestActor, app: &apps::Model) -> AuditEntry {
    AuditEntry::for_request(actor, "app.deleted")
        .org(app.org_id)
        .workspace(app.project_id)
        .target("app", app.id.to_string(), app.name.clone())
        .metadata(serde_json::json!({ "slug": app.slug }))
}
