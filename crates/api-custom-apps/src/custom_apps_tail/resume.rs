//! A resumed tail (`Last-Event-ID`): what it replays.
//!
//! Lines are never stored for the tail, so a stream reopened after a dropped
//! connection cannot be given the lines it missed. What it is given is each
//! invocation's **outcome**, read back from `app_function_invocations`: one
//! `finish` (marked `replayed`) for every invocation of the environment that
//! finished after the last event the reader saw — at most [`LIMIT`], from at
//! most [`WINDOW`] ago.
//!
//! An event id is the stream's clock in milliseconds (`stream::EventIds`),
//! so "after the last event" is a time. The replica that held the old stream
//! and the one holding the new may disagree by their clock skew; an outcome
//! near the boundary can arrive twice, once live and once replayed, and the
//! `replayed` flag is what lets a reader drop the second.

use axum::http::HeaderMap;
use chrono::{DateTime, Duration, TimeZone, Utc};
use entity::app_function_invocations::{Column, Entity, Model};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use super::event::{Outcome, TailEvent};

/// The most outcomes one resume replays: the newest, when there are more.
pub const LIMIT: usize = 200;

/// How far back a resume reaches, whatever id it names.
pub const WINDOW: Duration = Duration::minutes(10);

/// A function's longest possible run (`custom_apps_functions`'s
/// `TIMEOUT_CEILING_SECS`) and a margin: how much earlier than the resume
/// point an invocation that finished after it can have started.
const LONGEST_RUN: Duration = Duration::seconds(300 + 60);

/// Rows read to find the outcomes: the index on `(app_id, environment,
/// created_at)` serves the read, and this bounds it on a busy environment.
const SCAN: u64 = 1_000;

/// The resume point a request names, when its `Last-Event-ID` is one of
/// this stream's ids. Anything else is a fresh stream.
pub fn last_event_id(headers: &HeaderMap) -> Option<i64> {
    headers
        .get("last-event-id")?
        .to_str()
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}

/// What a resume replays: its outcomes, oldest first, and how many newer
/// than the point it had to leave out.
#[derive(Debug, Default)]
pub struct Replay {
    pub outcomes: Vec<TailEvent>,
    pub left_out: usize,
}

/// The outcomes of `environment` of `app_id` that finished after `after_ms`
/// (or [`WINDOW`] ago, whichever is later), of invocations created at or
/// after `since` when that is given.
pub async fn replay(
    db: &impl ConnectionTrait,
    app_id: Uuid,
    environment: &str,
    after_ms: i64,
    since: Option<DateTime<Utc>>,
) -> Result<Replay, DbErr> {
    let floor = Utc::now() - WINDOW;
    let after = Utc
        .timestamp_millis_opt(after_ms)
        .single()
        .map_or(floor, |at| at.max(floor));
    let mut query = Entity::find()
        .filter(Column::AppId.eq(app_id))
        .filter(Column::Environment.eq(environment))
        .filter(Column::Status.ne("running"))
        .filter(Column::CreatedAt.gt(after - LONGEST_RUN));
    if let Some(since) = since {
        query = query.filter(Column::CreatedAt.gte(since));
    }
    let rows = query
        .order_by_desc(Column::CreatedAt)
        .limit(SCAN)
        .all(db)
        .await?;
    Ok(select(rows, after))
}

/// When a row's invocation finished.
fn finished_at(row: &Model) -> DateTime<Utc> {
    let ran = Duration::milliseconds(row.duration_ms.unwrap_or(0));
    row.created_at.with_timezone(&Utc) + ran
}

/// The rows that finished after `after`, oldest first, the newest [`LIMIT`].
fn select(rows: Vec<Model>, after: DateTime<Utc>) -> Replay {
    let mut finished: Vec<(DateTime<Utc>, Model)> = rows
        .into_iter()
        .map(|row| (finished_at(&row), row))
        .filter(|(at, _)| *at > after)
        .collect();
    finished.sort_by_key(|(at, _)| *at);
    let left_out = finished.len().saturating_sub(LIMIT);
    let outcomes = finished
        .into_iter()
        .skip(left_out)
        .map(|(_, row)| outcome_of(&row))
        .collect();
    Replay { outcomes, left_out }
}

