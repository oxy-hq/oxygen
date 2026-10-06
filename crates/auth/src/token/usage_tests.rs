//! Unit tests for the usage accumulator: counting, "last seen", the day key,
//! drain/restore, and the bounds.

use super::*;
use chrono::{Duration, TimeZone};

fn at(h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 30, h, m, 0).unwrap()
}

fn sample(token_id: Uuid, status: u16, when: DateTime<Utc>) -> UsageSample {
    UsageSample {
        token_id,
        status,
        ip: Some("203.0.113.4".into()),
        user_agent: Some("oxyc/0.5.0".into()),
        route: Some("/api/{workspace_id}/sql/query".into()),
        at: when,
    }
}

fn only(acc: &mut UsageAccumulator) -> UsageRow {
    let (mut rows, _) = acc.drain();
    assert_eq!(rows.len(), 1);
    rows.pop().unwrap()
}

#[test]
fn counts_requests_and_splits_4xx_from_5xx() {
    let mut acc = UsageAccumulator::default();
    let t = Uuid::new_v4();
    for status in [200, 201, 204, 302, 400, 401, 404, 429, 500, 503] {
        acc.record(sample(t, status, at(10, 0)));
    }
    let row = only(&mut acc);
    assert_eq!(row.requests, 10);
    assert_eq!(row.errors_4xx, 4);
    assert_eq!(row.errors_5xx, 2);
}

#[test]
fn a_2xx_or_3xx_is_not_an_error() {
    let mut acc = UsageAccumulator::default();
    let t = Uuid::new_v4();
    for status in [200, 299, 301, 399] {
        acc.record(sample(t, status, at(10, 0)));
    }
    let row = only(&mut acc);
    assert_eq!((row.requests, row.errors_4xx, row.errors_5xx), (4, 0, 0));
}

#[test]
fn last_fields_follow_the_newest_request_whatever_the_arrival_order() {
    let mut acc = UsageAccumulator::default();
    let t = Uuid::new_v4();
    let mut newest = sample(t, 200, at(12, 0));
    newest.ip = Some("198.51.100.7".into());
    newest.route = Some("/api/{workspace_id}/threads".into());
    acc.record(sample(t, 200, at(10, 0)));
    acc.record(newest);
    acc.record(sample(t, 200, at(11, 0))); // arrives late, is older
    let row = only(&mut acc);
    assert_eq!(row.last_seen_at, at(12, 0));
    assert_eq!(row.last_ip.as_deref(), Some("198.51.100.7"));
    assert_eq!(
        row.last_route.as_deref(),
        Some("/api/{workspace_id}/threads")
    );
    assert_eq!(row.requests, 3);
}

#[test]
fn the_route_is_recorded_as_given_and_absent_when_unrouted() {
    let mut acc = UsageAccumulator::default();
    let t = Uuid::new_v4();
    let mut unrouted = sample(t, 404, at(10, 0));
    unrouted.route = None;
    acc.record(unrouted);
    assert_eq!(only(&mut acc).last_route, None);
}

#[test]
fn rows_are_keyed_by_token_and_utc_day() {
    let mut acc = UsageAccumulator::default();
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    acc.record(sample(a, 200, at(23, 59)));
    acc.record(sample(a, 200, at(23, 59) + Duration::minutes(2))); // next UTC day
    acc.record(sample(b, 200, at(23, 59)));
    let (rows, _) = acc.drain();
    assert_eq!(rows.len(), 3);
    let a_days: Vec<NaiveDate> = rows
        .iter()
        .filter(|r| r.token_id == a)
        .map(|r| r.day)
        .collect();
    assert_eq!(a_days.len(), 2);
    assert_ne!(a_days[0], a_days[1]);
}

#[test]
fn drain_empties_the_accumulator() {
    let mut acc = UsageAccumulator::default();
    acc.record(sample(Uuid::new_v4(), 200, at(10, 0)));
    assert_eq!(acc.drain().0.len(), 1);
    assert_eq!(acc.len(), 0);
    assert!(acc.drain().0.is_empty());
}

#[test]
fn a_failed_flush_is_restored_and_merged_with_what_arrived_since() {
    let mut acc = UsageAccumulator::default();
    let t = Uuid::new_v4();
    acc.record(sample(t, 200, at(10, 0)));
    acc.record(sample(t, 500, at(10, 1)));
    let (unwritten, _) = acc.drain();

    // Traffic continues while the flush is failing.
    let mut later = sample(t, 404, at(10, 5));
    later.ip = Some("198.51.100.7".into());
    acc.record(later);

    acc.restore(unwritten);
    let row = only(&mut acc);
    assert_eq!(row.requests, 3, "nothing counted twice, nothing lost");
    assert_eq!((row.errors_4xx, row.errors_5xx), (1, 1));
    assert_eq!(row.last_seen_at, at(10, 5));
    assert_eq!(row.last_ip.as_deref(), Some("198.51.100.7"));
}

