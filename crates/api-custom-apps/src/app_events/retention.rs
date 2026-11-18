//! Retention and delivery-status sync (app events design §6.9).
//!
//! The platform cleans up; an app never deletes an event. One hourly pass
//! ([`sweep_once`]), in this order, each step in batches of [`BATCH`] rows
//! under `FOR UPDATE SKIP LOCKED` so replicas and a slow pass never contend:
//!
//! 1. **Dead sync.** A `pending` delivery whose task row (`run_id`, ruling R3)
//!    is `dead` or `failed` becomes `dead`, carrying the run's error. The
//!    delivery keeps that status after the purger removes the task row.
//! 2. **Expiry.** A `pending` or `held` delivery older than [`RETENTION_DAYS`]
//!    becomes `expired`, and its task is cancelled if still waiting or
//!    running.
//! 3. **Events.** An event published more than [`RETENTION_DAYS`] ago with no
//!    `pending` delivery is deleted, its deliveries and their attempts with it
//!    (`ON DELETE CASCADE`), and **its body first**: the platform holds only
//!    the event's metadata, and the body stays in the org's OLTP outbox log
//!    under `(writer_role, id)`, so a batch's bodies are deleted from the
//!    org's store before its metadata goes. Only an org with an event to
//!    delete is connected to, never an idle one. When the store cannot be
//!    reached the batch's metadata is kept and the next pass tries again, so
//!    no body is ever left with nothing pointing at it. An org with no store
//!    at all has no bodies to delete. Its id is derived from its key, so a key
//!    reused afterwards stores a new event: nothing remembers the old one.
//!    While connected to an org's store, the pass also deletes up to
//!    [`ORPHAN_BODIES_PER_PASS`] bodies relayed more than [`ORPHAN_BODY_DAYS`]
//!    ago: an app's deletion takes its events and leaves their bodies, which
//!    no event leads retention to. Never a row still waiting.
//! 4. **Schema revisions** no retained event names and no existing build of the
//!    app declares (a draft can still be promoted, and a rollback goes back to
//!    an older build). An app with a build whose manifest cannot be read keeps
//!    all its revisions. A revision must also be older than [`SCHEMA_GRACE`]:
//!    `revision_for` stamps `created_at` on a reuse too, so one a publish just
//!    recorded or reused is normally left alone. A sweep can still take one
//!    between `revision_for`'s read and its stamp; the stamp then updates
//!    nothing, and `revision_for` records the schema afresh.
//!
//! **Steps 1 to 3 pass over an org with `app_events` off** (its events are
//! inert: kept, never synced, expired or deleted, and its store never
//! connected to). Step 4 runs for every org: a schema revision is platform
//! metadata a publish records whatever the flag, like a subscription.
//!
//! [`retire_environment_events`] is a sandbox teardown's part: everything its
//! environment published or received.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use entity::{app_builds, app_event_schemas, app_events};
use oxy_custom_apps::events::catalog::EventCatalog;
use oxy_oltp::events::{self as outbox, OutboxKey, OutboxTarget};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, Statement, TransactionTrait,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::schema::schema_sha256;

/// The `fleet_job_claims` row the sweep claims each interval.
pub const SWEEP_JOB: &str = "custom_app_event_retention";
/// How often the sweep runs, across the fleet.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Rows one statement takes.
pub const BATCH: i64 = 500;
/// How long an event is kept, and how long a delivery may stay unfinished.
pub const RETENTION_DAYS: i64 = 30;
/// How long a schema revision is left alone after it was recorded.
pub const SCHEMA_GRACE: Duration = Duration::from_secs(60 * 60);
/// How long after it was relayed a body no event points at is deleted:
/// retention and a day, by when its event (and every delivery reading it)
/// is gone.
pub const ORPHAN_BODY_DAYS: i32 = RETENTION_DAYS as i32 + 1;
/// The most such bodies one pass deletes from one org's store.
pub const ORPHAN_BODIES_PER_PASS: i64 = 1_000;

