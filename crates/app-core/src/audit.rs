//! Audit emission + read-scope queries for the append-only `audit_events`
//! stream. See `internal-docs/partner-platform.md`
//! §6.
//!
//! Callers build an [`AuditEntry`] at each privileged mutation and pass it to
//! [`record`]. `record` computes a **per-org hash chain** (tamper-evidence):
//! each event's `hash = sha256(prev_hash || content)`, where `prev_hash` is the
//! previous event's hash for the same org. To keep the chain correct under
//! concurrency without serializing unrelated orgs, the read-then-insert runs
//! inside a transaction guarded by a Postgres advisory lock keyed on the org id.
//! Events with no org scope (platform-level, e.g. partner creation) are recorded
//! unchained.
//!
//! Reads come in three scopes matching the design: org, partner-subtree, and
//! platform. Each is bounded by a `limit` (silent-truncation-free — callers pass
//! an explicit cap).

use chrono::Utc;
use entity::audit_events;
use entity::prelude::AuditEvents;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Seed for the first event in any chain.
const GENESIS: &str = "genesis";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorType {
    User,
    System,
    ApiKey,
    PartnerAdmin,
}

impl ActorType {
    fn as_str(self) -> &'static str {
        match self {
            ActorType::User => "user",
            ActorType::System => "system",
            ActorType::ApiKey => "api_key",
            ActorType::PartnerAdmin => "partner_admin",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Failure => "failure",
        }
    }
}

/// Request-derived context: source IP, user agent, and the request/trace id so
/// an event can be correlated with the trace stream.
#[derive(Clone, Debug, Default)]
pub struct AuditContext {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub request_id: Option<String>,
}

/// A privileged action to record. Construct with [`AuditEntry::new`] and refine
/// with the builder setters; unspecified fields default to `None`/empty.
#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub actor_user_id: Option<Uuid>,
    pub actor_email: String,
    pub actor_type: ActorType,
    /// Versioned action name, e.g. `partner.org.attached`.
    pub action: &'static str,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub partner_id: Option<Uuid>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub target_label: Option<String>,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub context: AuditContext,
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub metadata: Value,
    /// The custom-app environment the action happened in
    /// (`audit_events.environment`). `None` is production — the column's
    /// default, and every row written before environments existed.
    pub environment: Option<String>,
}

impl AuditEntry {
    pub fn new(actor_email: impl Into<String>, action: &'static str) -> Self {
        Self {
            actor_user_id: None,
            actor_email: actor_email.into(),
            actor_type: ActorType::User,
            action,
            org_id: None,
            workspace_id: None,
            partner_id: None,
            target_type: None,
            target_id: None,
            target_label: None,
            before: None,
            after: None,
            context: AuditContext::default(),
            outcome: Outcome::Success,
            reason: None,
            metadata: json!({}),
            environment: None,
        }
    }

    pub fn actor(mut self, user_id: Uuid, actor_type: ActorType) -> Self {
        self.actor_user_id = Some(user_id);
        self.actor_type = actor_type;
        self
    }

    pub fn org(mut self, org_id: Uuid) -> Self {
        self.org_id = Some(org_id);
        self
    }

    pub fn workspace(mut self, workspace_id: Uuid) -> Self {
        self.workspace_id = Some(workspace_id);
        self
    }

    pub fn partner(mut self, partner_id: Uuid) -> Self {
        self.partner_id = Some(partner_id);
        self
    }

    pub fn target(
        mut self,
        target_type: impl Into<String>,
        target_id: impl Into<String>,
        target_label: impl Into<String>,
    ) -> Self {
        self.target_type = Some(target_type.into());
        self.target_id = Some(target_id.into());
        self.target_label = Some(target_label.into());
        self
    }

    pub fn change(mut self, before: Value, after: Value) -> Self {
        self.before = Some(before);
        self.after = Some(after);
        self
    }

    pub fn context(mut self, context: AuditContext) -> Self {
        self.context = context;
        self
    }