#[test]
fn restore_into_an_empty_accumulator_keeps_the_row() {
    let mut acc = UsageAccumulator::default();
    let t = Uuid::new_v4();
    acc.record(sample(t, 200, at(10, 0)));
    let (unwritten, _) = acc.drain();
    acc.restore(unwritten.clone());
    assert_eq!(acc.drain().0, unwritten);
}

#[test]
fn memory_is_bounded_and_drops_are_counted() {
    let mut acc = UsageAccumulator::default();
    for _ in 0..(MAX_PENDING_ROWS + 7) {
        acc.record(sample(Uuid::new_v4(), 200, at(10, 0)));
    }
    assert_eq!(acc.len(), MAX_PENDING_ROWS);
    // A token already present keeps counting at the cap.
    let (rows, dropped) = acc.drain();
    assert_eq!(rows.len(), MAX_PENDING_ROWS);
    assert_eq!(dropped, 7);
    assert_eq!(acc.drain().1, 0, "the drop counter resets");
}

#[test]
fn a_known_token_still_counts_at_the_cap() {
    let mut acc = UsageAccumulator::default();
    let known = Uuid::new_v4();
    acc.record(sample(known, 200, at(10, 0)));
    for _ in 0..MAX_PENDING_ROWS {
        acc.record(sample(Uuid::new_v4(), 200, at(10, 0)));
    }
    acc.record(sample(known, 200, at(10, 1)));
    let (rows, _) = acc.drain();
    let row = rows.iter().find(|r| r.token_id == known).unwrap();
    assert_eq!(row.requests, 2);
}

#[test]
fn stored_strings_are_truncated() {
    let mut acc = UsageAccumulator::default();
    let mut s = sample(Uuid::new_v4(), 200, at(10, 0));
    s.user_agent = Some("u".repeat(5000));
    s.ip = Some("9".repeat(500));
    s.route = Some("/r".repeat(500));
    acc.record(s);
    let row = only(&mut acc);
    assert_eq!(row.last_user_agent.unwrap().len(), MAX_USER_AGENT_CHARS);
    assert_eq!(row.last_ip.unwrap().len(), MAX_IP_CHARS);
    assert_eq!(row.last_route.unwrap().len(), MAX_ROUTE_CHARS);
}

#[test]
fn the_upsert_adds_and_never_names_a_token_secret() {
    let sql = upsert_sql(1);
    // Additive, so two pods flushing the same token-day sum.
    assert!(sql.contains("api_token_usage_daily.requests + EXCLUDED.requests"));
    assert!(sql.contains("WHERE EXISTS (SELECT 1 FROM api_tokens"));
    assert!(!sql.contains("token_hash"));
}

/// `tokens` tokens, each seen on `days` days: `tokens * days` rows, drained.
fn drained(tokens: usize, days: i64) -> Vec<UsageRow> {
    let mut acc = UsageAccumulator::default();
    for _ in 0..tokens {
        let token = Uuid::new_v4();
        for day in 0..days {
            acc.record(sample(token, 200, at(10, 0) - Duration::days(day)));
            acc.record(sample(token, 404, at(10, 1) - Duration::days(day)));
        }
    }
    acc.drain().0
}

fn statements(rows: &[UsageRow]) -> Vec<Statement> {
    rows.chunks(ROWS_PER_STATEMENT)
        .map(upsert_statement)
        .collect()
}

fn bound(statement: &Statement) -> &[Value] {
    &statement.values.as_ref().expect("bound values").0
}

/// Every row is bound once, in order, with its own counts beside its key.
fn assert_binds(statements: &[Statement], rows: &[UsageRow]) {
    let all: Vec<&Value> = statements.iter().flat_map(bound).collect();
    assert_eq!(all.len(), rows.len() * ROW_CASTS.len());
    for (row, values) in rows.iter().zip(all.chunks(ROW_CASTS.len())) {
        assert_eq!(*values[0], Value::from(row.token_id));
        assert_eq!(*values[1], Value::from(row.day));
        assert_eq!(*values[2], Value::from(row.requests));
        assert_eq!(*values[3], Value::from(row.errors_4xx));
        assert_eq!(*values[8], Value::from(row.last_seen_at.fixed_offset()));
    }
}

