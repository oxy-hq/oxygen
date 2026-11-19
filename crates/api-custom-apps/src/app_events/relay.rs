//! The relay (app events design §6.2): moves an org's outbox rows' routing
//! metadata into the platform and fans each event out to the functions
//! listening to it.
//!
//! **The body never leaves the org's store.** A claim reads each row's
//! metadata and its payload's size (`octet_length(data::text)`), never the
//! payload, and the relay never decodes one. The platform stores the event as
//! routing metadata, `data_bytes` and the `writer_role` that, with the id,
//! names the body in the org's outbox log; a listener's task carries the
//! envelope with `data: null`, and the delivery reads the body by that key
//! when it runs (`oxy_oltp::events::read_body`).
//!
//! **The order is the outbox guarantee.** [`relay_once`]:
//!
//! 1. claims up to [`BATCH`] waiting rows in the org's store (`FOR UPDATE
//!    SKIP LOCKED`, so two relays never hold the same row);
//! 2. reads each row: the app its writer role belongs to, whether its
//!    provenance MAC is the host's over its fields, the environment the store
//!    vouches for, the event's id derived again from those, and whether that
//!    environment's live build declares the event and the schema revision it
//!    was published against is one recorded for it;
//! 3. in **one platform transaction**, stores each event (`ON CONFLICT (id) DO
//!    NOTHING`) and, for one it stored without a relay error, inserts a
//!    delivery per listener, queuing an `app_function` task (trigger `event`)
//!    for each pending one;
//! 4. commits that transaction, and only then
//! 5. marks the rows relayed in the outbox (`relayed_at`), where they stay as
//!    the bodies' log. A row no stored event points at — its writer is no
//!    app, or the event is stored under another id or another writer's row —
//!    is deleted instead: nothing would ever read its body. Unless it was
//!    quarantined for its provenance: that body is kept.
//!
//! A crash between 4 and 5 relays the rows again, and step 3's conflict makes
//! that a no-op: an event already stored gets no second delivery.
//!
//! **The store decides an event's environment, never the row** (R12,
//! `relay::vouch`). Production's store holds production's events only: a row
//! there claiming any other environment is stored as production's with
//! `relay_error` set and is not delivered, so whether an event crosses from
//! non-production into production never rests on a column app SQL wrote. The
//! org's OLTP staging branch holds staging's events and those of each sandbox
//! of the publishing app on a schema there; a row claiming anything else is
//! stored as staging's with `relay_error` set. A non-production delivery whose
//! environment is not taking events is held (§6.7).
//!
//! **Nor a row the host did not sign** (R25(c), [`super::provenance`]). App
//! SQL can call `oxy_events.publish` itself, around every check the host
//! makes; the host signs each row it writes with the org's key, which never
//! enters the store. A row whose MAC is missing or does not match its fields
//! is stored with `relay_error` `unverified_provenance`, delivered to nobody,
//! and its body kept; each pass logs how many, never a body.
//!
//! **Nor the row's id.** App SQL can call `oxy_events.publish` directly, so an
//! event is stored under the id the relay derives itself (§6.1: org, the
//! environment above, the type of the writer's own app, the row's key). A row
//! whose own id differs is stored under the derived one with `relay_error` and
//! not delivered: an app can only ever take ids in its own namespace, never
//! another app's event before it is published.
//!
//! **Nor the payload's size.** A row whose payload is over
//! [`MAX_PAYLOAD_BYTES`](oxy_custom_apps::events::MAX_PAYLOAD_BYTES), measured
//! in the store as `jsonb` text, is stored with
//! `relay_error` and its size, and is never delivered. The host measures a
//! publish the same way
//! ([`jsonb_text_len`](oxy_custom_apps::events::jsonb_text_len)) and the
//! definer function refuses such a row by this very expression, so this is a
//! backstop for a row stored before that bound. The relay reads no body, so
//! no row, however large or however its numbers are written, can exhaust a
//! relay or stall the org's outbox.
//!
//! **A revision is any one recorded for the event**, not only the live one:
//! an event published just before a compatible schema change went live still
//! delivers (an incompatible change is refused at publish while anything
//! listens). The live build must still declare the event's name.
//!
//! **Wakes.** A nudge queues one [`RELAY_KIND`] task per (org, environment)
//! ([`enqueue_relay`]), and a pass that claimed a full batch queues the next
//! one itself, so a backlog drains without waiting for the sweep. Every
//! publish also marks its (org, environment) in the platform
//! ([`marks`](super::marks)), and a pass that claimed less than a full batch
//! marks it drained as of when it began claiming — unless, in its claim's own
//! snapshot, it left rows waiting (another pass holds them), when the mark
//! stays due and its wait (`pending_since`) follows the oldest row actually
//! waiting. The fleet sweep
//! ([`spawn_sweep`]) reads only those marks, from the platform database: it
//! queues a relay for each mark published since its last drain and left so
//! for [`SETTLE`] (the nudge's to handle until then), in an org with
//! `app_events` on, and logs `app_events_outbox_stale` for one whose work has
//! waited longer than [`STALE_AFTER`] (its `pending_since`, which neither a
//! failed pass's reopen nor a publish into a waiting outbox moves). It never connects to an org's store, so a
//! scale-to-zero store with nothing waiting is left asleep, and an org whose
//! app SQL wrote a row without a publish is relayed by its next publish.
//!
//! **Every environment's store is relayed** (Task 17): production's, the
//! staging branch for staging and the schema sandboxes, and each sandbox's own
//! OLTP branch (its planes' `oltp` is `branch` or `empty`): a pass for a
//! sandbox claims from the staging branch and from every own branch of that
//! name in the org (`app_events::stores`). A store the org does not have (no
//! staging branch; a sandbox's branch gone with its teardown) holds nothing,
//! so a pass that finds none drains its mark: it never retries a store that
//! is gone. One that fails (an own branch whose credential is unreadable, say)
//! holds up no other: the pass relays every other store, then fails naming
//! each that did, and leaves the mark due, never settled, so the sweep
//! retries. A pass for a name that is no environment does nothing and
//! succeeds.
//!
//! **Inert while the org's `app_events` flag is off:** a pass relays nothing
//! and touches no store, and the sweep passes the org by.
//!
//! **Connections.** A pass holds one owner connection to the org's store (the
//! claim's) and reads everything else from the platform database, once per
//! writer role, event and listener rather than once per row.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use agentic_core::delegation::{
    BackoffStrategy, RetryPolicy, TaskAssignment, TaskOutcome, TaskPolicy, TaskSpec,
};
use agentic_pipeline::app_function_task::AppFunctionTask;
use agentic_pipeline::scheduler::{ScheduleError, enqueue_app_function_task_at};
use agentic_runtime::crud::{TaskScope, enqueue_task, insert_run};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::Utc;
use entity::{app_environments, app_event_deliveries, app_events, apps};
use futures::FutureExt;
use oxy_app_core::custom_app_environment::AppEnvironmentKind;
use oxy_custom_apps::calls::SendTarget;
use oxy_custom_apps::events::{MAX_DELAY_SECS, OxyEvent};
use oxy_oltp::entity::roles::{self as oltp_roles, WriterKind};
use oxy_oltp::events::{OutboxKey, OutboxRow, OutboxTarget};
use oxy_oltp::schema::app_writer_name;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait, QueryFilter, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::marks;
use super::stores::is_relayed;
use super::subscriptions::{Listener, listeners};
use super::table_watch;
use crate::custom_apps_functions::function_task_policy;
use crate::custom_apps_sandboxes::activity::is_past_idle_expiry;
use oxy_custom_apps::events::table::is_table_event_type;

