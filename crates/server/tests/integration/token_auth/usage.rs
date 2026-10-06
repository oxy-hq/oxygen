//! Per-token usage (design §3.7, layer 3): the middleware counts what a key
//! did, by route template, and a flush writes one row per token per day.

use super::*;
use axum::http::StatusCode;
use chrono::Duration;
use entity::api_token_usage_daily as usage_daily;
use oxy_auth::token::usage::{self, UsageSample};
use oxy_telemetry::http_trace::RequestTokenId;
use sea_orm::ConnectionTrait;

async fn usage_rows(db: &DatabaseConnection, token_id: Uuid) -> Vec<usage_daily::Model> {
    usage_daily::Entity::find()
        .filter(usage_daily::Column::TokenId.eq(token_id))
        .all(db)
        .await
        .unwrap()
}

async fn get_as(fx: &Fixture, router: Router, path: &str, headers: &[(&str, &str)]) -> StatusCode {
    let uri = format!("/{}/{path}", fx.workspace_id);
    call(router, "GET", &uri, headers, None).await.0
}

#[tokio::test]
async fn usage_counts_2xx_4xx_and_5xx_and_records_the_route_template() {
    let fx = fixture().await;
    let (id, key) = legacy_key(&fx, None).await;
    let headers = [
        ("x-api-key", key.as_str()),
        // The caller wrote the first hop; the load balancer appended the second.
        ("x-forwarded-for", "6.6.6.6, 203.0.113.4"),
        ("user-agent", "oxyc/0.5.0"),
    ];
    for (path, expected) in [
        ("probe", StatusCode::OK),
        ("probe", StatusCode::OK),
        ("teapot", StatusCode::IM_A_TEAPOT),
        ("boom", StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        assert_eq!(get_as(&fx, api_surface(), path, &headers).await, expected);
    }
    // The other surface feeds the same counters.
    assert_eq!(
        get_as(&fx, external_surface(), "probe", &headers).await,
        StatusCode::OK
    );
    // A session is not a token, and a refused key names none: neither counts.
    assert_eq!(
        get_as(&fx, api_surface(), "probe", &[("cookie", &fx.cookie)]).await,
        StatusCode::OK
    );
    let bad = [("x-api-key", "oxy_00000000000000000000000000000000")];
    assert_eq!(
        get_as(&fx, api_surface(), "probe", &bad).await,
        StatusCode::UNAUTHORIZED
    );

    assert!(
        usage_rows(&fx.db, id).await.is_empty(),
        "nothing before a flush"
    );
    assert_eq!(usage::flush(&fx.db).await.expect("flush"), 1);

    let rows = usage_rows(&fx.db, id).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.day, Utc::now().date_naive());
    assert_eq!(row.requests, 5);
    assert_eq!(row.errors_4xx, 1);
    assert_eq!(row.errors_5xx, 1);
    assert_eq!(
        row.last_ip.as_deref(),
        Some("203.0.113.4"),
        "the hop the load balancer appended, not the one the caller wrote"
    );
    assert_eq!(row.last_user_agent.as_deref(), Some("oxyc/0.5.0"));
    // The matched template, never the raw path (which carries the workspace id).
    let route = row.last_route.as_deref().expect("route");
    assert_eq!(route, "/{workspace_id}/probe");
    assert!(!route.contains(&fx.workspace_id.to_string()));

    // A second flush with nothing new writes nothing; more traffic adds.
    assert_eq!(usage::flush(&fx.db).await.unwrap(), 0);
    assert_eq!(
        get_as(&fx, api_surface(), "boom", &headers).await.as_u16(),
        500
    );
    usage::flush(&fx.db).await.unwrap();
    let row = &usage_rows(&fx.db, id).await[0];
    assert_eq!((row.requests, row.errors_4xx, row.errors_5xx), (6, 1, 2));
    assert_eq!(row.last_route.as_deref(), Some("/{workspace_id}/boom"));
}

#[tokio::test]
async fn the_response_carries_the_token_id_for_the_request_span() {
    let fx = fixture().await;
    let (id, key) = legacy_key(&fx, None).await;
    let uri = format!("/{}/probe", fx.workspace_id);
    let send = |name: &'static str, value: String| {
        let req = Request::builder()
            .uri(&uri)
            .header(name, value)
            .body(Body::empty())
            .unwrap();
        api_surface().oneshot(req)
    };

    let keyed = send("x-api-key", key.clone()).await.unwrap();
    assert_eq!(
        keyed.extensions().get::<RequestTokenId>(),
        Some(&RequestTokenId(id.to_string())),
        "the id, which the trace layer records as oxy.token_id"
    );
    assert_ne!(id.to_string(), key, "the id is not the token");

    let session = send("cookie", fx.cookie.clone()).await.unwrap();
    assert!(session.extensions().get::<RequestTokenId>().is_none());
}

