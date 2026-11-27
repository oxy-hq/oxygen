//! The audit row of the one pointer move no request makes: the promote checks'
//! automatic rollback of production (`act::rollback_unattended`).
//!
//! Every other move's row is built from the request's actor
//! (`AuditEntry::for_request`, in `audit`), so a key or token that moved an
//! environment is named on it. This move runs on the worker fleet for nobody,
//! so its row names the system, and it is kept in this file on its own, the
//! only one here `call_sites_guard` allows a bare `AuditEntry::new`.

use entity::apps;
use oxy_app_core::audit::{ActorType, AuditEntry};
use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::{Value, json};
use uuid::Uuid;

/// Who the row names.
pub const ACTOR: &str = "system:promote-checks";

/// A move the platform makes itself, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Unattended {
    /// The audit row's action (`app.auto_rolled_back`).
    pub action: &'static str,
    /// What the row says beside `{from, build, previous}`: an object naming
    /// what the move answered (the failed check, its attempt).
    pub because: Value,
}

/// The row of an unattended move. `moved` is `(app, environment)`, `builds`
/// is `(build, previous)` as every move's row has them; `broken` is what a
/// rollback broke.
pub fn entry(
    why: &Unattended,
    moved: (&apps::Model, &AppEnvironment),
    builds: (Uuid, Option<Uuid>),
    broken: &[String],
) -> AuditEntry {
    let (app, environment) = moved;
    let mut metadata = json!({ "from": null, "build": builds.0, "previous": builds.1 });
    if let (Some(metadata), Some(because)) = (metadata.as_object_mut(), why.because.as_object()) {
        for (key, value) in because {
            metadata.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if !broken.is_empty() {
        metadata["broken_listeners"] = json!(broken);
    }
    let mut entry = AuditEntry::new(ACTOR, why.action);
    entry.actor_type = ActorType::System;
    entry
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_environment",
            format!("{}/{environment}", app.id),
            format!("{}/{environment}", app.slug),
        )
        .environment(environment.name())
        .metadata(metadata)
}