// Which store an environment's events wait in, and what a store vouches for.
mod vouch;
use vouch::Vouch;
// A claimed row read as the event to store, and why it is refused.
mod read;
use read::{EventSchemas, read_row};
// One pass over every store of an environment's outbox, and its mark.
mod pass;
use pass::drained;
pub use pass::relay_once;

/// The `TaskSpec::Custom` kind, and the run's `source_type`.
pub const RELAY_KIND: &str = "app_events_relay";
/// The most rows one pass claims.
pub const BATCH: i64 = 500;
/// The `fleet_job_claims` row the sweep claims each interval.
pub const SWEEP_JOB: &str = "custom_app_event_relay_sweep";
/// How often the sweep looks for outboxes with rows waiting.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// How long a mark is left to its publish's nudge before the sweep queues a
/// relay for it.
pub const SETTLE: Duration = Duration::from_secs(60);
/// How long work may wait in an outbox (its mark's `pending_since`) before
/// the sweep logs `app_events_outbox_stale`.
pub const STALE_AFTER: Duration = Duration::from_secs(5 * 60);

const PRODUCTION: &str = "production";

/// The payload of one relay task: whose outbox, and which environment's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayPayload {
    pub org_id: Uuid,
    pub environment: String,
}

impl RelayPayload {
    fn spec(&self) -> Result<TaskSpec, DbErr> {
        Ok(TaskSpec::Custom {
            kind: RELAY_KIND.to_string(),
            payload: serde_json::to_value(self).map_err(|e| DbErr::Custom(e.to_string()))?,
        })
    }

    fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == RELAY_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad app events relay payload: {e}"))
            }
            other => Err(format!("not an app events relay task: {other:?}")),
        }
    }
}

/// What one pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RelayStats {
    /// Events this pass stored and fanned out.
    pub relayed: usize,
    /// Deliveries it inserted, held ones included.
    pub deliveries: usize,
    /// Rows it refused: stored with a relay error, or dropped for having no
    /// app to store them under.
    pub rejected: usize,
}

/// One outbox row as the platform stores it: routing metadata, never the
/// body. Its environment is the one its store vouches for, never the row's
/// own claim.
#[derive(Clone, Debug)]
pub struct RelayedEvent {
    /// What each listener's task carries. `id` is the one the relay derived,
    /// and `dataschema` the revision the row was published against: empty
    /// when no such revision is recorded, which `relay_error` then says.
    /// `data` is always `null`: the body stays in the org's store, read by
    /// `(writer_role, id)` when the event is delivered.
    pub envelope: OxyEvent,
    pub org_id: Uuid,
    /// The publishing app's workspace.
    pub workspace_id: Uuid,
    pub source_app_id: Option<Uuid>,
    /// The writer role that published the row: with the id, the key of the
    /// body in the org's outbox log.
    pub writer_role: String,
    /// The body's size, as `jsonb` text in the org's store.
    pub data_bytes: i32,
    /// The revision the row was published against.
    pub schema_revision_id: Option<Uuid>,
    pub delay_seconds: i32,
    /// Why the event is stored but delivered to nobody.
    pub relay_error: Option<String>,
    /// The one listener of a directed event (`ctx.apps.send`), resolved when
    /// the row was read; `None` for an event its subscribers receive.
    pub directed: Option<Listener>,
    /// Deliver to this one listener (app id, function) and no other: the
    /// backfill event `oxyc events resync --backfill` queues.
    pub only_listener: Option<(Uuid, String)>,
    /// Who a directed event's sending run ran as, verified against the host's
    /// invoker MAC (`relay::read`): its delivery runs as that person. `None`
    /// for every other event, and for a send that runs as the system.
    pub invoker: Option<super::provenance::Invoker>,
}

/// A listener's retry policy: the `retries` its manifest declares (a single
/// attempt included), or 5 attempts backing off from 10 s to 10 minutes.
pub fn listener_task_policy(function_manifest: &Value) -> TaskPolicy {
    if function_manifest.pointer("/retries/maxAttempts").is_some() {
        return function_task_policy(function_manifest).unwrap_or(TaskPolicy {
            retry: None,
            fallback_targets: Vec::new(),
        });
    }
    TaskPolicy {
        retry: Some(RetryPolicy {
            max_retries: 4,
            backoff: BackoffStrategy::Exponential {
                initial_delay_ms: 10_000,
                max_delay_ms: 600_000,
            },
            retry_on: Vec::new(),
        }),
        fallback_targets: Vec::new(),
    }
}

/// Queue a relay of `org_id`'s `environment` outbox, unless one is already
/// waiting to run (one that is running may have claimed its batch before the
/// row that asked for this one committed, so it does not count). `false`
/// when the org has no app to queue it under: nothing in its outbox could be
/// stored either.
pub async fn enqueue_relay(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<bool, DbErr> {
    let prefix = format!("{RELAY_KIND}:{org_id}:{environment}:");
    let waiting = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS waiting FROM agentic_task_queue \
              WHERE task_id LIKE $1 AND queue_status = 'queued' LIMIT 1",
            [format!("{prefix}%").into()],
        ))
        .await?;
    if waiting.is_some() {
        return Ok(true);
    }
    let Some(workspace_id) = run_workspace(db, org_id).await? else {
        tracing::warn!(%org_id, environment, "app events relay not queued: the org has no app");
        return Ok(false);
    };
    let payload = RelayPayload {
        org_id,
        environment: environment.to_string(),
    };
    let spec = payload.spec()?;
    let run_id = format!("{prefix}{}", Utc::now().timestamp_micros());
    let txn = db.begin().await?;
    insert_run(
        &txn,
        &run_id,
        "Relay app events",
        None,
        RELAY_KIND,
        Some(json!({ "org_id": org_id, "environment": environment })),
        workspace_id,
    )
    .await?;
    enqueue_task(&txn, &run_id, &run_id, None, &spec, None, TaskScope::Global).await?;
    txn.commit().await?;
    Ok(true)
}

