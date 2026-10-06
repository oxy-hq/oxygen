//! The maintenance loop's pass over sandboxes whose **token ended** (sandbox
//! agent credential design §2, "Sandboxes outlive their token by at most a
//! day").
//!
//! A sandbox agent token creates up to three sandboxes, and an agent that
//! crashes leaves them behind. Once the token that created a sandbox was
//! revoked or expired — whichever came first — more than [`grace`] ago, the
//! sandbox is deleted exactly as a `DELETE` would, with no actor and the
//! reason `token_ended`. The day in between is for the minter to look at the
//! result under their own session.
//!
//! **A sandbox whose token row is gone is not selected.** Its
//! `created_by_token_id` names no row, so nothing says when its token ended;
//! it falls back to idle expiry, like a sandbox a person created.
//!
//! **One transaction per token.** Its sandboxes are marked and their teardowns
//! queued together with the `token.expired_sandboxes_queued` audit event, so
//! the event is written once per token, in the transaction of the change. It
//! has one row per org of the token's granted apps, and each row names only
//! that org's sandboxes (`user_tokens::sandboxes_queued`). A second replica's
//! pass finds the rows already deleting and writes nothing.
//!
//! The rule is stated twice — [`ended_at`] and [`is_due`] in Rust for the look
//! under the lock, and [`ENDED_TOKEN_SANDBOXES_SQL`] for what the pass selects
//! — and `tests/custom_apps/sandbox_agent_token/token_ended.rs` holds the two
//! together.

use chrono::{DateTime, Utc};
use entity::{api_tokens, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use super::delete::{self, Expect};
use super::{SandboxError, TeardownReason};
use crate::server::api::user_tokens::sandboxes_queued::{self, QueuedSandbox};

/// Ended tokens handled per pass; the next pass continues. Each holds at most
/// `MAX_SANDBOXES_PER_TOKEN` sandboxes.
pub const TOKENS_PER_PASS: i64 = 20;

/// How long a sandbox outlives the token that created it.
pub fn grace() -> chrono::Duration {
    chrono::Duration::hours(24)
}

/// When a token ended: the earlier of its expiry and its revocation. `None`
/// for a token with neither.
pub fn ended_at(
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (expires_at, revoked_at) {
        (Some(expired), Some(revoked)) => Some(expired.min(revoked)),
        (ended, None) | (None, ended) => ended,
    }
}

/// Is a sandbox whose token ended at `ended_at` torn down at `now`? Strictly
/// past the grace: one whose day ends exactly now is spared this pass.
pub fn is_due(ended_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    ended_at.is_some_and(|ended| ended + grace() < now)
}

/// Active sandboxes of the first `$2` sandbox agent tokens that ended before
/// `$1`, longest-ended first — [`is_due`] in SQL. The join drops a sandbox
/// whose token row is gone; `LEAST` skips a null, as [`ended_at`] does.
pub const ENDED_TOKEN_SANDBOXES_SQL: &str = "\
    SELECT token_id, app_id, name, created_at FROM ( \
        SELECT t.id AS token_id, e.app_id, e.name, e.created_at, \
               DENSE_RANK() OVER (ORDER BY LEAST(t.expires_at, t.revoked_at), t.id) AS nth \
          FROM app_environments e \
          JOIN api_tokens t ON t.id = e.created_by_token_id \
         WHERE e.kind = 'dev' AND e.deleting_at IS NULL \
           AND t.kind = 'sandbox_agent' \
           AND LEAST(t.expires_at, t.revoked_at) < $1 \
    ) ended \
     WHERE nth <= $2 \
     ORDER BY nth, app_id, name";

/// One sandbox the pass selected, as the row it was.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Selected {
    app_id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
}

