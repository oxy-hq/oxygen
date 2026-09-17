//! Tests for the per-app health endpoint.
//!
//! Split out because `mod.rs` was two-thirds test against a ~400-line HARD file
//! limit, and because these assertions ARE the contract: a monitor is configured
//! against the bytes this endpoint emits, so they are read at the wire level
//! (status + `Cache-Control` + body text) rather than off the structs. A serde
//! rename or a hand-edited JSON literal would pass a struct-level assertion and
//! silently break every configured monitor.

use super::*;

/// An `apps::Model` with only the fields this module reads set meaningfully.
/// Mirrors `custom_apps_source`'s own fixture; kept local so a change to that
/// test module can't silently retune this one.
fn fake_app(source_type: &str, source_config: serde_json::Value) -> apps::Model {
    apps::Model {
        id: Uuid::nil(),
        visibility: "org".into(),
        slug: "console".into(),
        name: "Console".into(),
        org_id: Uuid::nil(),
        project_id: Uuid::nil(),
        branch: "main".into(),
        source_repo: "oxy-hq/customer-apps".into(),
        status: "created".into(),
        source_type: source_type.into(),
        source_config,
        bootstrap_pr_url: None,
        last_synced_at: None,
        manifest_override: None,
        published_at: None,
        repo_path: None,
        draft_build_id: None,
        published_build_id: None,
        last_promoted_by: None,
        last_promoted_at: None,
        created_at: chrono::Utc::now().fixed_offset(),
        updated_at: chrono::Utc::now().fixed_offset(),
    }
}