/// The workspace a relay's run is queued under. A run belongs to a workspace
/// and the relay to the org, so this only decides where the run is listed and
/// which driver takes it: one of the org's apps' workspaces, preferring one
/// the run driver can build a context for (a path and a promoted revision),
/// then the oldest app's. Fully ordered, so every call picks the same one.
pub(crate) async fn run_workspace(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Option<Uuid>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT a.project_id FROM apps a LEFT JOIN workspaces w ON w.id = a.project_id \
              WHERE a.org_id = $1 \
              ORDER BY w.path IS NULL, w.current_revision_id IS NULL, a.created_at, a.id \
              LIMIT 1",
            [org_id.into()],
        ))
        .await?;
    row.map(|r| r.try_get("", "project_id")).transpose()
}

/// What [`relay_rows`] made of a claimed batch.
pub(crate) struct Relayed {
    pub(crate) stats: RelayStats,
    /// The rows no stored event points at, which the claim deletes rather
    /// than marks: nothing would ever read their bodies.
    pub(crate) discard: Vec<OutboxKey>,
}

/// Steps 2 to 4 for rows claimed from `target`: read each, then store and fan
/// them out in one platform transaction, committed. The caller marks the
/// rows relayed afterwards, deleting the ones in [`Relayed::discard`].
pub(crate) async fn relay_rows(
    db: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    rows: &[OutboxRow],
) -> Result<Relayed, String> {
    let failed = |e: DbErr| format!("relay the org's events: {e}");
    let tenant = oxy_oltp::migrator::tenant_for_org(db, org_id)
        .await
        .map_err(|e| e.to_string())?;
    let mut apps_by_role: HashMap<String, Option<apps::Model>> = HashMap::new();
    // By app, environment and name: a branch's rows are in several
    // environments, each with its own live build.
    let mut schemas: HashMap<(Uuid, String, String), EventSchemas> = HashMap::new();
    let mut vouch = Vouch::new(db, target);
    let mut events = Vec::with_capacity(rows.len());
    // The row each event came from, by index.
    let mut sources = Vec::with_capacity(rows.len());
    let mut discard = Vec::new();
    let mut dropped = 0;
    for row in rows {
        let Some(app) = cached_app(db, &mut apps_by_role, tenant.id, org_id, &row.writer_role)
            .await
            .map_err(failed)?
        else {
            // Nothing to store it under (`source_app_id` is an app), and left
            // in the outbox it would be claimed again on every pass.
            dropped_row(org_id, row);
            dropped += 1;
            discard.push(row.key());
            continue;
        };
        let vouched = vouch
            .environment(&app, &row.environment)
            .await
            .map_err(failed)?;
        let key = (app.id, vouched.0.clone(), row.name.clone());
        let send = SendTarget::parse(&row.name);
        if send.is_none() && !schemas.contains_key(&key) {
            let found = EventSchemas::read(db, app.id, &vouched.0, &row.name)
                .await
                .map_err(failed)?;
            schemas.insert(key.clone(), found);
        }
        let mut event = match &send {
            Some(_) => read_row(&app, row, vouched, &EventSchemas::directed()),
            None => read_row(&app, row, vouched, &schemas[&key]),
        };
        if let (Some(target), None) = (&send, &event.relay_error) {
            match super::directed::resolve(db, org_id, &event.envelope.environment, &app, target)
                .await
            {
                Ok(listener) => event.directed = Some(listener),
                Err(reason) => event.relay_error = Some(reason),
            }
        }
        events.push(event);
        sources.push(row);
    }
    let (mut stats, points_here) = store_and_fan_out(db, &events, None).await.map_err(failed)?;
    stats.rejected += dropped;
    log_quarantined(org_id, target, &events);
    discard.extend(unread_bodies(&sources, &events, &points_here));
    Ok(Relayed { stats, discard })
}

/// The rows of `sources` whose body no stored event reads, which the claim
/// deletes: a row is its event's body only when the event is stored under
/// the row's own id and writer. Never a row quarantined for its provenance
/// (R25(c)), whose body is kept for whoever looks into it; retention's
/// orphan pass deletes it in time.
fn unread_bodies(
    sources: &[&OutboxRow],
    events: &[RelayedEvent],
    points_here: &[bool],
) -> Vec<OutboxKey> {
    sources
        .iter()
        .zip(events)
        .zip(points_here)
        .filter(|((row, event), points_here)| {
            !unverified(event) && (row.id != event.envelope.id || !**points_here)
        })
        .map(|((row, _), _)| row.key())
        .collect()
}

fn unverified(event: &RelayedEvent) -> bool {
    event
        .relay_error
        .as_deref()
        .is_some_and(super::provenance::is_unverified)
}