/// What one pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepStats {
    /// Deliveries marked `dead` from their task.
    pub dead: u64,
    /// Deliveries marked `expired`.
    pub expired: u64,
    /// Events deleted (their deliveries went with them).
    pub events_deleted: u64,
    /// Relayed bodies deleted from an org's store that no event pointed at
    /// any more (step 3).
    pub orphan_bodies_deleted: u64,
    /// Schema revisions deleted.
    pub schemas_deleted: u64,
}

/// Run [`sweep_once`] once per [`SWEEP_INTERVAL`] across the fleet until
/// shutdown. Start it on every replica: the
/// [`fleet_claim`](oxy_app_core::fleet_claim) row makes it once per interval.
pub fn spawn_retention(db: DatabaseConnection, shutdown: CancellationToken) {
    let pass_db = db.clone();
    tokio::spawn(oxy_app_core::fleet_claim::run_claimed(
        db,
        SWEEP_JOB,
        SWEEP_INTERVAL,
        shutdown.cancelled_owned(),
        move || {
            let db = pass_db.clone();
            async move {
                match sweep_once(&db, Utc::now()).await {
                    Ok(stats) => tracing::info!(?stats, "app events retention swept"),
                    Err(e) => tracing::warn!(error = %e, "app events retention sweep failed"),
                }
            }
        },
    ));
}

/// One pass over every org, as of `now`.
pub async fn sweep_once(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<SweepStats, DbErr> {
    sweep(db, now, None).await
}

/// [`sweep_once`] limited to what `org_id` owns: its events, their deliveries
/// and its apps' schema revisions. For a caller that must not touch another
/// org's rows.
pub async fn sweep_org(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
    org_id: Uuid,
) -> Result<SweepStats, DbErr> {
    sweep(db, now, Some(org_id)).await
}

async fn sweep(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
    org: Option<Uuid>,
) -> Result<SweepStats, DbErr> {
    let orgs = event_orgs(db, org).await?;
    let dead = sync_dead(db, now, &orgs).await?;
    let expired = expire(db, now, &orgs).await?;
    let (events_deleted, orphan_bodies_deleted) = delete_events(db, now, &orgs).await?;
    Ok(SweepStats {
        dead,
        expired,
        events_deleted,
        orphan_bodies_deleted,
        schemas_deleted: delete_schemas(db, now, org).await?,
    })
}

/// The orgs steps 1 to 3 look at: `org` alone when given, else every org
/// with an event; either way only those with `app_events` on.
async fn event_orgs(db: &DatabaseConnection, org: Option<Uuid>) -> Result<Vec<Uuid>, DbErr> {
    let candidates: Vec<Uuid> = match org {
        Some(org) => vec![org],
        None => db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT DISTINCT org_id FROM app_events",
            ))
            .await?
            .iter()
            .map(|r| r.try_get::<Uuid>("", "org_id"))
            .collect::<Result<_, _>>()?,
    };
    Ok(candidates
        .into_iter()
        .filter(|&org| super::enabled_for(org))
        .collect())
}

/// Run `sql` (taking `$1` = the instant, `$2` = the orgs, `$3` = the batch)
/// until a batch comes back short; the rows it touched.
async fn in_batches(
    db: &DatabaseConnection,
    sql: &str,
    now: DateTime<Utc>,
    orgs: &[Uuid],
) -> Result<u64, DbErr> {
    if orgs.is_empty() {
        return Ok(0);
    }
    let mut total = 0;
    loop {
        let done = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                [now.into(), orgs.to_vec().into(), BATCH.into()],
            ))
            .await?
            .rows_affected();
        total += done;
        if (done as i64) < BATCH {
            return Ok(total);
        }
    }
}

