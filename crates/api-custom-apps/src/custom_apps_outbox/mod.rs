//! An environment's outbox: the email and notifications a function sent while
//! its messages plane was `capture`
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.2).
//!
//! A captured message is rendered as it would have gone out, sent to nobody,
//! and written as `_outbox/<inverted millis>-<rfc3339>-<uuid>.json` in the
//! environment's own storage silo; the leading number counts down with time,
//! so the store's ascending listing is newest first and a read never lists
//! more than the page it needs. [`Message::encode`] makes the object, `ProjectFunctionHost`
//! writes it (`host::outbox`, behind the silo's storage limit), and `GET
//! …/environments/{name}/outbox` reads it back ([`list_newest_first`]).
//!
//! The outbox is read from the environment's silo alone ([`silo`]): a key
//! production happens to hold under `_outbox/` is not this environment's mail.

use std::future::Future;

use chrono::{DateTime, SecondsFormat, Utc};
use oxy_app_core::custom_app_environment::AppEnvironment;
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::custom_apps_sandbox_instance::is_of_instance;
use crate::custom_apps_storage::{self as storage, ListPage, Silo, StorageError};

/// Where an environment's captured messages sit, relative to its silo.
pub const PREFIX: &str = "_outbox/";

/// What `GET …/outbox` answers when `?limit=` is absent.
pub const DEFAULT_LIMIT: usize = 50;

/// The most one `GET …/outbox` answers.
pub const MAX_LIMIT: usize = 200;

/// The page size the key walk lists with, at most.
const WALK_PAGE: usize = 1000;

/// What a capture time is subtracted from to make the leading, ascending part
/// of a key: thirteen digits, which holds every millisecond until the year
/// 2286.
const INVERT_FROM: i64 = 9_999_999_999_999;

/// Digits in the inverted part, and so the offset of the time after its `-`.
const INVERTED_WIDTH: usize = 13;

/// Characters of an RFC 3339 time with millisecond precision and a `Z`.
const TIME_WIDTH: usize = 24;

/// The silo `environment` of `app_id` keeps its outbox in: its own, with no
/// read-through to production's. Production has none; it is never asked.
pub fn silo(app_id: Uuid, environment: &AppEnvironment) -> Silo {
    Silo::for_environment(app_id, environment).without_fallback()
}

/// One captured message.
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    /// `email` or `notify`.
    pub kind: &'static str,
    /// Who the call named: addresses for an email, the targets as the call
    /// wrote them for a notification. Nobody was reached.
    pub to: Value,
    /// An email's subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// A notification's title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub body: Option<String>,
    pub invocation_id: Uuid,
    pub function: String,
    pub captured_at: String,
    /// What only one kind has (`cc`, `bcc`, `reply_to`, `format`,
    /// `attachments` for an email; `notify_kind`, `link` for a notification).
    #[serde(flatten)]
    pub details: Map<String, Value>,
}

impl Message {
    /// The pathname (relative to the silo) and bytes of this message captured
    /// at `at` as object `id`: `_outbox/<inverted millis>-<time>-<id>.json`.
    /// The inverted milliseconds are fixed-width and fall as time rises, so a
    /// store that lists keys ascending lists the newest message first; the
    /// readable time follows for people. Storage turns each `:` into `_`.
    pub fn encode(mut self, at: DateTime<Utc>, id: Uuid) -> Result<(String, Vec<u8>), String> {
        self.captured_at = at.to_rfc3339_opts(SecondsFormat::Millis, true);
        let inverted = INVERT_FROM.saturating_sub(at.timestamp_millis()).max(0);
        let pathname = format!(
            "{PREFIX}{inverted:0INVERTED_WIDTH$}-{}-{id}.json",
            self.captured_at
        );
        let bytes = serde_json::to_vec(&self).map_err(|e| format!("outbox: {e}"))?;
        Ok((pathname, bytes))
    }
}

/// Write an encoded message into `silo`. The caller has already asked the
/// silo's storage limit.
pub async fn write(
    silo: &Silo,
    pathname: &str,
    bytes: Vec<u8>,
    retention: &storage::RetentionPolicy,
) -> Result<String, StorageError> {
    let options = storage::PutOptions {
        content_type: Some("application/json".to_string()),
        ..Default::default()
    };
    storage::put(silo, pathname, bytes, options, retention)
        .await
        .map(|put| put.key)
}

/// When the message stored at `key` was captured, read from its name
/// (`_outbox/<inverted millis>-<time>-<id>.json`, where storage turned each
/// `:` of the time into `_`). `None` for a key that is not one of ours.
pub fn captured_at_of_key(key: &str) -> Option<DateTime<Utc>> {
    let name = key.rsplit('/').next()?;
    let (inverted, rest) = name.split_at_checked(INVERTED_WIDTH)?;
    if !inverted.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let time = rest.strip_prefix('-')?.get(..TIME_WIDTH)?.replace('_', ":");
    DateTime::parse_from_rfc3339(&time)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// A page of the outbox, newest first.
#[derive(Debug, Serialize)]
pub struct Page {
    pub messages: Vec<Value>,
    pub has_more: bool,
}

/// The newest `limit` messages of `silo`'s outbox. Keys list newest first (the
/// name's leading number counts down), so the walk stops as soon as it holds
/// `limit + 1` keys: a page costs one storage listing however long the
/// outbox is. Then only the page is read. An object that no longer parses is
/// left out and logged.
///
/// With `since` (a sandbox agent token's view: the instance of the sandbox,
/// or the grant of staging, it holds), a message captured before it is not
/// this reader's and is never counted toward the limit or `has_more`, so a
/// page is never short because of it. The first such message ends the walk,
/// since every key after it is older still. A name that carries no time
/// belongs to no instance and is skipped.
pub async fn list_newest_first(
    silo: &Silo,
    limit: usize,
    since: Option<DateTime<Utc>>,
) -> Result<Page, StorageError> {
    let page_size = limit.saturating_add(1).clamp(1, WALK_PAGE);
    let keys = newest_keys(limit, since, |cursor| {
        storage::list(silo, Some(PREFIX), Some(page_size), cursor)
    })
    .await?;
    let has_more = keys.len() > limit;
    let mut messages = Vec::with_capacity(limit.min(keys.len()));
    for key in keys.iter().take(limit) {
        let Some((bytes, _)) = storage::get(silo, key).await? else {
            continue; // swept between the listing and the read
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(message) => messages.push(message),
            Err(error) => tracing::warn!(%key, %error, "outbox: object is not JSON; left out"),
        }
    }
    Ok(Page { messages, has_more })
}

/// Up to `limit + 1` keys of this reader's, newest first, from `list_page`
/// (one storage page per call, ascending by key, given the cursor the last
/// one returned). One more than `limit` is what `has_more` is decided on.
async fn newest_keys<F, Fut>(
    limit: usize,
    since: Option<DateTime<Utc>>,
    mut list_page: F,
) -> Result<Vec<String>, StorageError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<ListPage, StorageError>>,
{
    let wanted = limit.saturating_add(1);
    let mut keys = Vec::new();
    let mut cursor = None;
    loop {
        let page = list_page(cursor).await?;
        for object in page.objects {
            if let Some(since) = since {
                match captured_at_of_key(&object.key) {
                    Some(at) if is_of_instance(at, since) => {}
                    Some(_) => return Ok(keys), // newest first: the rest is older
                    None => continue,
                }
            }
            keys.push(object.key);
            if keys.len() >= wanted {
                return Ok(keys);
            }
        }
        match page.cursor {
            Some(next) if page.has_more => cursor = Some(next),
            _ => return Ok(keys),
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
