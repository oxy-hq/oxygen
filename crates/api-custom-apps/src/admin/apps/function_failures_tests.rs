use chrono::{DateTime, Duration, TimeZone, Utc};
use uuid::Uuid;

use super::{
    Cursor, DEFAULT_LIMIT, FailuresQuery, MAX_LIMIT, MAX_WINDOW_DAYS, Request, WINDOW_SKEW_MINUTES,
};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
}

fn query(since: Option<&str>, until: Option<&str>) -> FailuresQuery {
    FailuresQuery {
        since: since.map(str::to_string),
        until: until.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn no_parameters_is_the_last_day_at_the_default_limit() {
    let r = Request::from_query(&FailuresQuery::default(), now()).unwrap();
    assert_eq!(r.until, now());
    assert_eq!(r.since, now() - Duration::hours(24));
    assert_eq!(r.limit, DEFAULT_LIMIT);
    assert_eq!(r.after, None);
}

#[test]
fn since_alone_reads_up_to_now() {
    let r = Request::from_query(&query(Some("2026-10-28T19:00:00Z"), None), now()).unwrap();
    assert_eq!(
        r.since,
        Utc.with_ymd_and_hms(2026, 10, 28, 19, 0, 0).unwrap()
    );
    assert_eq!(r.until, now());
}

#[test]
fn an_offset_is_read_as_the_instant_it_names() {
    let r = Request::from_query(
        &query(
            Some("2026-10-29T02:56:00+07:00"),
            Some("2026-10-29T05:00:00+07:00"),
        ),
        now(),
    )
    .unwrap();
    assert_eq!(
        r.since,
        Utc.with_ymd_and_hms(2026, 10, 28, 19, 56, 0).unwrap()
    );
}

#[test]
fn a_window_that_is_empty_backwards_or_too_wide_is_refused() {
    for (since, until) in [
        ("2026-10-29T00:00:00Z", "2026-10-29T00:00:00Z"),
        ("2026-10-29T01:00:00Z", "2026-10-29T00:00:00Z"),
        ("2026-10-01T00:00:00Z", "2026-10-29T00:00:00Z"),
    ] {
        let err = Request::from_query(&query(Some(since), Some(until)), now()).unwrap_err();
        assert_eq!(err.code, "invalid_window", "{since}..{until}");
    }
    // Exactly the cap is allowed.
    let until = now();
    let since = until - Duration::days(MAX_WINDOW_DAYS);
    let q = query(Some(&since.to_rfc3339()), Some(&until.to_rfc3339()));
    assert!(Request::from_query(&q, now()).is_ok());
}

/// "The last 7 days", computed on the caller's clock, reaches a server whose
/// clock is a little later. With `until` defaulted to the server's now, the
/// window is just over 7 days, and refusing it refused the obvious request.
#[test]
fn the_last_seven_days_from_a_clock_a_moment_behind_is_accepted() {
    let callers_since = now() - Duration::days(MAX_WINDOW_DAYS) - Duration::seconds(3);
    let q = query(Some(&callers_since.to_rfc3339()), None);
    let r = Request::from_query(&q, now()).expect("the last 7 days");
    assert_eq!(r.since, callers_since);
    assert_eq!(r.until, now());

    // Past the skew allowance it is a wider window, and refused.
    let too_wide =
        now() - Duration::days(MAX_WINDOW_DAYS) - Duration::minutes(WINDOW_SKEW_MINUTES + 1);
    let q = query(Some(&too_wide.to_rfc3339()), None);
    assert_eq!(
        Request::from_query(&q, now()).unwrap_err().code,
        "invalid_window"
    );
}

#[test]
fn a_time_that_is_not_rfc_3339_is_refused() {
    let err = Request::from_query(&query(Some("yesterday"), None), now()).unwrap_err();
    assert_eq!(err.code, "invalid_time");
}

#[test]
fn limit_out_of_range_is_refused_not_clamped() {
    for limit in [0, MAX_LIMIT + 1] {
        let q = FailuresQuery {
            limit: Some(limit),
            ..Default::default()
        };
        assert_eq!(
            Request::from_query(&q, now()).unwrap_err().code,
            "invalid_limit"
        );
    }
    let q = FailuresQuery {
        limit: Some(MAX_LIMIT),
        ..Default::default()
    };
    assert_eq!(Request::from_query(&q, now()).unwrap().limit, MAX_LIMIT);
}

#[test]
fn a_cursor_round_trips_to_the_microsecond() {
    let cursor = Cursor {
        created_at: Utc.timestamp_micros(1_793_225_828_108_219).unwrap(),
        id: Uuid::new_v4(),
    };
    assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
    let q = FailuresQuery {
        cursor: Some(cursor.encode()),
        ..Default::default()
    };
    assert_eq!(Request::from_query(&q, now()).unwrap().after, Some(cursor));
}

#[test]
fn a_cursor_this_endpoint_did_not_mint_is_refused() {
    for raw in [
        "abc",
        "123",
        "123.not-a-uuid",
        "x.7c9e6679-7425-40de-944b-e07fc1f90ae7",
    ] {
        let q = FailuresQuery {
            cursor: Some(raw.into()),
            ..Default::default()
        };
        assert_eq!(
            Request::from_query(&q, now()).unwrap_err().code,
            "invalid_cursor",
            "{raw}"
        );
    }
}

/// A caller whose clock runs ahead sends an `until` a moment in our future.
/// Nothing compares `until` with our clock, so that is just a window.
#[test]
fn an_until_a_moment_in_the_future_is_accepted() {
    let until = now() + Duration::seconds(3);
    let since = until - Duration::days(MAX_WINDOW_DAYS);
    let q = query(Some(&since.to_rfc3339()), Some(&until.to_rfc3339()));
    let r = Request::from_query(&q, now()).expect("a clock ahead of ours");
    assert_eq!(r.until, until);
}