/// Log how many of `events`, read from `target`, were quarantined for their
/// provenance: the count, never a body (the relay holds none).
fn log_quarantined(org_id: Uuid, target: &OutboxTarget, events: &[RelayedEvent]) {
    let quarantined = events.iter().filter(|e| unverified(e)).count();
    if quarantined > 0 {
        tracing::warn!(
            %org_id,
            store = ?target,
            quarantined,
            "app events: quarantined outbox rows whose provenance did not verify \
             (unverified_provenance)"
        );
    }
}

/// The app whose writer role is `role`, read once per pass.
async fn cached_app(
    db: &DatabaseConnection,
    cache: &mut HashMap<String, Option<apps::Model>>,
    tenant_row_id: Uuid,
    org_id: Uuid,
    role: &str,
) -> Result<Option<apps::Model>, DbErr> {
    if let Some(app) = cache.get(role) {
        return Ok(app.clone());
    }
    let app = writer_app(db, tenant_row_id, org_id, role).await?;
    cache.insert(role.to_string(), app.clone());
    Ok(app)
}

fn dropped_row(org_id: Uuid, row: &OutboxRow) {
    tracing::error!(
        %org_id,
        event_id = %row.id,
        writer_role = %row.writer_role,
        name = %row.name,
        "app events: dropped an outbox row whose writer role is no app of the org"
    );
}

/// The app whose OLTP writer role `role` is in the tenant `tenant_row_id`.
async fn writer_app(
    db: &DatabaseConnection,
    tenant_row_id: Uuid,
    org_id: Uuid,
    role: &str,
) -> Result<Option<apps::Model>, DbErr> {
    let Some(writer) = oltp_roles::Entity::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant_row_id))
        .filter(oltp_roles::Column::RoleName.eq(role))
        .filter(oltp_roles::Column::WriterKind.eq(WriterKind::App))
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    // `app_writer_name` turns a slug's `-` into `_` and refuses a slug holding
    // `_`, so the slug is the writer name with each `_` turned back.
    let slug = writer.writer_name.replace('_', "-");
    if app_writer_name(&slug).as_deref() != Some(writer.writer_name.as_str()) {
        return Ok(None);
    }
    apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Slug.eq(slug))
        .one(db)
        .await
}

/// Step 3 and 4: store `events` and fan each new one out to its listeners in
/// one platform transaction, and commit it. An event already stored (a relay
/// again after a crash, or beside another) adds nothing, and one with a relay
/// error is stored for nobody.
///
/// Each event's environment is trusted as given: the caller is a store's
/// relay, which decided it from the store ([`relay_rows`]).
pub(crate) async fn fan_out(
    db: &DatabaseConnection,
    events: &[RelayedEvent],
) -> Result<RelayStats, DbErr> {
    Ok(store_and_fan_out(db, events, None).await?.0)
}

/// A table cursor's move, made in the transaction that stores the event
/// ([`super::table_watch::cursors::advance_in`]).
pub(crate) struct CursorMove {
    pub id: Uuid,
    pub from: Option<i64>,
    pub to: i64,
}

/// The cursor no longer stood where the pass read it: another pass moved it,
/// so this transaction (its duplicate event included) is rolled back.
pub(crate) const CURSOR_MOVED: &str = "the table cursor moved under this pass";

/// [`fan_out`] for a table event: the event, its deliveries and the cursor's
/// move commit together, so a snapshot is delivered exactly when the cursor
/// has moved past it.
pub(crate) async fn fan_out_with_cursor(
    db: &DatabaseConnection,
    event: &RelayedEvent,
    cursor: &CursorMove,
) -> Result<RelayStats, DbErr> {
    Ok(
        store_and_fan_out(db, std::slice::from_ref(event), Some(cursor))
            .await?
            .0,
    )
}

