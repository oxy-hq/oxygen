//! `oxy.test`: a notice a person sends themselves, to see that the inbox and
//! push reach them (`POST /api/notifications/test`, `../test_notice.rs`).
//!
//! The normal path end to end — a signal, a `notification_deliver` task, push
//! on the caller's devices — with three differences, all in [`super::kinds`]:
//! push only, no mute applies, and the subject is the test itself
//! ([`TestSubject`]), so there is no record to look up but the token's name.
//!
//! **One per person per [`COOLDOWN`], across every replica.** The check reads
//! the person's latest `oxy.test` signal under a transaction-scoped advisory
//! lock on the person, in the transaction that writes the next one, so two
//! taps on two pods cannot both pass it. No in-memory counter: a serve fleet
//! has as many of those as it has replicas.

use chrono::{DateTime, SubsecRound, Utc};
use entity::{api_tokens, notifications};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Statement, TransactionTrait,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::kinds::{PlatformNotice, TEST, TestSubject};
use super::render::Rendered;
use super::send;

/// The cooldown is `oxy_app_core`'s: the staff console's app test counts its
/// window by the same rule.
pub use oxy_app_core::notifications::platform::self_test::{COOLDOWN, cooldown_left};

/// What a test send did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestSent {
    /// The caller's new inbox row (`notifications.id`).
    Sent { id: Uuid },
    /// The caller had one less than [`COOLDOWN`] ago.
    RateLimited { retry_after_secs: u64 },
}

/// Send `user_id` a test notice filed under `org_id`, unless they got one less
/// than [`COOLDOWN`] ago. `via` is the API token the request came with.
pub async fn send_test(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
    via: Option<Uuid>,
) -> Result<TestSent, DbErr> {
    let txn = db.begin().await?;
    lock_person(&txn, user_id).await?;
    // Read after the lock: a request that waited on another's send must
    // measure from that send, not from before it.
    let now = Utc::now().trunc_subsecs(3);
    if let Some(last) = latest_test(&txn, user_id, now).await?
        && let Some(retry_after_secs) = cooldown_left(last, now)
    {
        txn.rollback().await?;
        return Ok(TestSent::RateLimited { retry_after_secs });
    }
    let notice = PlatformNotice::Test {
        org_id,
        subject: TestSubject { at: now, via },
    };
    let workspace = send::delivery_workspace(&txn, &notice).await?;
    let sent = send::write(&txn, &notice, &[user_id], workspace).await?;
    let send_id = sent.id.ok_or_else(|| {
        DbErr::Custom("a test notice's subject is new, yet it was already told".into())
    })?;
    let id = signal_of(&txn, send_id, user_id).await?;
    txn.commit().await?;
    send::after_commit(db, &notice, &sent, workspace).await;
    Ok(TestSent::Sent { id })
}

/// Serialize every test send by `user_id` until the transaction ends.
async fn lock_person(txn: &DatabaseTransaction, user_id: Uuid) -> Result<(), DbErr> {
    txn.execute_raw(Statement::from_sql_and_values(
        txn.get_database_backend(),
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        [format!("notice-test|{user_id}").into()],
    ))
    .await?;
    Ok(())
}

/// When `user_id`'s latest test notice was sent, if within the cooldown of
/// `now`. In any org: the limit is the person's.
async fn latest_test(
    txn: &DatabaseTransaction,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, DbErr> {
    let since = now - chrono::Duration::from_std(COOLDOWN).unwrap_or_default();
    let latest = notifications::Entity::find()
        .select_only()
        .column(notifications::Column::CreatedAt)
        .filter(notifications::Column::UserId.eq(user_id))
        .filter(notifications::Column::Kind.eq(TEST))
        .filter(notifications::Column::AppId.is_null())
        .filter(notifications::Column::CreatedAt.gt(since.fixed_offset()))
        .order_by_desc(notifications::Column::CreatedAt)
        .into_tuple::<DateTime<chrono::FixedOffset>>()
        .one(txn)
        .await?;
    Ok(latest.map(|at| at.with_timezone(&Utc)))
}

/// The inbox row send `send_id` wrote for `user_id`.
async fn signal_of(txn: &DatabaseTransaction, send_id: Uuid, user_id: Uuid) -> Result<Uuid, DbErr> {
    notifications::Entity::find()
        .select_only()
        .column(notifications::Column::Id)
        .filter(notifications::Column::MessageId.eq(send_id))
        .filter(notifications::Column::UserId.eq(user_id))
        .into_tuple::<Uuid>()
        .one(txn)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("the test notice's inbox row".into()))
}

