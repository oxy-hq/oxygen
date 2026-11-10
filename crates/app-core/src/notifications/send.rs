//! One notification, sent to people who are already resolved.
//!
//! [`send`] is the write path for a custom app's `ctx.notify` (the
//! platform's own notices store no content and are written by
//! [`super::platform::send`]):
//!
//! 1. An idempotency key that was already used answers with the first send,
//!    and nothing is written.
//! 2. The content goes to the org's OLTP store (`oxy_inbox.messages`). First,
//!    because a signal pointing at content that was never written is a
//!    notification nobody can read; content with no signal is invisible.
//! 3. One transaction writes the send, a signal per recipient, and — when the
//!    send has a workspace to run under — the delivery task, so a queued push
//!    can never refer to a send that rolled back.
//!
//! Who the recipients are is decided before this ([`super::recipients`]);
//! which channel reaches each of them, after it (`delivery` in
//! `oxy-api-notifications`).

use chrono::Utc;
use entity::{notification_sends, notifications};
use oxy_oltp::inbox::{self, InboxError, InboxMessage};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
    SqlErr, TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The custom app a notification comes from.
#[derive(Debug, Clone)]
pub struct AppRef {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

/// The email a notification sends, already composed.
#[derive(Debug, Clone)]
pub struct EmailContent {
    pub subject: String,
    pub html: Option<String>,
    pub text: Option<String>,
}

/// Everything a send needs, recipients resolved.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub org_id: Uuid,
    pub app: Option<AppRef>,
    /// `production`, `staging` or a sandbox — the sending run's environment.
    pub environment: String,
    /// The workspace the delivery run is recorded under. `None` delivers
    /// inline instead of queueing.
    pub workspace_id: Option<Uuid>,
    pub kind: String,
    pub critical: bool,
    pub title: String,
    pub body: Option<String>,
    pub link: Option<String>,
    pub email: Option<EmailContent>,
    pub slack: bool,
    pub idempotency_key: Option<String>,
    /// The thread's hash ([`thread_key`]), never the app's text.
    pub thread_key: Option<String>,
    /// Distinct user ids.
    pub recipients: Vec<Uuid>,
}

impl Outgoing {
    fn app_id(&self) -> Option<Uuid> {
        self.app.as_ref().map(|a| a.id)
    }
}

/// What a send did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The send's id — also the content row's.
    pub id: Uuid,
    pub recipients: usize,
    /// An earlier send with the same idempotency key answered instead.
    pub deduplicated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// The org has no store to keep notification content in.
    #[error("{0}")]
    NoStore(InboxError),
    /// The org's store refused or could not be reached.
    #[error("{0}")]
    Store(InboxError),
    #[error("notification bookkeeping: {0}")]
    Db(#[from] DbErr),
}

impl From<InboxError> for SendError {
    fn from(e: InboxError) -> Self {
        if e.is_not_provisioned() {
            SendError::NoStore(e)
        } else {
            SendError::Store(e)
        }
    }
}