/// Step 1. The error is the run's own, else what the delivery already
/// recorded, else the task's status; capped at
/// [`MAX_ERROR_CHARS`](super::MAX_ERROR_CHARS) characters.
pub(super) const DEAD_SQL: &str = "\
WITH picked AS ( \
    SELECT d.id, \
           left(COALESCE(r.error_message, d.last_error, 'task ' || q.queue_status), 1000) AS err \
      FROM app_event_deliveries d \
      JOIN agentic_task_queue q ON q.task_id = d.run_id \
      JOIN app_events e ON e.id = d.event_id \
      LEFT JOIN agentic_runs r ON r.id = q.run_id \
     WHERE d.status = 'pending' \
       AND q.queue_status IN ('dead', 'failed') \
       AND e.org_id = ANY($2::uuid[]) \
     ORDER BY d.created_at \
     LIMIT $3 \
       FOR UPDATE OF d SKIP LOCKED) \
UPDATE app_event_deliveries d \
   SET status = 'dead', last_error = p.err, updated_at = $1::timestamptz \
  FROM picked p \
 WHERE d.id = p.id";

async fn sync_dead(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
    orgs: &[Uuid],
) -> Result<u64, DbErr> {
    in_batches(db, DEAD_SQL, now, orgs).await
}

/// Step 2. A task still queued or claimed is cancelled with the delivery.
const EXPIRE_SQL: &str = "\
WITH picked AS ( \
    SELECT d.id, d.run_id \
      FROM app_event_deliveries d \
      JOIN app_events e ON e.id = d.event_id \
     WHERE d.status IN ('pending', 'held') \
       AND d.created_at < $1::timestamptz - make_interval(days => 30) \
       AND e.org_id = ANY($2::uuid[]) \
     ORDER BY d.created_at \
     LIMIT $3 \
       FOR UPDATE OF d SKIP LOCKED), \
cancelled AS ( \
    UPDATE agentic_task_queue q \
       SET queue_status = 'cancelled', updated_at = now() \
      FROM picked p \
     WHERE q.task_id = p.run_id AND q.queue_status IN ('queued', 'claimed') \
 RETURNING q.task_id) \
UPDATE app_event_deliveries d \
   SET status = 'expired', \
       last_error = COALESCE(d.last_error, 'expired: unfinished after 30 days'), \
       updated_at = $1::timestamptz \
  FROM picked p \
 WHERE d.id = p.id";

async fn expire(db: &DatabaseConnection, now: DateTime<Utc>, orgs: &[Uuid]) -> Result<u64, DbErr> {
    in_batches(db, EXPIRE_SQL, now, orgs).await
}

/// Step 3: of the orgs `$2`, those with an event past retention (`$1` = the
/// instant). Only these are connected to.
const ORGS_WITH_EXPIRED_EVENTS_SQL: &str = "\
SELECT DISTINCT org_id FROM app_events \
 WHERE published_at < $1::timestamptz - make_interval(days => 30) \
   AND org_id = ANY($2::uuid[])";

/// Step 3: a batch of one org's events past retention, locked (`$1` = the
/// instant, `$2` = the org, `$3` = the batch). An event any of whose
/// deliveries is `pending` is left.
const EXPIRED_EVENTS_SQL: &str = "\
SELECT e.id, e.environment, e.writer_role FROM app_events e \
 WHERE e.published_at < $1::timestamptz - make_interval(days => 30) \
   AND e.org_id = $2 \
   AND NOT EXISTS (SELECT 1 FROM app_event_deliveries d \
                    WHERE d.event_id = e.id AND d.status = 'pending') \
 ORDER BY e.published_at \
 LIMIT $3 \
   FOR UPDATE OF e SKIP LOCKED";