/// [`fan_out`], and for each event whether the stored event is that one —
/// stored now, or already stored from the same writer's row — rather than
/// another writer's under the same id.
async fn store_and_fan_out(
    db: &DatabaseConnection,
    events: &[RelayedEvent],
    cursor: Option<&CursorMove>,
) -> Result<(RelayStats, Vec<bool>), DbErr> {
    // Read first, outside the transaction: who listens to each event, and
    // whether each listener's environment holds it.
    let mut listening: HashMap<(Uuid, String, String), Vec<Listener>> = HashMap::new();
    let mut holds: HashMap<(Uuid, String), bool> = HashMap::new();
    let mut plans: Vec<Vec<(Listener, bool)>> = Vec::with_capacity(events.len());
    for event in events {
        let mut plan = Vec::new();
        if event.relay_error.is_none() {
            let environment = &event.envelope.environment;
            let key = (
                event.org_id,
                environment.clone(),
                event.envelope.event_type.clone(),
            );
            if let Some(listener) = &event.directed {
                listening.insert(key.clone(), vec![listener.clone()]);
            } else if !listening.contains_key(&key) {
                let mut found =
                    listeners(db, event.org_id, environment, &event.envelope.event_type).await?;
                if is_table_event_type(&event.envelope.event_type) {
                    // A table is in one workspace's Airhouse.
                    found.retain(|l| l.workspace_id == event.workspace_id);
                }
                listening.insert(key.clone(), found);
            }
            for listener in listening[&key].iter().filter(|l| {
                event
                    .only_listener
                    .as_ref()
                    .is_none_or(|(app, function)| l.app_id == *app && &l.function_name == function)
            }) {
                let held = listener.paused
                    || environment != PRODUCTION
                        && match holds.get(&(listener.app_id, environment.clone())) {
                            Some(&held) => held,
                            None => {
                                let held = is_held(db, listener.app_id, environment).await?;
                                holds.insert((listener.app_id, environment.clone()), held);
                                held
                            }
                        };
                plan.push((listener.clone(), held));
            }
        }
        plans.push(plan);
    }
    let listener_apps: HashSet<Uuid> = plans.iter().flatten().map(|(l, _)| l.app_id).collect();
    let workspaces: HashMap<Uuid, Uuid> = if listener_apps.is_empty() {
        HashMap::new()
    } else {
        apps::Entity::find()
            .filter(apps::Column::Id.is_in(listener_apps))
            .all(db)
            .await?
            .into_iter()
            .map(|app| (app.id, app.project_id))
            .collect()
    };

    let txn = db.begin().await?;
    let mut stats = RelayStats::default();
    let mut points_here = Vec::with_capacity(events.len());
    for (event, plan) in events.iter().zip(plans) {
        match store(&txn, event).await? {
            Stored::New => points_here.push(true),
            Stored::Already { writer_role } => {
                points_here.push(writer_role == event.writer_role);
                continue;
            }
        }
        if event.relay_error.is_some() {
            stats.rejected += 1;
            continue;
        }
        stats.relayed += 1;
        for (listener, held) in plan {
            // An app deleted since `listeners` read it has no deliveries.
            let Some(&workspace_id) = workspaces.get(&listener.app_id) else {
                continue;
            };
            deliver(&txn, event, &listener, workspace_id, held).await?;
            stats.deliveries += 1;
        }
    }
    if let Some(cursor) = cursor
        && !table_watch::cursors::advance_in(&txn, cursor.id, cursor.from, cursor.to).await?
    {
        // Dropping the transaction rolls the event and its deliveries back.
        return Err(DbErr::Custom(CURSOR_MOVED.to_string()));
    }
    txn.commit().await?;
    Ok((stats, points_here))
}

/// Whether `app_id`'s `environment` (never production) holds what is
/// published to it (§6.7): its events switch is off, the app has no
/// environment of that name, or it is being deleted or a sandbox idle past its
/// expiry.
async fn is_held(db: &DatabaseConnection, app_id: Uuid, environment: &str) -> Result<bool, DbErr> {
    let Some(row) = app_environments::Entity::find_by_id((app_id, environment.to_string()))
        .one(db)
        .await?
    else {
        return Ok(true);
    };
    if !row.events_enabled || row.deleting_at.is_some() {
        return Ok(true);
    }
    if row.kind == AppEnvironmentKind::Dev.as_str() {
        return is_past_idle_expiry(db, &row, Utc::now()).await;
    }
    Ok(false)
}

/// Whether [`store`] inserted the event.
enum Stored {
    New,
    /// An event with its id was stored before, from `writer_role`'s row.
    Already {
        writer_role: String,
    },
}

