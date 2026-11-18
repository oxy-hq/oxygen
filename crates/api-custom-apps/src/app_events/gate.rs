//! The gate on shipping: a build that removes an event type, or changes its
//! schema incompatibly, must not go live while a function in production
//! listens to it. Apps ship independently, so the platform is the only place
//! that sees both sides.
//!
//! [`check`] compares the build about to ship with what production serves now
//! (the revision [`schema::live_revision`] finds) and finds the listeners
//! ([`subscriptions::listeners`]) of every event type the app publishes:
//!
//! * a listener **in production** is a refusal of a ship ([`GateScope::Ship`]);
//! * a listener in any other environment (staging, a sandbox) only warns;
//! * a [`GateScope::Rollback`] is never refused: the emergency path turns every
//!   refusal into a warning.
//!
//! The report lists the refusals even when the request passed
//! `--break-listeners`: the caller decides (`break_listeners` is its to read),
//! and when it overrides it writes the audit row ([`audit_override`]) naming
//! each listener it broke. Listeners of the publisher's own app are left out:
//! they ship in the same build.
//!
//! **Inert while the org's `app_events` flag is off:** no event reaches a
//! listener then, so there is nothing to break, and the report is empty — no
//! refusal, no warning. The subscriptions it reads still reconcile, so the
//! gate holds from the first ship after the flag goes on.

use std::collections::{BTreeSet, HashMap};

use entity::{app_builds, app_event_subscriptions as subs, apps};
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_custom_apps::events::catalog::EventCatalog;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::schema;
use super::subscriptions::{self, Listener};

/// The audit action of a ship that broke listeners on purpose.
pub const BROKE_LISTENERS: &str = "app.event_listeners_broken";

const PRODUCTION: &str = "production";

/// What is being done with the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateScope {
    /// A publish with promote, or a promote, to production.
    Ship { break_listeners: bool },
    /// Production moving back: never refused.
    Rollback,
}

/// What the gate found. `refusals` stop a ship that did not pass
/// `--break-listeners`; `warnings` never stop anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GateReport {
    pub refusals: Vec<String>,
    pub warnings: Vec<String>,
}

/// How a listener's event type breaks in the target build.
enum Break {
    Removed,
    Incompatible(Vec<String>),
}

pub async fn check(
    db: &DatabaseConnection,
    app: &apps::Model,
    target: &app_builds::Model,
    scope: GateScope,
) -> Result<GateReport, DbErr> {
    if !super::enabled_for(app.org_id) {
        return Ok(GateReport::default());
    }
    let declared = EventCatalog::from_manifest(target.manifest_json.as_ref())
        .map(|c| c.events)
        .unwrap_or_default();
    let mut report = GateReport::default();
    // Each (old revision, event) pair compared once per run, however many
    // environments listen to it: the comparison is bounded, not free.
    let mut compared: HashMap<(Uuid, String), Vec<String>> = HashMap::new();
    for (event_type, environments) in listened(db, app).await? {
        let name = event_type
            .strip_prefix(&format!("{}/", app.slug))
            .unwrap_or(&event_type);
        let in_production = schema::live_revision(db, app.id, PRODUCTION, name).await?;
        for environment in environments {
            // What the platform vouches for is production's revision; an
            // event production never declared is judged by the listener's own
            // environment.
            let old = match &in_production {
                Some(revision) => Some(revision.clone()),
                None => schema::live_revision(db, app.id, &environment, name).await?,
            };
            let Some(old) = old else { continue };
            let broken = match declared.get(name) {
                None => Break::Removed,
                Some(decl) => {
                    let changes = compared
                        .entry((old.id, name.to_string()))
                        .or_insert_with(|| schema::incompatibilities(&old.schema, &decl.schema));
                    if changes.is_empty() {
                        continue;
                    }
                    Break::Incompatible(changes.clone())
                }
            };
            let listeners =
                subscriptions::listeners(db, app.org_id, &environment, &event_type).await?;
            for listener in listeners.iter().filter(|l| l.app_id != app.id) {
                let text = message(&event_type, &broken, listener, &environment);
                if environment == PRODUCTION && matches!(scope, GateScope::Ship { .. }) {
                    report.refusals.push(text);
                } else {
                    report.warnings.push(text);
                }
            }
        }
    }
    Ok(report)
}

/// The app's published event types somebody listens to, each with the
/// environments the listeners are in, in a stable order.
async fn listened(
    db: &DatabaseConnection,
    app: &apps::Model,
) -> Result<Vec<(String, BTreeSet<String>)>, DbErr> {
    let prefix = format!("{}/", app.slug);
    let rows = subs::Entity::find()
        .filter(subs::Column::OrgId.eq(app.org_id))
        .filter(subs::Column::EventType.starts_with(&prefix))
        .all(db)
        .await?;
    let mut grouped: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
    for row in rows.into_iter().filter(|r| r.app_id != app.id) {
        grouped
            .entry(row.event_type)
            .or_default()
            .insert(row.environment);
    }
    Ok(grouped.into_iter().collect())
}

fn message(event_type: &str, broken: &Break, listener: &Listener, environment: &str) -> String {
    let who = format!("{}/{}", listener.app_slug, listener.function_name);
    match broken {
        Break::Removed => format!(
            "`{event_type}` is removed, but {who} listens to it in {environment}. Ship a build \
             that keeps it, or pass --break-listeners."
        ),
        Break::Incompatible(changes) => format!(
            "`{event_type}` changes incompatibly ({}), and {who} listens to it in {environment}.",
            changes.join(", ")
        ),
    }
}

/// The audit row of a ship that went ahead over `broken` listeners
/// (`--break-listeners`): who, which app and build, and the refusals it
/// overrode. Best effort, as every row written after the move it records.
pub async fn audit_override(
    db: &DatabaseConnection,
    actor: &RequestActor,
    app: &apps::Model,
    build: &str,
    broken: &[String],
) {
    let entry: AuditEntry = AuditEntry::for_request(actor, BROKE_LISTENERS)
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_environment",
            format!("{}/{PRODUCTION}", app.id),
            format!("{}/{PRODUCTION}", app.slug),
        )
        .environment(PRODUCTION)
        .metadata(json!({ "build": build, "broken": Value::from(broken.to_vec()) }));
    audit::record_best_effort(db, entry).await;
}