/// Step 3 over `orgs`: `(events deleted, orphaned bodies deleted)`.
async fn delete_events(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
    orgs: &[Uuid],
) -> Result<(u64, u64), DbErr> {
    if orgs.is_empty() {
        return Ok((0, 0));
    }
    let orgs: Vec<Uuid> = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            ORGS_WITH_EXPIRED_EVENTS_SQL,
            [now.into(), orgs.to_vec().into()],
        ))
        .await?
        .iter()
        .map(|r| r.try_get::<Uuid>("", "org_id"))
        .collect::<Result<_, _>>()?;
    let (mut deleted, mut orphans) = (0, 0);
    for org_id in orgs {
        let mut reached = HashSet::new();
        loop {
            match delete_org_events(db, now, org_id).await? {
                Some(batch) => {
                    deleted += batch.deleted;
                    reached.extend(batch.reached);
                    if (batch.picked as i64) < BATCH {
                        break;
                    }
                }
                // The store cannot be reached: the rest wait for the next pass.
                None => break,
            }
        }
        for target in &reached {
            orphans += delete_orphan_bodies(db, org_id, target).await;
        }
    }
    Ok((deleted, orphans))
}

/// While retention is connected to `org_id`'s store anyway: the bodies no
/// event points at any more — an app's deletion took their events — relayed
/// more than [`ORPHAN_BODY_DAYS`] ago, at most [`ORPHAN_BODIES_PER_PASS`].
/// An org with no event to delete is not visited for them (its store is left
/// asleep), so a deleted app's bodies in an otherwise idle org wait until it
/// has one. A failure is logged and left to the next pass.
///
/// **What makes the cut-off safe.** A body relayed that long ago has no event
/// still reading it only because a delivery is created when its event is
/// relayed, every delivery unfinished at [`RETENTION_DAYS`] is expired by
/// step 2 of this same pass (before step 3 runs), and its event then goes in
/// step 3; and because the platform servers' clocks and the store's (which
/// stamped `relayed_at` and judges this cut-off) differ by well under the
/// one day of margin. Should either change, so must [`ORPHAN_BODY_DAYS`].
async fn delete_orphan_bodies(db: &DatabaseConnection, org_id: Uuid, target: &OutboxTarget) -> u64 {
    match outbox::delete_relayed_before(
        db,
        org_id,
        target,
        ORPHAN_BODY_DAYS,
        ORPHAN_BODIES_PER_PASS,
    )
    .await
    {
        Ok(deleted) => deleted,
        Err(e) => {
            tracing::warn!(
                %org_id,
                error = %e,
                "app events retention could not delete old relayed bodies; the next pass tries again"
            );
            0
        }
    }
}

/// What one batch of [`delete_org_events`] did.
struct Batch {
    /// Events picked.
    picked: usize,
    /// Events deleted.
    deleted: u64,
    /// The stores whose bodies it deleted: connected to.
    reached: Vec<OutboxTarget>,
}

/// One batch of `org_id`'s events past retention: their bodies from the
/// org's store, then their metadata, under the batch's row locks. `None` when
/// a body could not be deleted, which keeps every event of the batch.
async fn delete_org_events(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
    org_id: Uuid,
) -> Result<Option<Batch>, DbErr> {
    let txn = db.begin().await?;
    let rows = txn
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            EXPIRED_EVENTS_SQL,
            [now.into(), org_id.into(), BATCH.into()],
        ))
        .await?;
    let mut ids = Vec::with_capacity(rows.len());
    let mut bodies: HashMap<OutboxTarget, Vec<OutboxKey>> = HashMap::new();
    for row in &rows {
        let id: Uuid = row.try_get("", "id")?;
        let environment: String = row.try_get("", "environment")?;
        ids.push(id);
        // An environment whose store is not relayed has no body in a store
        // the platform reads (a branch store's go with the branch).
        if let Some(target) = super::relay::relayed_store(&environment) {
            bodies.entry(target).or_default().push(OutboxKey {
                writer_role: row.try_get("", "writer_role")?,
                id,
            });
        }
    }
    if ids.is_empty() {
        txn.commit().await?;
        return Ok(Some(Batch {
            picked: 0,
            deleted: 0,
            reached: Vec::new(),
        }));
    }
    let mut reached = Vec::new();
    if !bodies.is_empty() && has_a_store(db, org_id).await? {
        for (target, keys) in &bodies {
            if let Err(e) = outbox::delete_bodies(db, org_id, target, keys).await {
                tracing::warn!(
                    %org_id,
                    events = keys.len(),
                    error = %e,
                    "app events retention could not delete event bodies; the events are kept \
                     until the next pass"
                );
                txn.rollback().await?;
                return Ok(None);
            }
            reached.push(target.clone());
        }
    }
    let deleted = app_events::Entity::delete_many()
        .filter(app_events::Column::Id.is_in(ids.clone()))
        .exec(&txn)
        .await?
        .rows_affected;
    txn.commit().await?;
    Ok(Some(Batch {
        picked: ids.len(),
        deleted,
        reached,
    }))
}