#[test]
fn each_row_is_one_tuple_numbered_on_from_the_last() {
    let sql = upsert_sql(2);
    assert!(sql.contains(
        "($1::uuid, $2::date, $3::bigint, $4::bigint, $5::bigint, \
         $6::text, $7::text, $8::text, $9::timestamptz),"
    ));
    assert!(sql.contains(
        "($10::uuid, $11::date, $12::bigint, $13::bigint, $14::bigint, \
         $15::text, $16::text, $17::text, $18::timestamptz)\n"
    ));
    assert!(!sql.contains("$19"));
}

#[test]
fn a_drained_batch_holds_each_token_day_once() {
    // One statement cannot update a row twice, so this is what lets a whole
    // batch go in one: repeats and a restored row all fold into their key.
    let mut acc = UsageAccumulator::default();
    let t = Uuid::new_v4();
    for minute in 0..5 {
        acc.record(sample(t, 200, at(10, minute)));
    }
    let (unwritten, _) = acc.drain();
    acc.record(sample(t, 200, at(11, 0)));
    acc.record(sample(Uuid::new_v4(), 200, at(11, 0)));
    acc.restore(unwritten);

    let (rows, _) = acc.drain();
    let keys: std::collections::HashSet<_> = rows.iter().map(|r| (r.token_id, r.day)).collect();
    assert_eq!((rows.len(), keys.len()), (2, 2));
}

#[test]
fn a_flush_of_many_tokens_is_one_statement() {
    let rows = drained(250, 1);
    assert_eq!(rows.len(), 250);
    let statements = statements(&rows);
    assert_eq!(statements.len(), 1, "one round trip, not one per row");
    assert_eq!(statements[0].sql, upsert_sql(250));
    assert_binds(&statements, &rows);
    assert!(rows.iter().all(|r| (r.requests, r.errors_4xx) == (2, 1)));
}

#[test]
fn a_flush_larger_than_one_statement_is_split_under_the_bind_limit() {
    let rows = drained(3, 669);
    assert_eq!(rows.len(), 2 * ROWS_PER_STATEMENT + 7);
    let statements = statements(&rows);
    let sizes: Vec<usize> = statements.iter().map(|s| bound(s).len()).collect();
    let full = ROWS_PER_STATEMENT * ROW_CASTS.len();
    assert_eq!(sizes, [full, full, 7 * ROW_CASTS.len()]);
    assert!(sizes.iter().all(|size| *size <= MAX_BIND_PARAMS));
    // Each statement numbers its own parameters from `$1`.
    assert!(
        statements[0]
            .sql
            .contains(&format!("${full}::timestamptz)"))
    );
    assert!(!statements[0].sql.contains(&format!("${}::", full + 1)));
    assert_eq!(statements[2].sql, upsert_sql(7));
    // Nothing lost and nothing sent twice across the split.
    assert_binds(&statements, &rows);
}

#[tokio::test]
async fn a_failed_statement_leaves_exactly_the_rows_before_it_written() {
    let rows = drained(3, 669);
    let sent = std::cell::Cell::new(0);
    let outcome = write_rows(&rows, |_statement| {
        let nth = sent.replace(sent.get() + 1);
        async move {
            match nth {
                1 => Err(DbErr::Custom("connection reset".into())),
                _ => Ok(()),
            }
        }
    })
    .await;

    let (written, _) = outcome.expect_err("the second statement failed");
    assert_eq!(written, ROWS_PER_STATEMENT, "the first statement landed");
    assert_eq!(sent.get(), 2, "nothing is sent past the failure");

    // What goes back is every row from the failed statement on, and no other.
    let mut acc = UsageAccumulator::default();
    let mut unwritten = rows.clone();
    acc.restore(unwritten.split_off(written));
    let (restored, _) = acc.drain();
    assert_eq!(restored.len(), ROWS_PER_STATEMENT + 7);
    let landed: std::collections::HashSet<_> =
        unwritten.iter().map(|r| (r.token_id, r.day)).collect();
    assert!(
        restored
            .iter()
            .all(|r| !landed.contains(&(r.token_id, r.day)))
    );
}

#[tokio::test]
async fn every_statement_succeeding_writes_every_row() {
    let rows = drained(3, 669);
    let sent = std::cell::Cell::new(0);
    let outcome = write_rows(&rows, |statement| {
        sent.set(sent.get() + bound(&statement).len() / ROW_CASTS.len());
        async { Ok(()) }
    })
    .await;
    assert!(outcome.is_ok());
    assert_eq!(sent.get(), rows.len());
}