/// A test notice, with the token that sent it named.
#[derive(Debug, Clone, PartialEq)]
pub struct TestRecord {
    pub subject: TestSubject,
    /// The sending token's name; `None` for a login session, or when the
    /// token is gone.
    pub token_name: Option<String>,
}

impl TestRecord {
    /// Where it was sent from, as the body names it: the token's name, or
    /// "Oxygen" for a login session — the web app and the mobile app sign in
    /// the same way, so a session cannot say which.
    pub fn sent_from(&self) -> &str {
        match (&self.subject.via, &self.token_name) {
            (Some(_), Some(name)) => name,
            (Some(_), None) => "the API",
            (None, _) => "Oxygen",
        }
    }

    /// What it says. Taps through to the notification settings, where the
    /// person's devices are.
    pub fn words(&self, org_slug: Option<&str>) -> Rendered {
        let body = format!(
            "Sent from {} at {} UTC on {}.",
            self.sent_from(),
            self.subject.at.format("%H:%M:%S"),
            self.subject.at.format("%B %-d")
        );
        Rendered {
            title: TITLE.to_string(),
            body: Some(body),
            href: org_slug.map(|s| format!("/{s}?settings=account.notifications")),
        }
    }
}

/// A test notice's title, also its fallback.
pub const TITLE: &str = "Test notification";

/// Test notices by subject id, naming the tokens that sent them in one query.
pub async fn load(
    db: &DatabaseConnection,
    ids: Vec<String>,
) -> Result<Vec<(String, TestRecord)>, DbErr> {
    let subjects: Vec<(String, TestSubject)> = ids
        .into_iter()
        .filter_map(|id| TestSubject::parse(&id).map(|s| (id, s)))
        .collect();
    let tokens: HashSet<Uuid> = subjects.iter().filter_map(|(_, s)| s.via).collect();
    let names: HashMap<Uuid, String> = if tokens.is_empty() {
        HashMap::new()
    } else {
        api_tokens::Entity::find()
            .filter(api_tokens::Column::Id.is_in(tokens))
            .all(db)
            .await?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect()
    };
    Ok(subjects
        .into_iter()
        .map(|(id, subject)| {
            let token_name = subject.via.and_then(|v| names.get(&v).cloned());
            let record = TestRecord {
                subject,
                token_name,
            };
            (id, record)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32, s: u32, ms: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 16, h, m, s).unwrap()
            + chrono::Duration::milliseconds(ms.into())
    }

    fn record(via: Option<Uuid>, token_name: Option<&str>) -> TestRecord {
        TestRecord {
            subject: TestSubject {
                at: at(14, 3, 21, 123),
                via,
            },
            token_name: token_name.map(str::to_string),
        }
    }

    #[test]
    fn test_notice_names_where_it_came_from_and_opens_notification_settings() {
        let token = Some(Uuid::new_v4());
        let r = record(token, Some("Luong's iPhone")).words(Some("acme"));
        assert_eq!(r.title, "Test notification");
        assert_eq!(
            r.body.as_deref(),
            Some("Sent from Luong's iPhone at 14:03:21 UTC on October 16.")
        );
        assert_eq!(
            r.href.as_deref(),
            Some("/acme?settings=account.notifications")
        );
        assert_eq!(record(token, None).sent_from(), "the API");
        assert_eq!(record(None, None).sent_from(), "Oxygen");
        assert_eq!(record(None, None).words(None).href, None);
    }
}