/// Whether `org_id` has an OLTP store at all. One that has none holds no
/// event body: there is nothing to delete, and nothing to wait for.
async fn has_a_store(db: &DatabaseConnection, org_id: Uuid) -> Result<bool, DbErr> {
    match oxy_oltp::migrator::tenant_for_org(db, org_id).await {
        Ok(_) => Ok(true),
        Err(oxy_oltp::migrator::MigrateError::NotProvisioned(_)) => Ok(false),
        Err(e) => Err(DbErr::Custom(format!("find the org's OLTP store: {e}"))),
    }
}

/// The revisions of the batch after `last` that no retained event names,
/// locked.
const UNREFERENCED_SCHEMAS_SQL: &str = "\
SELECT s.id FROM app_event_schemas s \
 WHERE s.id > $1 \
   AND s.created_at < $2 \
   AND ($3::uuid IS NULL OR s.app_id IN (SELECT id FROM apps WHERE org_id = $3)) \
   AND NOT EXISTS (SELECT 1 FROM app_events e WHERE e.schema_revision_id = s.id) \
 ORDER BY s.id \
 LIMIT $4 \
   FOR UPDATE OF s SKIP LOCKED";

/// Step 4. Walks the unreferenced revisions by id, a batch at a time, and
/// deletes those no existing build of the app declares.
async fn delete_schemas(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
    org: Option<Uuid>,
) -> Result<u64, DbErr> {
    let cutoff = now - chrono::Duration::from_std(SCHEMA_GRACE).unwrap_or_default();
    let mut last = Uuid::nil();
    let mut deleted = 0;
    loop {
        let txn = db.begin().await?;
        let ids: Vec<Uuid> = txn
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                UNREFERENCED_SCHEMAS_SQL,
                [last.into(), cutoff.into(), org.into(), BATCH.into()],
            ))
            .await?
            .iter()
            .map(|r| r.try_get::<Uuid>("", "id"))
            .collect::<Result<_, _>>()?;
        let Some(&batch_last) = ids.last() else {
            txn.commit().await?;
            return Ok(deleted);
        };
        let candidates = app_event_schemas::Entity::find()
            .filter(app_event_schemas::Column::Id.is_in(ids.clone()))
            .all(&txn)
            .await?;
        let declared = declared_by_builds(&txn, &candidates).await?;
        let unused: Vec<Uuid> = candidates
            .iter()
            .filter(|s| !declared.uses(s))
            .map(|s| s.id)
            .collect();
        if !unused.is_empty() {
            deleted += app_event_schemas::Entity::delete_many()
                .filter(app_event_schemas::Column::Id.is_in(unused))
                .exec(&txn)
                .await?
                .rows_affected;
        }
        txn.commit().await?;
        if (ids.len() as i64) < BATCH {
            return Ok(deleted);
        }
        last = batch_last;
    }
}

/// What the builds the apps of `candidates` still have declare.
struct Declared {
    /// `(app, event name, schema hash)` of every event any existing build
    /// declares.
    schemas: HashSet<(Uuid, String, String)>,
    /// Apps with a build whose manifest cannot be read: what it declares is
    /// unknown, so none of their revisions is treated as unused.
    unreadable: HashSet<Uuid>,
}

impl Declared {
    fn uses(&self, s: &app_event_schemas::Model) -> bool {
        self.unreadable.contains(&s.app_id)
            || self
                .schemas
                .contains(&(s.app_id, s.event_name.clone(), s.sha256.clone()))
    }
}