/// Hex SHA-256 of an app's idempotency key. The key is the app's text and may
/// embed a customer's data; a dedupe needs only this.
pub fn key_hash(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// How long a stored [`thread_key`] is: half a SHA-256 in hex. Enough that two
/// threads of one app never collide, and short enough to index.
pub const THREAD_KEY_LEN: usize = 32;

/// What `notifications.thread_key` holds for `thread` from `app` (`None`:
/// Oxygen): SHA-256 of the source and the text, hex, the first
/// [`THREAD_KEY_LEN`] characters. The app's text may name a customer, and a
/// client collapsing a thread needs only equality. Per source, so two apps
/// that both say `store:clovis` are two threads, and the key does not tell
/// anyone that they used the same words.
pub fn thread_key(app: Option<Uuid>, thread: &str) -> String {
    let source = app.map_or_else(|| "oxygen".to_string(), |a| a.to_string());
    let mut hex = key_hash(&format!("{source}\n{thread}"));
    hex.truncate(THREAD_KEY_LEN);
    hex
}

/// Send one notification. See the module docs for the order of writes.
///
/// With a `workspace_id` the delivery task is queued in the same transaction.
/// With none nothing is queued and the caller delivers inline: delivery is the
/// push, email and Slack transport, which needs `AppState`'s secrets and HTTP
/// clients and so is the notifications surface's (`oxy-api-notifications`'s
/// `send::send` wraps this function and does it).
pub async fn send(db: &DatabaseConnection, out: Outgoing) -> Result<Sent, SendError> {
    let hash = out.idempotency_key.as_deref().map(key_hash);
    if let Some(prior) = prior_send(db, &out, hash.as_deref()).await? {
        return Ok(prior);
    }
    let id = Uuid::new_v4();
    // Nobody to tell writes no content, but the send is still recorded: a
    // retried call with the same key must get the same "nobody" back rather
    // than a second try.
    if !out.recipients.is_empty() {
        inbox::write(db, out.org_id, &content(id, &out)).await?;
    }
    let sent = record_or_dedupe(db, id, &out, hash.as_deref()).await?;
    Ok(sent)
}

/// The content row for send `id`.
fn content(id: Uuid, out: &Outgoing) -> InboxMessage {
    InboxMessage {
        id,
        app_id: out.app_id(),
        kind: out.kind.clone(),
        title: out.title.clone(),
        body: out.body.clone(),
        link: out.link.clone(),
        email_subject: out.email.as_ref().map(|e| e.subject.clone()),
        email_html: out.email.as_ref().and_then(|e| e.html.clone()),
        email_text: out.email.as_ref().and_then(|e| e.text.clone()),
    }
}

/// The earlier send this key names, if there was one.
async fn prior_send(
    db: &DatabaseConnection,
    out: &Outgoing,
    hash: Option<&str>,
) -> Result<Option<Sent>, DbErr> {
    let Some(hash) = hash else { return Ok(None) };
    let mut q = notification_sends::Entity::find()
        .filter(notification_sends::Column::Environment.eq(&out.environment))
        .filter(notification_sends::Column::IdempotencyKeySha256.eq(hash));
    q = match out.app_id() {
        Some(app) => q.filter(notification_sends::Column::AppId.eq(app)),
        None => q.filter(notification_sends::Column::AppId.is_null()),
    };
    Ok(q.one(db).await?.map(|row| Sent {
        id: row.id,
        recipients: usize::try_from(row.recipients).unwrap_or(0),
        deduplicated: true,
    }))
}

/// Record the send, or — when a concurrent call with the same key won the
/// unique index — answer with that one.
async fn record_or_dedupe(
    db: &DatabaseConnection,
    id: Uuid,
    out: &Outgoing,
    hash: Option<&str>,
) -> Result<Sent, SendError> {
    match record(db, id, out, hash).await {
        Ok(()) => Ok(Sent {
            id,
            recipients: out.recipients.len(),
            deduplicated: false,
        }),
        Err(e)
            if hash.is_some()
                && matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) =>
        {
            prior_send(db, out, hash).await?.ok_or(SendError::Db(e))
        }
        Err(e) => Err(SendError::Db(e)),
    }
}

/// The send row, a signal per recipient, and the delivery task — together.
async fn record(
    db: &DatabaseConnection,
    id: Uuid,
    out: &Outgoing,
    hash: Option<&str>,
) -> Result<(), DbErr> {
    let now = Utc::now().fixed_offset();
    let queue = out.workspace_id.filter(|_| !out.recipients.is_empty());
    let run_id = queue.map(|_| Uuid::new_v4().to_string());
    let txn = db.begin().await?;
    notification_sends::ActiveModel {
        id: Set(id),
        org_id: Set(out.org_id),
        app_id: Set(out.app_id()),
        environment: Set(out.environment.clone()),
        kind: Set(out.kind.clone()),
        critical: Set(out.critical),
        email: Set(out.email.is_some()),
        slack: Set(out.slack),
        idempotency_key_sha256: Set(hash.map(str::to_string)),
        recipients: Set(i32::try_from(out.recipients.len()).unwrap_or(i32::MAX)),
        run_id: Set(run_id.clone()),
        created_at: Set(now),
    }
    .insert(&txn)
    .await?;
    if !out.recipients.is_empty() {
        notifications::Entity::insert_many(out.recipients.iter().map(|user| {
            notifications::ActiveModel {
                id: Set(Uuid::new_v4()),
                org_id: Set(out.org_id),
                user_id: Set(*user),
                kind: Set(out.kind.clone()),
                // Empty, not NULL: the previous deploy decodes `title` as a
                // `String`, and a NULL would 500 its inbox mid-rollout.
                title: Set(String::new()),
                body: Set(None),
                link: Set(None),
                subject_kind: Set(None),
                subject_id: Set(None),
                created_at: Set(now),
                read_at: Set(None),
                message_id: Set(Some(id)),
                app_id: Set(out.app_id()),
                thread_key: Set(out.thread_key.clone()),
            }
        }))
        .exec(&txn)
        .await?;
    }
    if let (Some(workspace_id), Some(run_id)) = (queue, run_id.as_deref()) {
        super::task::enqueue(&txn, run_id, id, workspace_id).await?;
    }
    txn.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_is_stored_as_half_a_sha256_per_source_never_as_text() {
        let app = Uuid::from_u128(1);
        let key = thread_key(Some(app), "store:clovis");
        assert_eq!(key.len(), THREAD_KEY_LEN);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(
            key,
            key_hash(&format!("{app}\nstore:clovis"))[..THREAD_KEY_LEN]
        );
        assert!(!key.contains("clovis"));
        assert_eq!(
            key,
            thread_key(Some(app), "store:clovis"),
            "stable, so a thread collapses"
        );
        assert_ne!(key, thread_key(Some(app), "store:fresno"));
        assert_ne!(
            key,
            thread_key(Some(Uuid::from_u128(2)), "store:clovis"),
            "another app's words are another thread"
        );
        assert_ne!(key, thread_key(None, "store:clovis"));
    }
}