    /// A human reason for a SUCCESSFUL action — distinct from [`Self::failure`],
    /// which sets a reason *and* flips the outcome. Required for assume-role:
    /// an unexplained impersonation is a red flag, not a convenience.
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn failure(mut self, reason: impl Into<String>) -> Self {
        self.outcome = Outcome::Failure;
        self.reason = Some(reason.into());
        self
    }

    pub fn metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// The custom-app environment the action happened in. `production` is
    /// stored as the default it already is.
    pub fn environment(mut self, environment: impl Into<String>) -> Self {
        let environment = environment.into();
        self.environment = (environment != PRODUCTION).then_some(environment);
        self
    }
}

/// `audit_events.environment`'s default.
const PRODUCTION: &str = "production";

/// The digest's environment suffix: nothing for production, so every row
/// written before the column (and every production row after) hashes exactly
/// as it always has, and `|env=<name>` otherwise, so a non-production row's
/// environment is covered by the chain like every other column.
fn environment_suffix(environment: Option<&str>) -> String {
    match environment {
        None | Some(PRODUCTION) => String::new(),
        Some(env) => format!("|env={env}"),
    }
}

/// Advisory-lock key derived from an org id (first 8 bytes, little-endian). Same
/// org → same key → serialized chain; different orgs never contend.
fn chain_key(org_id: Uuid) -> i64 {
    let b = org_id.as_bytes();
    i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Timestamps are truncated to **microseconds** before hashing *and* storing,
/// because Postgres `timestamptz` is microsecond-precision: hashing a nanosecond
/// `Utc::now()` and then storing a truncated value would make the chain
/// impossible to recompute from the rows (review #9 — the verifier surfaced it).
fn to_micros(t: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    use chrono::SubsecRound;
    t.trunc_subsecs(6)
}

/// Deterministic, fixed-order digest input for one event. Same event → same
/// content string, so the chain is reproducible for later verification.
fn content_digest_input(entry: &AuditEntry, id: Uuid, created_at: &str) -> String {
    let j = |v: &Option<Value>| v.as_ref().map(|x| x.to_string()).unwrap_or_default();
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    let uuid = |v: &Option<Uuid>| v.map(|u| u.to_string()).unwrap_or_default();
    // Covers EVERY persisted column so the chain is tamper-evident over the whole
    // row (target label, reason, and request context included), not just a subset.
    format!(
        "{id}|{created_at}|{action}|{actor}|{atype}|{org}|{ws}|{partner}|{ttype}|{tid}|{tlabel}|{before}|{after}|{ip}|{ua}|{req}|{outcome}|{reason}|{meta}{env}",
        action = entry.action,
        actor = entry.actor_email,
        atype = entry.actor_type.as_str(),
        org = uuid(&entry.org_id),
        ws = uuid(&entry.workspace_id),
        partner = uuid(&entry.partner_id),
        ttype = s(&entry.target_type),
        tid = s(&entry.target_id),
        tlabel = s(&entry.target_label),
        before = j(&entry.before),
        after = j(&entry.after),
        ip = s(&entry.context.ip),
        ua = s(&entry.context.user_agent),
        req = s(&entry.context.request_id),
        outcome = entry.outcome.as_str(),
        reason = s(&entry.reason),
        meta = entry.metadata,
        env = environment_suffix(entry.environment.as_deref()),
    )
}

/// The same digest, rebuilt from a **persisted row** — the verifier's half of
/// [`content_digest_input`]. The field order here must mirror it exactly.
fn content_digest_from_model(m: &audit_events::Model) -> String {
    let j = |v: &Option<serde_json::Value>| v.as_ref().map(|x| x.to_string()).unwrap_or_default();
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    let uuid = |v: &Option<Uuid>| v.map(|u| u.to_string()).unwrap_or_default();
    format!(
        "{id}|{created_at}|{action}|{actor}|{atype}|{org}|{ws}|{partner}|{ttype}|{tid}|{tlabel}|{before}|{after}|{ip}|{ua}|{req}|{outcome}|{reason}|{meta}{env}",
        id = m.id,
        created_at = m.created_at.to_utc().to_rfc3339(),
        action = m.action,
        actor = m.actor_email,
        atype = m.actor_type,
        org = uuid(&m.org_id),
        ws = uuid(&m.workspace_id),
        partner = uuid(&m.partner_id),
        ttype = s(&m.target_type),
        tid = s(&m.target_id),
        tlabel = s(&m.target_label),
        before = j(&m.before),
        after = j(&m.after),
        ip = s(&m.ip),
        ua = s(&m.user_agent),
        req = s(&m.request_id),
        outcome = m.outcome,
        reason = s(&m.reason),
        meta = m.metadata,
        env = environment_suffix(Some(m.environment.as_str())),
    )
}

/// Result of walking one org's hash chain.
#[derive(Debug, serde::Serialize)]
pub struct ChainReport {
    pub org_id: Uuid,
    pub events: usize,
    pub intact: bool,
    /// The first event whose stored hash / prev-link doesn't reproduce.
    pub broken_at: Option<Uuid>,
    pub detail: Option<String>,
}

/// Walk an org's audit chain in `seq` order and recompute every link. Without
/// this the chain is write-only — tamper-*evident* in principle but unfalsifiable
/// in practice (review #9).
pub async fn verify_chain(db: &DatabaseConnection, org_id: Uuid) -> Result<ChainReport, DbErr> {
    let events = AuditEvents::find()
        .filter(audit_events::Column::OrgId.eq(org_id))
        .order_by_asc(audit_events::Column::Seq)
        .all(db)
        .await?;

    // Anchor on the OLDEST RETAINED event's claimed predecessor rather than the
    // genesis. On a full chain the first event's `prev_hash` IS `None` (genesis),
    // so this is unchanged; after retention pruning it's the hash of a since-deleted
    // event, which we trust as the anchor (it's gone, so it can't be re-derived).
    // Every retained event's self-hash and every inter-link is still verified — only
    // the one link into the pruned window is (necessarily) taken on faith.
    let mut prev: Option<String> = events.first().and_then(|m| m.prev_hash.clone());
    for m in &events {
        let expected = compute_hash(prev.as_deref(), &content_digest_from_model(m));
        if m.prev_hash != prev {
            return Ok(ChainReport {
                org_id,
                events: events.len(),
                intact: false,
                broken_at: Some(m.id),
                detail: Some("prev_hash does not match the preceding event".into()),
            });
        }
        if m.hash.as_deref() != Some(expected.as_str()) {
            return Ok(ChainReport {
                org_id,
                events: events.len(),
                intact: false,
                broken_at: Some(m.id),
                detail: Some("row content does not reproduce its stored hash".into()),
            });
        }
        prev = m.hash.clone();
    }
    Ok(ChainReport {
        org_id,
        events: events.len(),
        intact: true,
        broken_at: None,
        detail: None,
    })
}

/// How long the audit log is retained. It's write-heavy, so we keep a rolling
/// window rather than growing unbounded; verification (above) is anchored so a
/// prune doesn't report a false break.
pub const AUDIT_RETENTION_DAYS: i64 = 30;

/// Delete audit events older than `retain_days`, returning how many rows went. The
/// chain stays verifiable over the retained window (see `verify_chain`). Idempotent
/// and cheap to run often — it only ever touches rows past the cutoff.
pub async fn prune_older_than(db: &DatabaseConnection, retain_days: i64) -> Result<u64, DbErr> {
    let cutoff = (Utc::now() - chrono::Duration::days(retain_days)).fixed_offset();
    // The table is append-only at the database (a `BEFORE UPDATE OR DELETE`
    // trigger); DELETE is opened for this transaction alone by the flag the
    // trigger checks. `SET LOCAL` dies with the transaction, so nothing else on
    // the connection inherits it.
    let txn = db.begin().await?;
    if txn.get_database_backend() == DatabaseBackend::Postgres {
        txn.execute_unprepared(&format!("SET LOCAL {PRUNE_SETTING} = 'on'"))
            .await?;
    }
    let res = AuditEvents::delete_many()
        .filter(audit_events::Column::CreatedAt.lt(cutoff))
        .exec(&txn)
        .await?;
    txn.commit().await?;
    Ok(res.rows_affected)
}

/// The transaction-scoped Postgres setting that lets the retention prune
/// delete. Checked by the `audit_events_append_only` trigger.
pub const PRUNE_SETTING: &str = "oxy.audit_prune";

/// The current head of one chain: the row with the highest `seq` for an org.
/// Rows recorded without an org are unchained (see the module doc) and are
/// not heads; neither is a row without a hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainHead {
    pub org_id: Uuid,
    pub seq: i64,
    pub hash: String,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
}