#[tokio::test]
async fn a_token_with_no_row_is_skipped_not_retried_forever() {
    let fx = fixture().await;
    usage::record(UsageSample {
        token_id: Uuid::new_v4(), // no api_tokens row: the FK would refuse it
        status: 200,
        ip: None,
        user_agent: None,
        route: None,
        at: Utc::now(),
    });
    usage::flush(&fx.db).await.expect("skipped, not an error");
    assert!(!usage::has_pending(), "not put back");
}

/// One request a day for `days` days, the newest at `newest`: a different
/// `(token, day)` row per day.
fn record_days(token_id: Uuid, newest: DateTime<Utc>, days: i64, status: u16, route: &str) {
    for days_ago in 0..days {
        usage::record(UsageSample {
            token_id,
            status,
            ip: Some("203.0.113.4".into()),
            user_agent: Some("oxyc/0.5.0".into()),
            route: Some(route.into()),
            at: newest - Duration::days(days_ago),
        });
    }
}

#[tokio::test]
async fn one_flush_writes_many_token_days_and_the_next_adds_to_them() {
    let fx = fixture().await;
    // A legacy key the endpoint minted (so it has its mirror) and a new token.
    let (legacy, _) = endpoint_key(&fx, None).await;
    let (pat, _) = minted_pat(&fx, None).await;
    let gone = Uuid::new_v4(); // no api_tokens row
    // 7,400 rows of nine values is more than one statement can bind at all
    // (65,535), so whatever the batch size this flush is several statements.
    const DAYS: i64 = 3_700;
    // Fixed once, so both flushes name the same days whenever the test runs.
    let noon = Utc::now().date_naive().and_hms_opt(12, 0, 0).unwrap();
    let noon = noon.and_utc();
    for id in [legacy, pat] {
        record_days(id, noon, DAYS, 200, "/first");
    }
    record_days(gone, noon, 50, 200, "/first");

    let sent = usage::flush(&fx.db).await.expect("flush");
    assert_eq!(
        sent,
        2 * DAYS as usize + 50,
        "the skipped rows are sent too"
    );
    assert!(!usage::has_pending());
    assert!(usage_rows(&fx.db, gone).await.is_empty(), "skipped");
    for id in [legacy, pat] {
        let rows = usage_rows(&fx.db, id).await;
        assert_eq!(rows.len(), DAYS as usize, "one row per day, none lost");
        assert!(rows.iter().all(|r| (r.requests, r.errors_4xx) == (1, 0)));
        assert!(
            rows.iter()
                .all(|r| r.last_route.as_deref() == Some("/first"))
        );
    }

    // A later request on every day adds and moves "last"; an earlier one adds
    // and leaves "last" alone. Both land on rows the first flush wrote.
    record_days(legacy, noon + Duration::hours(1), DAYS, 404, "/later");
    record_days(pat, noon - Duration::hours(1), DAYS, 500, "/earlier");
    assert_eq!(
        usage::flush(&fx.db).await.expect("flush"),
        2 * DAYS as usize
    );

    let rows = usage_rows(&fx.db, legacy).await;
    assert_eq!(rows.len(), DAYS as usize);
    assert!(rows.iter().all(|r| (r.requests, r.errors_4xx) == (2, 1)));
    assert!(
        rows.iter()
            .all(|r| r.last_route.as_deref() == Some("/later"))
    );
    let rows = usage_rows(&fx.db, pat).await;
    assert_eq!(rows.len(), DAYS as usize);
    assert!(rows.iter().all(|r| (r.requests, r.errors_5xx) == (2, 1)));
    assert!(
        rows.iter()
            .all(|r| r.last_route.as_deref() == Some("/first"))
    );
}

#[tokio::test]
async fn usage_older_than_the_retention_window_is_pruned() {
    let fx = fixture().await;
    let (id, _) = minted_pat(&fx, None).await;
    let today = Utc::now().date_naive();
    for day in [
        today,
        today - Duration::days(usage::USAGE_RETENTION_DAYS + 5),
    ] {
        fx.db
            .execute_unprepared(&format!(
                "INSERT INTO api_token_usage_daily (token_id, day, requests) \
                 VALUES ('{id}', '{day}', 1)"
            ))
            .await
            .unwrap();
    }
    let pruned = usage::prune_older_than(&fx.db, usage::USAGE_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(pruned, 1);
    let left = usage_rows(&fx.db, id).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].day, today);
}