fn outcome_of(row: &Model) -> TailEvent {
    let outcome = Outcome {
        status: row.status.clone(),
        // What the invocation listing reads back as a failure
        // (`admin::apps::invocations::counted_as_failure`, which this
        // surface may not import): the pager's fingerprint, or a status that
        // predates it.
        failed: row.failure_fingerprint.is_some()
            || matches!(row.status.as_str(), "error" | "timeout"),
        // Stored for keyed calls only; `0` reads as "not recorded".
        http_status: row
            .result_status
            .and_then(|s| u16::try_from(s).ok())
            .unwrap_or(0),
        duration_ms: row
            .duration_ms
            .and_then(|ms| u64::try_from(ms).ok())
            .unwrap_or(0),
        error: row.error.clone(),
    };
    let mut event = TailEvent::finish(row.id, &row.function_name, &row.mode, &outcome);
    if let TailEvent::Finish { replayed, .. } = &mut event {
        *replayed = true;
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(created: DateTime<Utc>, duration_ms: i64, status: &str) -> Model {
        Model {
            id: Uuid::new_v4(),
            app_id: Uuid::nil(),
            build_id: Uuid::nil(),
            function_name: "probe".into(),
            mode: "route".into(),
            user_id: None,
            status: status.into(),
            duration_ms: Some(duration_ms),
            error: None,
            cancel_requested_at: None,
            created_at: created.into(),
            idempotency_key: None,
            result_body: None,
            result_status: None,
            request_hash: None,
            failure_fingerprint: None,
            environment: "dev-a1".into(),
            credential_token_id: None,
            error_kind: None,
            parent_invocation_id: None,
        }
    }

    /// Only an id this stream could have sent resumes it.
    #[test]
    fn a_last_event_id_is_a_positive_millisecond_time() {
        let with = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("last-event-id", value.parse().unwrap());
            last_event_id(&headers)
        };
        assert_eq!(with("1760000000000"), Some(1_760_000_000_000));
        assert_eq!(with(" 42 "), Some(42));
        for bad in ["", "0", "-5", "abc", "1.5"] {
            assert_eq!(with(bad), None, "{bad:?}");
        }
        assert_eq!(last_event_id(&HeaderMap::new()), None);
    }

    /// An invocation counts by when it finished, not when it started: one
    /// that started before the point and finished after it is replayed; one
    /// that finished before it is not. Oldest first, each marked replayed.
    #[test]
    fn outcomes_that_finished_after_the_point_are_replayed_in_order() {
        let after = Utc::now() - Duration::minutes(1);
        let rows = vec![
            row(after + Duration::seconds(5), 10, "success"),
            row(after - Duration::seconds(2), 5_000, "error"),
            row(after - Duration::seconds(10), 1_000, "success"),
        ];
        let replay = select(rows, after);
        assert_eq!(replay.left_out, 0);
        let statuses: Vec<(&str, bool, bool)> = replay
            .outcomes
            .iter()
            .map(|e| match e {
                TailEvent::Finish {
                    status,
                    failed,
                    replayed,
                    ..
                } => (status.as_str(), *failed, *replayed),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(statuses, [("error", true, true), ("success", false, true)]);
    }

    /// Past the limit, the newest are kept and the rest are counted.
    #[test]
    fn a_resume_keeps_the_newest_outcomes_past_the_limit() {
        let after = Utc::now() - Duration::minutes(5);
        let rows = (0..LIMIT + 7)
            .map(|i| row(after + Duration::milliseconds(i as i64 + 1), 0, "success"))
            .collect();
        let replay = select(rows, after);
        assert_eq!((replay.outcomes.len(), replay.left_out), (LIMIT, 7));
    }
}
