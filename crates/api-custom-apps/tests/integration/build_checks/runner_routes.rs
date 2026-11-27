//! The four runner routes, as a person with non-production reach calls them,
//! and who is answered the not-found of an unknown build.

use super::*;

#[tokio::test]
async fn a_person_with_reach_drives_the_four_routes_and_a_verdict_is_written_once() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var("OXY_API_URL") };
    let t = seeded_tenant().await;
    let published = publish_staging(&t, "chk-hand", "ch-1").await;
    let (app, build) = models(&t, &published).await;
    let person = Runner::of(staff(&t), app.id, &build.build_id);

    let mut headers = HeaderMap::new();
    headers.insert("host", "checks.example.com".parse().unwrap());
    let claimed = person
        .claim(by_hand(), headers)
        .await
        .expect("staff claim the attempt");
    assert_eq!(
        claimed,
        json!({
            "app": { "id": app.id, "workspace_id": demo_workspace_id(),
                "org_slug": ORG_SLUG, "slug": "chk-hand" },
            "build": { "id": build.id, "build_id": "ch-1", "sdk_version": "2.30.0" },
            "environment": "staging",
            "attempt": 1,
            "serving": true,
            "url": format!("https://checks.example.com/customer-apps/{ORG_SLUG}/chk-hand/"),
            "identity": LOCAL_GUEST_EMAIL,
            "sdk_floor": "2.30.0",
            "open": ["loads", "design_system"],
            // The build declares no screens, production serves nothing, and a
            // staging attempt has no baseline.
            "screens": null,
            "production": null,
            "baseline": null,
            "limits": { "screens": 30, "file_bytes": 2_000_000, "files": 200 },
        })
    );
    // The platform's own two are decided by the claim; the runner's two run.
    assert_eq!(
        statuses(&t.db, build.id, 1).await,
        pairs(&[
            ("design_system", "running"),
            ("functions", "skipped"),
            ("loads", "running"),
            ("validation", "passed"),
        ])
    );
    let validation = row_of(&t.db, build.id, "validation").await;
    assert_eq!(validation.summary.as_deref(), Some("bundle valid"));
    // Named by the platform from who called, not by the label that was sent.
    let runner = validation.runner.unwrap();
    assert_eq!(runner["label"], hand_label());
    assert_eq!(runner["by"], LOCAL_GUEST_EMAIL);

    let settled = person.settle(json!({ "attempt": 1 })).await.unwrap();
    assert_eq!(settled["done"], true);
    assert_eq!(settled["status"], "skipped");
    assert!(
        settled["summary"]
            .as_str()
            .unwrap()
            .contains("declares no checks")
    );

    let sheets = person.stylesheets().await.unwrap();
    assert_eq!(
        sheets,
        json!({ "sources": [
            { "file": "index.html", "css": String::from_utf8_lossy(INDEX) },
            { "file": "assets/app.css", "css": String::from_utf8_lossy(CSS) },
        ] })
    );

    let loads = json!({ "kind": "loads", "status": "passed", "summary": "ready on desk and phone",
        "detail": { "findings": [], "identity": "app_checker" } });
    let first = person
        .report(json!({ "attempt": 1, "checks": [loads] }))
        .await
        .unwrap();
    assert_eq!(
        first,
        json!({ "state": "running", "open": ["design_system"] })
    );
    let design = json!({ "kind": "design_system", "status": "failed",
        "summary": "index.html:4 restates --card",
        "detail": { "findings": [{ "severity": "error", "message": "restates --card",
            "at": "index.html:4", "rule": "restates-sdk-token" }], "sdk_version": "2.30.0" } });
    let second = person
        .report(json!({ "attempt": null, "checks": [design] }))
        .await
        .unwrap();
    assert_eq!(second, json!({ "state": "failed", "open": [] }));

    // A verdict already written stands, whatever is reported after it.
    let again = json!({ "kind": "loads", "status": "failed", "summary": "changed my mind",
        "detail": { "findings": [] } });
    let third = person
        .report(json!({ "attempt": 1, "checks": [again] }))
        .await
        .unwrap();
    assert_eq!(third["state"], "failed");
    let loads = row_of(&t.db, build.id, "loads").await;
    assert_eq!(
        (loads.status.as_str(), loads.summary.as_deref()),
        ("passed", Some("ready on desk and phone"))
    );
    assert_eq!(loads.detail.unwrap()["identity"], "app_checker");

    // Every check has its verdict: the attempt is closed to a second runner.
    let closed = person.claim(by_hand(), HeaderMap::new()).await.unwrap_err();
    assert_eq!(closed.0, 409);
    assert_eq!(closed.1["code"], "attempt_closed");
    assert!(closed.1["message"].is_string(), "{:?}", closed.1);
    // What the platform decides is not a runner's to report.
    let refused = person
        .report(
            json!({ "checks": [{ "kind": "validation", "status": "passed",
            "detail": { "findings": [] } }] }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (refused.0, &refused.1["code"]),
        (400, &json!("kind_not_reportable"))
    );
}