/// Insert `event`'s metadata — never its body, which stays in the org's
/// store under `(writer_role, id)`.
async fn store(txn: &DatabaseTransaction, event: &RelayedEvent) -> Result<Stored, DbErr> {
    let e = &event.envelope;
    let row = app_events::ActiveModel {
        id: Set(e.id),
        org_id: Set(event.org_id),
        workspace_id: Set(event.workspace_id),
        environment: Set(e.environment.clone()),
        source_app_id: Set(event.source_app_id),
        r#type: Set(e.event_type.clone()),
        key: Set(e.key.clone()),
        data_bytes: Set(event.data_bytes),
        writer_role: Set(event.writer_role.clone()),
        schema_revision_id: Set(event.schema_revision_id),
        actor: Set(e.actor.clone()),
        causation_id: Set(e.causationid),
        root_id: Set(e.rootid),
        depth: Set(e.depth),
        invocation_id: Set(e.invocation),
        delay_seconds: Set(event.delay_seconds),
        published_at: Set(e.time.into()),
        relay_error: Set(event.relay_error.clone()),
        invoker: Set(event.invoker.map(|i| i.kind().to_string())),
        invoker_token_id: Set(event.invoker.and_then(|i| i.token_id())),
        // `relayed_at` is the database's `now()`.
        ..Default::default()
    };
    let inserted = app_events::Entity::insert(row)
        .on_conflict(
            OnConflict::column(app_events::Column::Id)
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(txn)
        .await?;
    if inserted == 1 {
        return Ok(Stored::New);
    }
    // Rare: a relay again after a crash, or the same app's event from a row
    // of another of its writer roles (a re-minted one).
    let writer_role = app_events::Entity::find_by_id(e.id)
        .one(txn)
        .await?
        .map(|stored| stored.writer_role)
        .unwrap_or_default();
    Ok(Stored::Already { writer_role })
}

/// `listener`'s delivery of `event`: `held`, or `pending` with its run queued
/// (not before the event's `delaySeconds`) and named on the row.
async fn deliver(
    txn: &DatabaseTransaction,
    event: &RelayedEvent,
    listener: &Listener,
    workspace_id: Uuid,
    held: bool,
) -> Result<(), DbErr> {
    let id = Uuid::new_v4();
    let environment = &event.envelope.environment;
    let run_id = if held {
        None
    } else {
        let mut task = AppFunctionTask::new(listener.app_id.to_string(), &listener.function_name);
        // A directed event (`ctx.apps.send`) runs as a call: its function
        // need not listen to anything, only be callable.
        let trigger = if event.directed.is_some() {
            "call"
        } else {
            "event"
        };
        task.trigger = Some(trigger.to_string());
        task.input =
            Some(serde_json::to_value(&event.envelope).map_err(|e| DbErr::Custom(e.to_string()))?);
        task.environment = (environment != PRODUCTION).then(|| environment.clone());
        task.event_delivery_id = Some(id.to_string());
        // The publish caps it; the row is app-written, so it is capped again.
        let delay = i64::from(event.delay_seconds).clamp(0, MAX_DELAY_SECS as i64);
        let available_at = Utc::now() + chrono::Duration::seconds(delay);
        let policy = listener_task_policy(&listener.function_manifest);
        let run_id = enqueue_app_function_task_at(
            txn,
            workspace_id,
            Some(policy),
            task,
            available_at.fixed_offset(),
        )
        .await
        .map_err(|e| match e {
            ScheduleError::Db(e) => e,
            other => DbErr::Custom(other.to_string()),
        })?;
        Some(run_id)
    };
    let now = Utc::now().fixed_offset();
    app_event_deliveries::ActiveModel {
        id: Set(id),
        event_id: Set(event.envelope.id),
        app_id: Set(listener.app_id),
        function_name: Set(listener.function_name.clone()),
        environment: Set(environment.clone()),
        status: Set(if held { "held" } else { "pending" }.to_string()),
        attempts: Set(0),
        last_error: Set(None),
        run_id: Set(run_id),
        invocation_id: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(txn)
    .await?;
    Ok(())
}

/// One sweep: queue a relay for every due mark ([`marks::due`]) of an org
/// with `app_events` on that was published more than [`SETTLE`] ago, and log
/// `app_events_outbox_stale` for every due mark whose work has waited more
/// than [`STALE_AFTER`] — published lately or not, so neither continuous
/// publishing nor a relay failing on every pass hides it. Every environment
/// whose store is relayed is swept: production's, staging's and each
/// sandbox's (Task 17); a mark for a name that is no environment is passed by.
/// Reads the platform database only: it never connects to an org's store.
/// A mark that cannot be queued is logged and skipped; one whose org has no
/// app is marked drained, since nothing in its outbox could be stored.
///
/// **No backfill for marks from before the branch stores were relayed** (R28
/// left staging and sandbox publishes marked but never relayed): there are
/// none to speak of, since a publish is refused while the org's `app_events`
/// flag is off, and it stayed off for every org until relaying the branch
/// stores shipped. One that did exist is due at once, its relay finds nothing
/// waiting (or relays what is), and the mark drains on that first pass.
pub async fn sweep_once(db: &DatabaseConnection) -> Result<SweepReport, DbErr> {
    let mut report = SweepReport::default();
    for due in marks::due(db, SETTLE, STALE_AFTER).await? {
        let (org_id, environment) = (due.org_id, due.environment.as_str());
        if !is_relayed(environment) || !super::enabled_for(org_id) {
            continue;
        }
        if due.waited > STALE_AFTER {
            tracing::error!(%org_id, environment, waited_seconds = due.waited.as_secs(), "app_events_outbox_stale");
            report.stale.push(org_id);
        }
        if !due.settled {
            // Its publish's own nudge has queued a relay.
            continue;
        }
        match enqueue_relay(db, org_id, environment).await {
            Ok(true) => report.queued += 1,
            Ok(false) => match marks::platform_now(db).await {
                Ok(now) => drained(db, org_id, environment, now).await,
                Err(e) => {
                    tracing::warn!(%org_id, error = %e, "app events sweep could not read the time")
                }
            },
            Err(e) => {
                tracing::warn!(%org_id, environment, error = %e, "app events sweep could not queue a relay");
            }
        }
    }
    Ok(report)
}

/// What one [`sweep_once`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Relays it found or queued waiting.
    pub queued: usize,
    /// The orgs it logged `app_events_outbox_stale` for.
    pub stale: Vec<Uuid>,
}

/// Run [`sweep_once`] once per [`SWEEP_INTERVAL`] across the fleet until
/// shutdown. Every replica that runs singleton work starts it; the
/// [`fleet_claim`](oxy_app_core::fleet_claim) row lets one of them sweep per
/// interval.
pub fn spawn_sweep(db: DatabaseConnection, shutdown: CancellationToken) {
    let pass_db = db.clone();
    tokio::spawn(oxy_app_core::fleet_claim::run_claimed(
        db,
        SWEEP_JOB,
        SWEEP_INTERVAL,
        shutdown.cancelled_owned(),
        move || {
            let db = pass_db.clone();
            async move {
                if let Err(e) = sweep_once(&db).await {
                    tracing::warn!(error = %e, "app events relay sweep failed");
                }
            }
        },
    ));
}

/// Runs a queued relay on the worker fleet.
pub struct RelayExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for RelayExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let payload = RelayPayload::from_spec(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(1);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let db = self.db.clone();
        let stop = cancel.clone();
        tokio::spawn(async move {
            // A pass dropped mid-way rolls back its platform transaction and
            // its claim: the rows wait for the next relay.
            let outcome = tokio::select! {
                outcome = run_guarded(&db, &payload) => outcome,
                _ = stop.cancelled() => TaskOutcome::Failed("cancelled".to_string()),
            };
            let _ = outcome_tx.send(outcome).await;
            drop(event_tx);
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

/// The task's outcome. A panic still ends in `Failed`, or the run would stay
/// `running` with no terminal event.
async fn run_guarded(db: &DatabaseConnection, payload: &RelayPayload) -> TaskOutcome {
    let RelayPayload {
        org_id,
        environment,
    } = payload;
    let pass = std::panic::AssertUnwindSafe(relay_once(db, *org_id, environment))
        .catch_unwind()
        .await;
    match pass {
        Ok(Ok(stats)) => TaskOutcome::Done {
            answer: format!(
                "relayed {} events with {} deliveries; {} rejected",
                stats.relayed, stats.deliveries, stats.rejected
            ),
            metadata: Some(json!({ "org_id": org_id, "environment": environment })),
        },
        Ok(Err(e)) => {
            tracing::warn!(%org_id, environment, error = %e, "{RELAY_KIND} failed");
            TaskOutcome::Failed(e)
        }
        Err(_) => {
            tracing::error!(%org_id, environment, "{RELAY_KIND} panicked");
            TaskOutcome::Failed("the app events relay panicked".to_string())
        }
    }
}

/// The relay's halves the integration tests drive directly: [`fan_out`],
/// the platform half every store's rows reach, and [`relay_rows`], the steps
/// between a claim and its mark. Not an API.
#[doc(hidden)]
pub mod testing {
    use super::*;

    pub async fn fan_out(
        db: &DatabaseConnection,
        events: &[RelayedEvent],
    ) -> Result<RelayStats, DbErr> {
        super::fan_out(db, events).await
    }

    /// The counts, and the rows the claim is to delete rather than mark.
    pub async fn relay_rows(
        db: &DatabaseConnection,
        org_id: Uuid,
        target: &OutboxTarget,
        rows: &[OutboxRow],
    ) -> Result<(RelayStats, Vec<OutboxKey>), String> {
        super::relay_rows(db, org_id, target, rows)
            .await
            .map(|relayed| (relayed.stats, relayed.discard))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_round_trips_through_its_spec() {
        let payload = RelayPayload {
            org_id: Uuid::from_u128(7),
            environment: "production".into(),
        };
        let spec = payload.spec().unwrap();
        assert_eq!(RelayPayload::from_spec(&spec).unwrap(), payload);
        assert!(
            RelayPayload::from_spec(&TaskSpec::Custom {
                kind: "other".into(),
                payload: json!({}),
            })
            .is_err()
        );
    }
}