/// The ladder-shape guarantee, enforced in a *release* build too.
///
/// `respond`'s `debug_assert!` covers CI and dev, which is most of the value —
/// but the shipped binary is a release build, so a drifted ladder would reach
/// production silently. This drives `evaluate` end to end on the one bail-out
/// that needs no database (an unparseable `source_config` returns before the
/// build lookup), and asserts the body carries every rung in `LADDER` order.
/// Add a name to the const without wiring it up and this fails.
#[tokio::test]
async fn evaluate_reports_the_whole_ladder_in_order_on_a_bail_out() {
    // A row left over from the removed `v0` source → `AppSource::from_model`
    // errors at the second rung, before anything touches Postgres.
    let app = fake_app("v0", serde_json::json!({ "url": "https://example.v0.dev" }));
    let (build, checks) = evaluate(&app).await;

    assert!(build.is_none());
    assert!(
        checks.iter().map(|c| c.name).eq(LADDER),
        "every rung, in order: got {:?}",
        checks.iter().map(|c| c.name).collect::<Vec<_>>()
    );
    assert_eq!(checks[0].result, PASS, "registered");
    assert_eq!(checks[1].result, FAIL, "source_config is the failing rung");
    assert!(
        checks[2..].iter().all(|c| c.result == "skipped"),
        "everything below an unparseable source is unevaluated, not failed"
    );
    // The remediation that made this its own rung rather than folding into
    // the bundle check.
    let detail = checks[1].detail.as_deref().unwrap();
    assert!(
        detail.contains("re-publishing will not repair it"),
        "{detail}"
    );
    assert!(detail.contains(r#"unknown source_type "v0""#), "{detail}");
}

fn state(marked_published: bool, has_published_build: bool) -> PublicationState {
    PublicationState {
        marked_published,
        has_published_build,
    }
}

/// The build pointer is the question: it is what the serve path resolves.
#[test]
fn a_published_app_needs_the_published_build_pointer() {
    assert_eq!(publication_check(state(true, true)).result, PASS);
    assert_eq!(publication_check(state(false, false)).result, FAIL);
}

/// "Published but nothing promoted" is reachable with an ordinary org-member
/// token — `user_can_access_app` only requires `published_at` — and the serve
/// path 404s on it. It gets its own sentence because that is the message an
/// operator reads, and "never promoted, or unpublished" would send them
/// looking for the wrong thing.
#[test]
fn published_without_a_build_is_its_own_diagnosis() {
    let bare = publication_check(state(true, false));
    let never = publication_check(state(false, false));
    assert_eq!(bare.result, FAIL);
    assert_eq!(never.result, FAIL);
    assert!(bare.detail.as_deref().unwrap().contains("marked published"));
    assert_ne!(bare.detail, never.detail);
}

/// Read a response the way a monitor does: status, cache header, body text.
async fn wire(resp: Response) -> (StatusCode, String, String) {
    let status = resp.status();
    let cache = resp
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        cache,
        String::from_utf8(bytes.to_vec()).expect("utf8"),
    )
}

fn app_ref() -> AppRef {
    AppRef {
        id: Uuid::nil(),
        org_slug: "acme".into(),
        slug: "console".into(),
    }
}

/// The exact bytes a monitor asserts on when everything is fine.
///
/// Pinned end-to-end rather than on the struct, because the contract is the
/// serialized body: `"oxy_app_health":"pass"`, a 200, and `no-store`. A serde
/// rename or a status flip would pass a struct-level test and break every
/// configured monitor.
#[tokio::test]
async fn a_fully_passing_ladder_is_200_pass_and_uncacheable() {
    let checks = LADDER.iter().map(|n| Check::pass(n)).collect();
    let (status, cache, body) = wire(respond(app_ref(), None, checks)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache, "no-store");
    assert!(body.contains(r#""oxy_app_health":"pass""#), "{body}");
    assert!(!body.contains(r#""oxy_app_health":"fail""#));
}

/// One failing rung anywhere flips the whole verdict — the property the
/// endpoint exists for. Swept across every position so a future short-circuit
/// can't leave a late failure unreported.
#[tokio::test]
async fn any_single_failing_rung_flips_the_verdict_to_503() {
    for i in 0..LADDER.len() {
        let checks = LADDER
            .iter()
            .enumerate()
            .map(|(j, n)| {
                if i == j {
                    Check::fail(n, "boom")
                } else {
                    Check::pass(n)
                }
            })
            .collect();
        let (status, _, body) = wire(respond(app_ref(), None, checks)).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "a failure at rung {} ({}) must 503",
            i,
            LADDER[i]
        );
        assert!(
            body.contains(r#""oxy_app_health":"fail""#),
            "rung {i}: {body}"
        );
    }
}

/// `skipped` is the third result, and a ladder carrying only passes and skips is
/// still a pass. If this ever flips, any rung that legitimately cannot be
/// evaluated turns the app permanently red — the shape of a bug this ladder has
/// already had once.
///
/// Exercises `respond` directly with a hand-built ladder rather than a state
/// `evaluate` reaches (its earliest bail-out is `source_config`), because the
/// property under test belongs to `respond`: how it maps results to a verdict.
#[tokio::test]
async fn a_ladder_of_skips_is_not_a_failure() {
    let mut checks = vec![Check::pass("registered")];
    skip_remaining(&mut checks, "nothing to evaluate");
    let (status, _, body) = wire(respond(app_ref(), None, checks)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""oxy_app_health":"pass""#), "{body}");
}

/// The Host-resolved form on a host that names no app. A monitor pointed at
/// the admin hostname by mistake must get the same `fail` shape as any other
/// failure, not an unparsed 400 or a bare status.
#[tokio::test]
async fn the_host_form_on_a_non_subdomain_is_a_fail_shaped_404() {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "app.oxygen-hq.com".parse().unwrap());
    let (status, cache, body) = wire(get_health_for_host(headers).await).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(cache, "no-store");
    assert!(body.contains(r#""oxy_app_health":"fail""#), "{body}");
    assert!(
        body.contains("/api/customer-apps/{org}/{app}/health"),
        "{body}"
    );
}

/// A missing `Host` header must not panic or resolve to some default app.
#[tokio::test]
async fn the_host_form_without_a_host_header_is_a_fail_shaped_404() {
    let (status, _, body) = wire(get_health_for_host(HeaderMap::new()).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains(r#""oxy_app_health":"fail""#), "{body}");
}

/// `LADDER` is an index into `checks` in `skip_remaining`, so a duplicate name
/// would make the tail fill wrong and the `debug_assert` compare equal anyway.
#[test]
fn ladder_names_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for name in LADDER {
        assert!(seen.insert(name), "duplicate rung name: {name}");
    }
}

/// Filling a complete ladder must add nothing — otherwise the happy path
/// would grow duplicate rungs the moment anything called it defensively.
#[test]
fn skip_remaining_is_a_no_op_on_a_complete_ladder() {
    let mut checks: Vec<Check> = LADDER.iter().map(|n| Check::pass(n)).collect();
    skip_remaining(&mut checks, "unused");
    assert_eq!(checks.len(), LADDER.len());
    assert!(checks.iter().all(|c| c.result == PASS));
}

/// Every non-200 path must look like `fail` to a **body-matching** monitor, or
/// the documented "200 = pass, anything else = fail" contract only holds for
/// status-matching ones — and §12b recommends matching on the body.
///
/// The body assertion is the point, not decoration. `error_response` hand-builds
/// its JSON from a `json!` literal rather than the serde-derived struct, so the
/// verdict key there can drift from the one every other response emits by a
/// single mistyped character. Asserting only the status would let that ship.
#[tokio::test]
async fn auth_failures_carry_the_same_verdict_key_and_are_uncacheable() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let (got, cache, body) = wire(error_response(status, reason(status))).await;
        assert_eq!(got, status);
        assert_eq!(
            cache, "no-store",
            "{status} must not be cacheable — a cached denial is as bad as a cached 200"
        );
        assert!(
            body.contains(r#""oxy_app_health":"fail""#),
            "{status} must read as fail to a body-matching monitor: {body}"
        );
        assert!(
            !body.contains(r#""oxy_app_health":"pass""#),
            "{status}: {body}"
        );
    }
}

/// The hand-built error body and the serde-derived verdict body must name the
/// verdict identically.
///
/// Two code paths produce the key a monitor greps for: `#[derive(Serialize)]` on
/// `HealthResponse`, and a literal inside `error_response`. Nothing but this test
/// couples them, so a rename on either side — a serde attribute, or a typo in the
/// literal — would leave half the responses unmatchable while every other test
/// still passed.
#[tokio::test]
async fn the_hand_built_error_body_cannot_drift_from_the_derived_one() {
    let derived = serde_json::to_value(HealthResponse {
        oxy_app_health: FAIL,
        app: app_ref(),
        build: None,
        checks: vec![],
        checked_at: "2026-08-21T00:00:00Z".into(),
    })
    .unwrap();
    let (_, _, raw) = wire(error_response(StatusCode::NOT_FOUND, "nope")).await;
    let hand_built: serde_json::Value = serde_json::from_str(&raw).unwrap();

    let key_of = |v: &serde_json::Value| -> String {
        v.as_object()
            .unwrap()
            .keys()
            .find(|k| k.starts_with("oxy_"))
            .expect("a verdict key")
            .clone()
    };
    assert_eq!(
        key_of(&derived),
        key_of(&hand_built),
        "the two paths must emit the same verdict key"
    );
    assert_eq!(derived[key_of(&derived)], hand_built[key_of(&hand_built)]);
}