/// Every build row of `candidates`' apps, not only the ones an environment
/// serves now: a draft is promoted, or a rollback goes back, to a build that
/// is still there, and its revisions must be too. A build the platform has
/// garbage-collected can be neither, so its revisions may go.
async fn declared_by_builds(
    db: &impl ConnectionTrait,
    candidates: &[app_event_schemas::Model],
) -> Result<Declared, DbErr> {
    let apps: HashSet<Uuid> = candidates.iter().map(|s| s.app_id).collect();
    let builds = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.is_in(apps))
        .all(db)
        .await?;
    let mut declared = Declared {
        schemas: HashSet::new(),
        unreadable: HashSet::new(),
    };
    for build in &builds {
        let Ok(catalog) = EventCatalog::from_manifest(build.manifest_json.as_ref()) else {
            declared.unreadable.insert(build.app_id);
            continue;
        };
        for (name, decl) in &catalog.events {
            declared
                .schemas
                .insert((build.app_id, name.clone(), schema_sha256(&decl.schema)));
        }
    }
    Ok(declared)
}

/// Remove what `app_id` published in `environment`, and what its listeners
/// were delivered there, as a sandbox's teardown. Returns the rows deleted.
/// Refuses `production`, before anything is touched.
///
/// - The events the app published in that environment, with every delivery of
///   them (cascade).
/// - The deliveries to the app's own functions in that environment, whoever
///   published the event. Another app's event stays: it is that app's.
/// - A delivery's task still waiting is cancelled first, so a function does
///   not run into a sandbox that is gone.
/// - The org's outbox mark for the environment (R36), so a torn-down sandbox
///   leaves nothing for the relay sweep — unless another app of the org still
///   has an environment of that name not itself being deleted: marks are per
///   org and environment, and its outbox may still be waiting. Not counted in
///   the rows returned.
///
/// **The outbox is not touched here.** A non-production environment's events
/// are never in production's store (R12): they are held, or their bodies sit
/// in that environment's branch store, which the teardown drops as its last
/// data step (`teardown_branch::delete_env_branch`) with every row in it.
/// Bodies are deleted only where the platform reaches the store, as
/// retention's step 3 does; nothing reachable from here is left to delete.
pub async fn retire_environment_events(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: &str,
) -> Result<u64, DbErr> {
    // Production's events are retention's alone: a teardown never takes them.
    if environment == "production" {
        return Err(DbErr::Custom(
            "retire_environment_events refuses production: only a sandbox's teardown retires \
             an environment's events"
                .into(),
        ));
    }
    let txn = db.begin().await?;
    let values = || [app_id.into(), environment.into()];
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
            SET queue_status = 'cancelled', updated_at = now() \
          WHERE queue_status IN ('queued', 'claimed') \
            AND task_id IN ( \
                SELECT d.run_id FROM app_event_deliveries d \
                 WHERE d.run_id IS NOT NULL \
                   AND ((d.app_id = $1 AND d.environment = $2) \
                     OR d.event_id IN (SELECT id FROM app_events \
                                        WHERE source_app_id = $1 AND environment = $2)))",
        values(),
    ))
    .await?;
    let deliveries = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM app_event_deliveries WHERE app_id = $1 AND environment = $2",
            values(),
        ))
        .await?
        .rows_affected();
    let events = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM app_events WHERE source_app_id = $1 AND environment = $2",
            values(),
        ))
        .await?
        .rows_affected();
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM app_event_outbox_marks m USING apps a \
          WHERE a.id = $1 AND m.org_id = a.org_id AND m.environment = $2 \
            AND NOT EXISTS ( \
                SELECT 1 FROM app_environments e JOIN apps o ON o.id = e.app_id \
                 WHERE o.org_id = a.org_id AND e.name = $2 AND e.app_id <> $1 \
                   AND e.deleting_at IS NULL)",
        values(),
    ))
    .await?;
    txn.commit().await?;
    Ok(deliveries + events)
}
