//! `GET /api/frontline/device` and the kiosk hint cookie, against a real
//! database.
//!
//! The web app's kiosk sign-out check (`useKioskSessionGuard`) only asks the
//! probe on a browser it has reason to think is a kiosk. Its first reason, a
//! flag in `localStorage`, is per origin, and an org subdomain never runs the
//! pages that set it. The second is `oxy_kiosk_hint`: page-readable, with the
//! same `Domain`, `Path` and `Max-Age` as `oxy_kiosk`, so every origin the
//! kiosk cookie covers can see it. It only switches the check on; the probe
//! still answers.
//!
//! Two promises about the probe itself. A database error is a 503 — the
//! client's "unknown changes nothing" path — never the `{"bound": false}` that
//! reads as "not a kiosk". And a browser that is genuinely no kiosk gets
//! exactly the answer it always did.

use axum::body::to_bytes;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use oxy_api_frontline::frontline_devices::{NewDevice, bind_submit, create, device_status};

use crate::frontline_kiosk_leave_route::break_kiosk_lookups;
use crate::frontline_kiosk_mode::{bound_kiosk, cookies, kiosk_cookie, seed_org, wired_db};

async fn probe(headers: HeaderMap) -> (StatusCode, serde_json::Value, Vec<String>) {
    let resp = device_status(headers).await;
    let status = resp.status();
    let set = set_cookies(&resp);
    let body = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, serde_json::from_slice(&body).expect("json"), set)
}

fn set_cookies(resp: &Response) -> Vec<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

/// A `Set-Cookie`'s attributes, order-free, without its `name=value`.
fn attributes(set_cookie: &str) -> std::collections::BTreeSet<String> {
    set_cookie.split("; ").skip(1).map(str::to_string).collect()
}

#[tokio::test]
async fn the_probe_answers_503_when_it_cannot_look_a_kiosk_up_and_bound_false_to_a_non_kiosk() {
    let db = wired_db().await;
    let org = seed_org(&db).await;
    let (id, cookie) = bound_kiosk(&db, org, "Front counter").await;

    let (status, body, _) = probe(kiosk_cookie(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bound"], true);
    // Not a kiosk, with the database answering: unchanged.
    for (case, headers) in [
        ("no cookie", HeaderMap::new()),
        ("forged secret", kiosk_cookie(&format!("{id}.forged"))),
        ("garbage", kiosk_cookie("garbage")),
    ] {
        let (status, body, set) = probe(headers).await;
        assert_eq!(status, StatusCode::OK, "{case}");
        assert_eq!(body, serde_json::json!({ "bound": false }), "{case}");
        assert!(set.is_empty(), "{case}: {set:?}");
    }

    break_kiosk_lookups(&db).await;

    let (status, body, set) = probe(kiosk_cookie(&cookie)).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a lookup that failed is unknown, not 'not a kiosk': {body}"
    );
    assert!(set.is_empty(), "{set:?}");
    // A browser with no kiosk cookie has nothing to look up, so the outage
    // does not reach it: the same answer as ever.
    let (status, body, _) = probe(HeaderMap::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "bound": false }));
}

#[tokio::test]
async fn binding_a_tablet_sets_the_page_readable_hint_beside_the_kiosk_cookie() {
    let db = wired_db().await;
    // SAFETY: nextest runs each test in its own process (asserted by
    // `fresh_db`), and nothing here reads the environment concurrently.
    unsafe { std::env::set_var("OXY_SESSION_COOKIE_DOMAIN", ".oxygen-hq.com") };
    let org = seed_org(&db).await;
    let (_, token) = create(
        &db,
        org,
        NewDevice {
            name: "Front counter",
            ..Default::default()
        },
    )
    .await
    .expect("create kiosk");

    let resp = bind_submit(HeaderMap::new(), format!("token={token}")).await;
    assert!(resp.status().is_redirection(), "{}", resp.status());
    let set = set_cookies(&resp);
    let kiosk = set
        .iter()
        .find(|c| c.starts_with("oxy_kiosk="))
        .unwrap_or_else(|| panic!("no kiosk cookie: {set:?}"));
    let hint = set
        .iter()
        .find(|c| c.starts_with("oxy_kiosk_hint="))
        .unwrap_or_else(|| panic!("no hint cookie: {set:?}"));

    assert!(hint.starts_with("oxy_kiosk_hint=1;"), "{hint}");
    // The page must be able to read it, so it is the one attribute apart.
    assert!(attributes(kiosk).contains("HttpOnly"), "{kiosk}");
    assert!(!attributes(hint).contains("HttpOnly"), "{hint}");
    let mut without_http_only = attributes(kiosk);
    without_http_only.remove("HttpOnly");
    assert_eq!(
        attributes(hint),
        without_http_only,
        "same Domain, Path, Max-Age and SameSite as the kiosk cookie"
    );
    assert!(attributes(hint).contains("Domain=.oxygen-hq.com"), "{hint}");
}

#[tokio::test]
async fn a_kiosk_bound_before_the_hint_existed_gets_it_from_the_probe() {
    let db = wired_db().await;
    let org = seed_org(&db).await;
    let (_, cookie) = bound_kiosk(&db, org, "Front counter").await;

    let (status, _, set) = probe(kiosk_cookie(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        set.iter().any(|c| c.starts_with("oxy_kiosk_hint=1;")),
        "a bound kiosk without the hint is given it: {set:?}"
    );

    // Already carried: nothing to say.
    let (_, _, set) = probe(cookies(&format!("oxy_kiosk={cookie}; oxy_kiosk_hint=1"))).await;
    assert!(set.is_empty(), "{set:?}");
}