/// Every chain's head, one row per org, for anchoring outside the database.
pub async fn chain_heads(db: &DatabaseConnection) -> Result<Vec<ChainHead>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_string(
            db.get_database_backend(),
            "SELECT DISTINCT ON (org_id) org_id, seq, hash, created_at \
             FROM audit_events WHERE org_id IS NOT NULL AND hash IS NOT NULL \
             ORDER BY org_id, seq DESC",
        ))
        .await?;
    rows.iter()
        .map(|r| {
            Ok(ChainHead {
                org_id: r.try_get("", "org_id")?,
                seq: r.try_get("", "seq")?,
                hash: r.try_get("", "hash")?,
                created_at: r.try_get("", "created_at")?,
            })
        })
        .collect()
}

/// The row at `seq` on an org's chain: `None` when no such row exists,
/// `Some(hash)` when it does — with the hash itself `None` if the row carries
/// no hash, which is a different finding from a missing row.
pub async fn hash_at(
    db: &DatabaseConnection,
    org_id: Uuid,
    seq: i64,
) -> Result<Option<Option<String>>, DbErr> {
    Ok(AuditEvents::find()
        .filter(audit_events::Column::Seq.eq(seq))
        .filter(audit_events::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .map(|m| m.hash))
}

/// Spawn a detached **daily** loop that prunes audit events past
/// [`AUDIT_RETENTION_DAYS`]. The prune is idempotent, so running it on every replica
/// is harmless; the daily cadence keeps the sweep negligible. Spawned alongside the
/// other pure-DB maintenance loops, regardless of `--no-workers`.
pub fn spawn_audit_prune_loop() {
    tokio::spawn(async {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        loop {
            tick.tick().await;
            match oxy::database::client::establish_connection().await {
                Ok(db) => match prune_older_than(&db, AUDIT_RETENTION_DAYS).await {
                    Ok(n) if n > 0 => {
                        tracing::info!(
                            pruned = n,
                            retain_days = AUDIT_RETENTION_DAYS,
                            "audit prune"
                        )
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "audit prune failed"),
                },
                Err(e) => tracing::warn!(error = %e, "audit prune: DB connect failed; skipping"),
            }
        }
    });
}

fn compute_hash(prev: Option<&str>, content: &str) -> String {
    let mut h = Sha256::new();
    h.update(prev.unwrap_or(GENESIS).as_bytes());
    h.update(b"::");
    h.update(content.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

async fn latest_hash_for_org<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<Option<String>, DbErr> {
    let latest = AuditEvents::find()
        .filter(audit_events::Column::OrgId.eq(org_id))
        // Order by the DB-assigned monotonic sequence, not app-generated
        // `created_at`, so cross-instance clock skew can't misorder the chain.
        .order_by_desc(audit_events::Column::Seq)
        .one(conn)
        .await?;
    Ok(latest.and_then(|m| m.hash))
}

#[allow(clippy::too_many_arguments)]
async fn insert_event<C: ConnectionTrait>(
    conn: &C,
    entry: &AuditEntry,
    id: Uuid,
    created_at: chrono::DateTime<Utc>,
    prev_hash: Option<String>,
    hash: String,
) -> Result<(), DbErr> {
    let model = audit_events::ActiveModel {
        id: Set(id),
        created_at: Set(created_at.into()),
        actor_user_id: Set(entry.actor_user_id),
        actor_email: Set(entry.actor_email.clone()),
        actor_type: Set(entry.actor_type.as_str().to_string()),
        action: Set(entry.action.to_string()),
        org_id: Set(entry.org_id),
        workspace_id: Set(entry.workspace_id),
        partner_id: Set(entry.partner_id),
        target_type: Set(entry.target_type.clone()),
        target_id: Set(entry.target_id.clone()),
        target_label: Set(entry.target_label.clone()),
        before: Set(entry.before.clone()),
        after: Set(entry.after.clone()),
        ip: Set(entry.context.ip.clone()),
        user_agent: Set(entry.context.user_agent.clone()),
        request_id: Set(entry.context.request_id.clone()),
        outcome: Set(entry.outcome.as_str().to_string()),
        reason: Set(entry.reason.clone()),
        metadata: Set(entry.metadata.clone()),
        prev_hash: Set(prev_hash),
        hash: Set(Some(hash)),
        seq: sea_orm::ActiveValue::NotSet, // DB-assigned BIGSERIAL
        // Unset is the column's default, `production`: a production row is
        // inserted exactly as before the column existed.
        environment: match &entry.environment {
            Some(env) => Set(env.clone()),
            None => sea_orm::ActiveValue::NotSet,
        },
    };
    model.insert(conn).await?;
    Ok(())
}

/// Record one audit event and return its id. Org-scoped events are hash-chained
/// under a per-org advisory lock; unscoped events are unchained.
///
/// ATOMICITY (important): this runs in its OWN transaction (needed for the hash
/// chain) — it is NOT part of the caller's mutation transaction. So it is not
/// atomic *with* the mutation: even if a caller propagates a `record` error, the
/// mutation has already committed.
///
/// For the compliance-critical partner **grants** (`partner.created`,
/// `partner.org.attached`, `partner.member.added`, `partner.capabilities.updated`)
/// that is not good enough — those sites now use [`record_in_txn`], which writes
/// the audit row inside the caller's own transaction so the grant and its audit
/// entry commit or roll back together (review #7). [`record_best_effort`] remains
/// for the operational-visibility events where a dropped row is acceptable.
pub async fn record(db: &DatabaseConnection, entry: AuditEntry) -> Result<Uuid, DbErr> {
    let id = Uuid::new_v4();
    let created_at = to_micros(Utc::now());
    let content = content_digest_input(&entry, id, &created_at.to_rfc3339());

    match entry.org_id {
        Some(org_id) if db.get_database_backend() == DatabaseBackend::Postgres => {
            let txn = db.begin().await?;
            txn.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT pg_advisory_xact_lock($1)",
                [chain_key(org_id).into()],
            ))
            .await?;
            let prev = latest_hash_for_org(&txn, org_id).await?;
            let hash = compute_hash(prev.as_deref(), &content);
            insert_event(&txn, &entry, id, created_at, prev, hash).await?;
            txn.commit().await?;
        }
        _ => {
            let hash = compute_hash(None, &content);
            insert_event(db, &entry, id, created_at, None, hash).await?;
        }
    }
    Ok(id)
}

/// Record an audit event **inside the caller's transaction**, so the mutation and
/// its audit row commit or roll back together (review #7). Use this for the
/// compliance-critical grants — "who gave this partner power over my org" — where
/// a silently-dropped audit row defeats the point of a tamper-evident chain.
///
/// The advisory lock is `pg_advisory_xact_lock`, i.e. transaction-scoped: taken
/// in the caller's txn, released on the caller's commit/rollback. That keeps the
/// per-org hash chain serialized exactly as [`record`] does, without a second
/// transaction.
pub async fn record_in_txn<C: ConnectionTrait>(txn: &C, entry: AuditEntry) -> Result<Uuid, DbErr> {
    let id = Uuid::new_v4();
    let created_at = to_micros(Utc::now());
    let content = content_digest_input(&entry, id, &created_at.to_rfc3339());

    match entry.org_id {
        Some(org_id) if txn.get_database_backend() == DatabaseBackend::Postgres => {
            txn.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT pg_advisory_xact_lock($1)",
                [chain_key(org_id).into()],
            ))
            .await?;
            let prev = latest_hash_for_org(txn, org_id).await?;
            let hash = compute_hash(prev.as_deref(), &content);
            insert_event(txn, &entry, id, created_at, prev, hash).await?;
        }
        _ => {
            let hash = compute_hash(None, &content);
            insert_event(txn, &entry, id, created_at, None, hash).await?;
        }
    }
    Ok(id)
}