#[tokio::test]
async fn a_caller_without_reach_and_a_publish_token_get_the_not_found_of_an_unknown_build() {
    let t = seeded_tenant().await;
    let published = publish_staging(&t, "chk-who", "cw-1").await;
    let (app, build) = models(&t, &published).await;
    let staff = staff(&t);

    let stranger = Runner::of(
        actor(Uuid::new_v4(), "someone@example.com"),
        app.id,
        &build.build_id,
    );
    // Staff, on a publish token: the token is never the runner.
    let mut token = Runner::of(staff.clone(), app.id, &build.build_id);
    token.marker = Some(axum::Extension(AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: Some(app.id),
        machine_identity: None,
    }));
    // Staff, naming a build this app does not have.
    let unknown = Runner::of(staff.clone(), app.id, "no-such-build");
    for (who, caller) in [
        ("stranger", &stranger),
        ("token", &token),
        ("unknown", &unknown),
    ] {
        let bare = (404, Value::Null);
        assert_eq!(
            caller.claim(by_hand(), HeaderMap::new()).await,
            Err(bare.clone()),
            "{who}"
        );
        assert_eq!(caller.settle(json!({})).await, Err(bare.clone()), "{who}");
        assert_eq!(caller.stylesheets().await, Err(bare.clone()), "{who}");
        // Refused before the body is read: a malformed one changes nothing.
        let by = Requester {
            actor: caller.actor.clone(),
            marker: caller.marker.clone(),
        };
        let malformed = runner::report(
            RefuseSandboxAgent,
            by,
            caller.path(),
            HeaderMap::new(),
            Bytes::from_static(b"not json"),
        )
        .await;
        assert_eq!(refusal(malformed.unwrap_err()).await, bare, "{who}");
    }
    assert_eq!(
        statuses(&t.db, build.id, 1).await,
        pairs(&ALL_QUEUED),
        "nobody without reach moved a check"
    );
    // An attempt the build never had is not found either, for staff too.
    let person = Runner::of(staff, app.id, &build.build_id);
    let body = json!({ "attempt": 7, "runner": { "label": "by hand" } });
    assert_eq!(
        person.claim(body, HeaderMap::new()).await,
        Err((404, Value::Null))
    );
    assert_eq!(
        person.report(json!({ "attempt": 7 })).await,
        Err((404, Value::Null))
    );
    let bad = person
        .claim(json!({ "runner": {} }), HeaderMap::new())
        .await;
    assert_eq!(bad.unwrap_err().1["code"], "invalid_body");
}

#[tokio::test]
async fn the_read_route_answers_only_a_caller_who_may_open_staging() {
    let t = seeded_tenant().await;
    let staff = staff(&t);
    let published = publish_staging(&t, "chk-read", "rd-1").await;
    let (app, build) = models(&t, &published).await;

    let path = || Path((app.id, build.build_id.clone()));
    let Json(answer) = handlers::show(
        RefuseSandboxAgent,
        staff,
        None,
        path(),
        Query(ShowQuery::default()),
    )
    .await
    .expect("staff read the checks");
    assert_eq!((answer.attempt, answer.state), (Some(1), "running"));
    assert_eq!(answer.checks.len(), 4);
    assert_eq!(answer.checks[0].label, "Builds and validates");
    assert!(answer.runner.is_none() && answer.stalled.is_none());

    let stranger = actor(Uuid::new_v4(), "someone@example.com");
    let refused = handlers::show(
        RefuseSandboxAgent,
        stranger,
        None,
        path(),
        Query(ShowQuery::default()),
    )
    .await
    .expect_err("a caller who may not open staging");
    assert_eq!(refused.into_response().status(), 404);
}