/// The live sandboxes of up to `limit` tokens that ended before `cutoff`,
/// grouped by token.
async fn ended_tokens<C: ConnectionTrait>(
    db: &C,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(Uuid, Vec<Selected>)>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            ENDED_TOKEN_SANDBOXES_SQL,
            [cutoff.into(), limit.into()],
        ))
        .await?;
    let mut tokens: Vec<(Uuid, Vec<Selected>)> = Vec::new();
    for row in rows {
        let token_id: Uuid = row.try_get("", "token_id")?;
        let selected = Selected {
            app_id: row.try_get("", "app_id")?,
            name: row.try_get("", "name")?,
            created_at: row.try_get("", "created_at")?,
        };
        match tokens.last_mut() {
            Some((last, sandboxes)) if *last == token_id => sandboxes.push(selected),
            _ => tokens.push((token_id, vec![selected])),
        }
    }
    Ok(tokens)
}

/// One pass at `now`; how many teardowns it queued. A token whose sandboxes
/// cannot be deleted is logged and left for the next pass; only a failed read
/// of the list fails the pass.
pub(super) async fn sweep(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<usize, DbErr> {
    let mut queued = 0;
    for (token_id, sandboxes) in ended_tokens(db, now - grace(), TOKENS_PER_PASS).await? {
        match delete_sandboxes_of(db, token_id, &sandboxes, now).await {
            Ok(count) => queued += count,
            Err(e) => {
                tracing::warn!(%token_id, error = %e,
                    "sandbox_maintenance: could not queue an ended token's teardowns");
            }
        }
    }
    Ok(queued)
}

/// A sandbox this pass marked, and the teardown it queued.
struct Queued {
    app: apps::Model,
    environment: AppEnvironment,
    run_id: String,
}

/// In one transaction: look at the token again, delete each of its selected
/// sandboxes that is still what the pass selected, and record the event.
/// Answers how many teardowns were queued; none writes nothing.
async fn delete_sandboxes_of(
    db: &DatabaseConnection,
    token_id: Uuid,
    sandboxes: &[Selected],
    now: DateTime<Utc>,
) -> Result<usize, SandboxError> {
    let txn = db.begin().await.map_err(|e| SandboxError::db("begin", e))?;
    let token = api_tokens::Entity::find_by_id(token_id)
        .one(&txn)
        .await
        .map_err(|e| SandboxError::db("read the ended token", e))?;
    // Gone since the list was read: its sandboxes are idle expiry's now.
    let Some(token) = token else { return Ok(0) };
    let ended = ended_at(
        token.expires_at.map(DateTime::<Utc>::from),
        token.revoked_at.map(DateTime::<Utc>::from),
    );
    let Some(ended) = ended.filter(|_| is_due(ended, now)) else {
        return Ok(0);
    };
    let mut queued = Vec::new();
    for sandbox in sandboxes {
        queued.extend(delete_one(&txn, sandbox, token.id).await?);
    }
    if queued.is_empty() {
        return Ok(0);
    }
    let named: Vec<QueuedSandbox> = queued.iter().map(Queued::named).collect();
    sandboxes_queued::record(&txn, &token, ended, &named)
        .await
        .map_err(|e| SandboxError::db("record the ended token's sandboxes", format!("{e:?}")))?;
    txn.commit()
        .await
        .map_err(|e| SandboxError::db("commit", e))?;
    for sandbox in &queued {
        sandbox.announce(db, token_id).await;
    }
    Ok(queued.len())
}

impl Queued {
    fn named(&self) -> QueuedSandbox {
        QueuedSandbox {
            org_id: self.app.org_id,
            app_id: self.app.id,
            environment: self.environment.name(),
            run_id: self.run_id.clone(),
        }
    }

    /// The sandbox's own `app.environment.deleted` row and the log line, once
    /// the delete is committed.
    async fn announce(&self, db: &DatabaseConnection, token_id: Uuid) {
        let reason = TeardownReason::TokenEnded;
        delete::audit_deletion(db, &self.app, &self.environment, None, reason).await;
        tracing::info!(app_id = %self.app.id, environment = %self.environment.name(),
            run_id = %self.run_id, %token_id, reason = reason.as_str(),
            "sandbox_maintenance: teardown queued");
    }
}

/// Mark one selected sandbox and queue its teardown, under its row lock.
/// `None`: it moved on since the pass selected it — deleted, torn down, or
/// created again under the name — or its app is gone.
async fn delete_one<C: ConnectionTrait>(
    txn: &C,
    sandbox: &Selected,
    token: Uuid,
) -> Result<Option<Queued>, SandboxError> {
    let Some(environment) = AppEnvironment::parse(&sandbox.name) else {
        return Ok(None);
    };
    let app = apps::Entity::find_by_id(sandbox.app_id)
        .one(txn)
        .await
        .map_err(|e| SandboxError::db("load the app", e))?;
    let Some(app) = app else { return Ok(None) };
    let expect = Expect::CreatedBy {
        created_at: sandbox.created_at,
        token,
    };
    let reason = TeardownReason::TokenEnded;
    match delete::mark_and_queue(txn, &app, &environment, None, reason, expect).await {
        Ok(Some(deletion)) if deletion.queued => Ok(Some(Queued {
            app,
            environment,
            run_id: deletion.run_id,
        })),
        Ok(_) | Err(SandboxError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
    }

    /// A token ends at the earlier of its expiry and its revocation; one with
    /// neither has not ended.
    #[test]
    fn a_token_ends_at_the_earlier_of_its_expiry_and_its_revocation() {
        assert_eq!(ended_at(None, None), None);
        assert_eq!(ended_at(Some(at(3, 9)), None), Some(at(3, 9)));
        assert_eq!(ended_at(None, Some(at(2, 9))), Some(at(2, 9)));
        assert_eq!(ended_at(Some(at(3, 9)), Some(at(2, 9))), Some(at(2, 9)));
        assert_eq!(
            ended_at(Some(at(1, 9)), Some(at(2, 9))),
            Some(at(1, 9)),
            "revoked after it had already expired: the expiry counts"
        );
    }

    /// The selection rule: a day after the token ended and not a moment
    /// before, whichever of the two ended it; never for a live token.
    #[test]
    fn a_sandbox_is_torn_down_a_day_after_its_token_ended_and_not_before() {
        assert_eq!(grace(), chrono::Duration::hours(24));
        let ended = Some(at(1, 9));
        assert!(!is_due(ended, at(1, 10)), "an hour after");
        assert!(!is_due(ended, at(2, 8)), "an hour short of a day");
        assert!(!is_due(ended, at(2, 9)), "exactly a day: spared");
        assert!(is_due(ended, at(2, 10)), "an hour past");
        assert!(!is_due(None, at(30, 0)), "a token that never ended");
        // A token with eight hours left, revoked now: the revocation counts.
        let revoked_early = ended_at(Some(at(1, 17)), Some(at(1, 9)));
        assert!(is_due(revoked_early, at(2, 10)));
        // A token still alive at `now`: its expiry is ahead.
        assert!(!is_due(ended_at(Some(at(9, 9)), None), at(2, 10)));
    }

    /// The SQL states the same rule: the earlier end, strictly before the
    /// cutoff, of a sandbox agent token whose row exists, for a live sandbox.
    #[test]
    fn the_sql_selects_by_the_earlier_end_of_a_token_that_still_has_a_row() {
        let sql = ENDED_TOKEN_SANDBOXES_SQL;
        assert!(sql.contains("LEAST(t.expires_at, t.revoked_at) < $1"));
        assert!(sql.contains("JOIN api_tokens t ON t.id = e.created_by_token_id"));
        assert!(
            !sql.contains("LEFT JOIN"),
            "a missing token row selects nothing"
        );
        assert!(sql.contains("e.kind = 'dev' AND e.deleting_at IS NULL"));
        assert!(sql.contains("t.kind = 'sandbox_agent'"));
    }
}