/// Best-effort: records the event, logging (not propagating) a DB error. Because
/// audit is emitted AFTER the mutation commits (see [`record`] — separate
/// transactions), a failure here leaves the mutation standing with its audit
/// dropped-and-logged. Acceptable for **operational visibility** events. The
/// compliance-critical grants do NOT use this — they use [`record_in_txn`].
pub async fn record_best_effort(db: &DatabaseConnection, entry: AuditEntry) {
    let action = entry.action;
    if let Err(e) = record(db, entry).await {
        tracing::error!(action, error = %e, "audit: failed to record event");
    }
}

/// Escape LIKE wildcards so an operator searching for a literal `%` or `_` gets
/// literal matching instead of a pattern (review #10).
fn escape_like(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

// ── read scopes ───────────────────────────────────────────────────────────

/// Org read scope: activity within one org, most recent first.
pub async fn events_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    limit: u64,
) -> Result<Vec<audit_events::Model>, DbErr> {
    AuditEvents::find()
        .filter(audit_events::Column::OrgId.eq(org_id))
        .order_by_desc(audit_events::Column::Seq)
        .limit(limit)
        .all(db)
        .await
}

/// Partner-subtree read scope: events for the partner itself OR any org it
/// manages, in production. `org_ids` is the partner's managed-org set.
pub async fn events_for_partner(
    db: &DatabaseConnection,
    partner_id: Uuid,
    org_ids: &[Uuid],
    limit: u64,
    offset: u64,
) -> Result<Vec<audit_events::Model>, DbErr> {
    // The org clause must be correlated with the partner, or a REASSIGNED client
    // leaks its former partner's activity.
    //
    // `org_ids` is the partner's CURRENT managed set. When staff detach org X from
    // partner A and attach it to partner B — ordinary reseller churn — an
    // uncorrelated `org_id IN (…)` hands B every historical event on X, including
    // A's `partner.member.*` rows: A's admin emails, A's actions, A's client
    // relationship. That is a cross-tenant disclosure between competitors.
    //
    // So: events THIS partner emitted (wherever), plus events in its managed orgs
    // that no partner emitted (`partner_id IS NULL` — the client's own admins
    // acting in their own org, which is legitimately the partner's business).
    // Another partner's events in the same org are never visible.
    let org_scope = Condition::all()
        .add(audit_events::Column::OrgId.is_in(org_ids.iter().copied()))
        .add(
            Condition::any()
                .add(audit_events::Column::PartnerId.is_null())
                .add(audit_events::Column::PartnerId.eq(partner_id)),
        );

    AuditEvents::find()
        // Production's rows only. A partner has no reach into an app's
        // non-production environments, and the rows written there — a staging
        // invocation's held writes, carrying the Oxy staff actor's email and
        // the write's target — are staff's. In the query, before the offset
        // walk, so a page is `limit` rows the partner may read.
        .filter(audit_events::Column::Environment.eq(PRODUCTION))
        .filter(
            Condition::any()
                .add(audit_events::Column::PartnerId.eq(partner_id))
                .add(org_scope),
        )
        // `seq` is monotonic and unique, so this IS a total order — an offset
        // walk over it cannot repeat or drop a row the way one ordered by a
        // shared timestamp can.
        .order_by_desc(audit_events::Column::Seq)
        .offset(offset)
        .limit(limit)
        .all(db)
        .await
}

/// Platform read scope: the whole stream (Oxy staff only), most recent first.
pub async fn events_for_platform(
    db: &DatabaseConnection,
    limit: u64,
) -> Result<Vec<audit_events::Model>, DbErr> {
    AuditEvents::find()
        .order_by_desc(audit_events::Column::Seq)
        .limit(limit)
        .all(db)
        .await
}

/// Filters for the platform audit search (Oxy staff admin UI). All fields are
/// optional and AND-combined; `q` is a free-text OR across action / actor /
/// target label.
#[derive(Debug, Default)]
pub struct AuditFilter {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub org_id: Option<Uuid>,
    pub outcome: Option<String>,
    pub q: Option<String>,
}

/// Platform-scoped search over the audit stream, most recent first, bounded by
/// `limit` + `offset` for paging.
pub async fn search_events(
    db: &DatabaseConnection,
    filter: &AuditFilter,
    limit: u64,
    offset: u64,
) -> Result<Vec<audit_events::Model>, DbErr> {
    let mut query = AuditEvents::find();
    if let Some(action) = filter.action.as_deref().filter(|s| !s.is_empty()) {
        query = query.filter(audit_events::Column::Action.eq(action));
    }
    if let Some(actor) = filter.actor.as_deref().filter(|s| !s.is_empty()) {
        query = query.filter(audit_events::Column::ActorEmail.contains(escape_like(actor)));
    }
    if let Some(org_id) = filter.org_id {
        query = query.filter(audit_events::Column::OrgId.eq(org_id));
    }
    if let Some(outcome) = filter.outcome.as_deref().filter(|s| !s.is_empty()) {
        query = query.filter(audit_events::Column::Outcome.eq(outcome));
    }
    if let Some(q) = filter.q.as_deref().filter(|s| !s.is_empty()) {
        query = query.filter(
            Condition::any()
                .add(audit_events::Column::Action.contains(escape_like(q)))
                .add(audit_events::Column::ActorEmail.contains(escape_like(q)))
                .add(audit_events::Column::TargetLabel.contains(escape_like(q))),
        );
    }
    query
        .order_by_desc(audit_events::Column::Seq)
        .limit(limit)
        .offset(offset)
        .all(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chain is only verifiable if the WRITER's digest (from an AuditEntry)
    /// and the VERIFIER's digest (from the persisted row) produce the identical
    /// string. If these two ever drift, `verify_chain` would report every chain
    /// broken — so pin them against each other (review #9).
    #[test]
    fn writer_and_verifier_digests_agree() {
        let id = Uuid::new_v4();
        let org = Uuid::new_v4();
        let actor = Uuid::new_v4();
        // Microsecond-truncated, exactly as `record` stores it.
        let created_at = to_micros(Utc::now());

        let entry = AuditEntry::new("op@oxy.tech".to_string(), "partner.org.attached")
            .actor(actor, ActorType::User)
            .org(org)
            .target("organization", org.to_string(), "Northwind".to_string());

        let written = content_digest_input(&entry, id, &created_at.to_rfc3339());

        // The row as it comes back out of Postgres.
        let model = audit_events::Model {
            id,
            created_at: created_at.into(),
            actor_user_id: Some(actor),
            actor_email: "op@oxy.tech".to_string(),
            actor_type: ActorType::User.as_str().to_string(),
            action: "partner.org.attached".to_string(),
            org_id: Some(org),
            workspace_id: None,
            partner_id: None,
            target_type: Some("organization".to_string()),
            target_id: Some(org.to_string()),
            target_label: Some("Northwind".to_string()),
            before: None,
            after: None,
            ip: None,
            user_agent: None,
            request_id: None,
            outcome: Outcome::Success.as_str().to_string(),
            reason: None,
            metadata: serde_json::json!({}),
            prev_hash: None,
            hash: None,
            seq: 1,
            environment: "production".to_string(),
        };
        let verified = content_digest_from_model(&model);
        assert_eq!(
            written, verified,
            "writer/verifier digest drift — verify_chain would flag every chain broken"
        );
        assert!(
            !written.contains("|env="),
            "a production row hashes exactly as it did before the environment column"
        );

        // A staging row: its environment is in the digest on both sides, so
        // rewriting it to `production` breaks the chain.
        let staged = entry.environment("staging");
        let written = content_digest_input(&staged, id, &created_at.to_rfc3339());
        let mut model = model;
        model.environment = "staging".to_string();
        assert_eq!(written, content_digest_from_model(&model));
        assert!(written.ends_with("|env=staging"), "{written}");
        model.environment = "production".to_string();
        assert_ne!(written, content_digest_from_model(&model));
    }

    #[test]
    fn production_is_stored_as_the_default() {
        let entry = AuditEntry::new("op@oxy.tech", "app.oltp.write").environment("production");
        assert_eq!(entry.environment, None);
        let entry = AuditEntry::new("op@oxy.tech", "app.oltp.write").environment("staging");
        assert_eq!(entry.environment.as_deref(), Some("staging"));
    }

    /// A tampered row must not reproduce its stored hash.
    #[test]
    fn tampering_breaks_the_hash() {
        let content = "id|ts|partner.org.attached|op@oxy.tech";
        let good = compute_hash(None, content);
        let tampered = compute_hash(None, "id|ts|partner.org.detached|op@oxy.tech");
        assert_ne!(good, tampered);
    }

    #[test]
    fn escape_like_neutralizes_wildcards() {
        assert_eq!(escape_like("100%_x"), "100\\%\\_x");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn hash_is_deterministic() {
        let a = compute_hash(Some("prev"), "content");
        let b = compute_hash(Some("prev"), "content");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // sha256 hex
    }

    #[test]
    fn hash_depends_on_prev_and_content() {
        let base = compute_hash(Some("p1"), "c1");
        assert_ne!(base, compute_hash(Some("p2"), "c1")); // chain link matters
        assert_ne!(base, compute_hash(Some("p1"), "c2")); // content matters
        assert_ne!(base, compute_hash(None, "c1")); // genesis vs linked
    }

    #[test]
    fn chain_key_is_stable_per_org() {
        let org = Uuid::new_v4();
        assert_eq!(chain_key(org), chain_key(org));
        assert_ne!(chain_key(org), chain_key(Uuid::new_v4()));
    }

    #[test]
    fn content_digest_changes_with_action() {
        let id = Uuid::new_v4();
        let e1 = AuditEntry::new("a@b.com", "member.added");
        let e2 = AuditEntry::new("a@b.com", "member.removed");
        let ts = "2026-07-13T00:00:00+00:00";
        assert_ne!(
            content_digest_input(&e1, id, ts),
            content_digest_input(&e2, id, ts)
        );
    }

    #[test]
    fn actor_and_outcome_render() {
        assert_eq!(ActorType::PartnerAdmin.as_str(), "partner_admin");
        assert_eq!(Outcome::Failure.as_str(), "failure");
    }

    /// The partner audit scope must be CORRELATED, not a bare OR.
    ///
    /// Reseller churn is ordinary: staff detach org X from partner A and attach it
    /// to partner B. With an uncorrelated `org_id IN (…)`, B's audit view then
    /// returns every historical event on X — including A's `partner.member.*` rows,
    /// exposing a competitor's admin emails and actions. This asserts the shape of
    /// the condition rather than round-tripping a DB: the bug was in the boolean
    /// structure, so that is what we pin.
    #[test]
    fn partner_audit_scope_excludes_another_partners_events() {
        let me = Uuid::new_v4();
        let other = Uuid::new_v4();
        let managed_org = Uuid::new_v4();

        // The predicate `events_for_partner` builds, restated as a pure decision so
        // it can be exercised without a database.
        let visible = |event_partner: Option<Uuid>, event_org: Option<Uuid>| -> bool {
            let mine = event_partner == Some(me);
            let in_my_org = event_org == Some(managed_org)
                && (event_partner.is_none() || event_partner == Some(me));
            mine || in_my_org
        };

        // My own events, anywhere.
        assert!(visible(Some(me), Some(managed_org)));
        // The client's OWN admins acting in their org — no partner attribution.
        // Legitimately my business; I manage them.
        assert!(visible(None, Some(managed_org)));
        // The PREVIOUS partner's events in the org I now manage. Never.
        assert!(!visible(Some(other), Some(managed_org)));
        // Another partner's events elsewhere. Never.
        assert!(!visible(Some(other), Some(Uuid::new_v4())));
    }
}
