use serde_json::json;

use super::*;

fn app() -> apps::Model {
    apps::Model {
        id: Uuid::from_u128(1),
        slug: "store-ops".into(),
        name: "Store Ops".into(),
        org_id: Uuid::from_u128(2),
        project_id: Uuid::from_u128(3),
        branch: "main".into(),
        source_repo: String::new(),
        status: "active".into(),
        source_type: "s3".into(),
        source_config: json!({}),
        last_synced_at: None,
        manifest_override: None,
        bootstrap_pr_url: None,
        published_at: None,
        repo_path: None,
        last_promoted_by: None,
        last_promoted_at: None,
        visibility: "org".into(),
        require_second_person_to_promote: false,
        promotion_checks: None,
        created_at: Default::default(),
        updated_at: Default::default(),
    }
}

fn build() -> app_builds::Model {
    app_builds::Model {
        id: Uuid::from_u128(10),
        app_id: Uuid::from_u128(1),
        build_id: "b-42".into(),
        s3_prefix: "customer-apps/x/builds/b-42/".into(),
        manifest_json: None,
        created_at: Default::default(),
        published_by: None,
        published_via: None,
        source_repo: None,
        commit_sha: None,
        source_branch: None,
        validation_status: "passed".into(),
        validation_detail: None,
        semantic_revision_id: None,
        published_token_id: None,
        published_to: Some("staging".into()),
        sdk_version: None,
    }
}

fn check(detail: serde_json::Value) -> app_build_checks::Model {
    app_build_checks::Model {
        id: Uuid::from_u128(100),
        app_id: Uuid::from_u128(1),
        build_id: Uuid::from_u128(10),
        environment: "production".into(),
        kind: "production_smoke".into(),
        attempt: 1,
        status: "failed".into(),
        advisory: false,
        summary: Some("2 new errors on 1 of 3 screens".into()),
        detail: Some(detail),
        notes: json!([]),
        run_id: None,
        runner: None,
        created_at: Default::default(),
        started_at: None,
        finished_at: None,
    }
}

fn errors(n: usize) -> Vec<serde_json::Value> {
    (0..n)
        .map(|i| json!({ "path": "stores", "viewport": "desk", "error": format!("Uncaught TypeError: e{i}") }))
        .collect()
}

/// The app, the build, the build before it and the errors, and what to do,
/// with a link to the place the check and Roll back are.
#[test]
fn it_names_the_app_the_build_the_one_before_and_the_new_errors() {
    let detail = json!({
        "findings": [],
        "new_errors": [
            { "path": "stores", "viewport": "desk", "error": "Uncaught TypeError: x is undefined" },
            { "path": "", "viewport": "phone", "error": "Uncaught ReferenceError: y" },
        ],
        "move": { "action": "promote", "by": null,
            "previous": { "id": Uuid::from_u128(9), "build_id": "b-41" } },
    });
    let record = record_of(&check(detail), &app(), &build());
    assert_eq!(record.previous.as_deref(), Some("b-41"));
    assert_eq!(record.total, 2);
    assert_eq!(record.rollback, Rollback::NotAsked);
    let said = words(&record, Some("acme"));
    assert_eq!(
        said.title,
        "Store Ops: new errors on production after build b-42"
    );
    assert_eq!(
        said.body.as_deref(),
        Some(
            "Build b-42 is live and production shows 2 new errors since it replaced build b-41. \
             /stores at desk: Uncaught TypeError: x is undefined; / at phone: Uncaught \
             ReferenceError: y. Roll back or fix forward from the app's Releases page."
        )
    );
    assert_eq!(
        said.href.as_deref(),
        Some("/admin/apps/acme/store-ops/releases")
    );
    assert_eq!(words(&record, None).href, None);
}

#[test]
fn it_says_when_production_was_rolled_back_and_why_when_it_was_not() {
    let rolled = json!({ "findings": [], "new_errors": errors(1),
        "rollback": { "rolled_back": true, "to": { "id": Uuid::from_u128(9), "build_id": "b-41" },
            "at": "2026-11-22T10:00:00Z" } });
    let record = record_of(&check(rolled), &app(), &build());
    assert_eq!(record.rollback, Rollback::RolledBack { to: "b-41".into() });
    let body = words(&record, Some("acme")).body.unwrap();
    assert!(
        body.ends_with("Production was rolled back to build b-41 automatically."),
        "{body}"
    );
    // Rolled back, the build is no longer live: told in the past.
    assert!(
        body.starts_with("Build b-42 showed 1 new error on production."),
        "{body}"
    );
    assert!(!body.contains("is live"), "{body}");

    let refused = json!({ "findings": [], "new_errors": errors(1),
        "rollback": { "rolled_back": false,
            "reason": "production has moved to another build since the check started",
            "at": "2026-11-22T10:00:00Z" } });
    let body = words(&record_of(&check(refused), &app(), &build()), None)
        .body
        .unwrap();
    assert!(
        body.ends_with(
            "Production was not rolled back automatically: production has moved to another \
             build since the check started."
        ),
        "{body}"
    );
    // A record that says it rolled back and names no build did not.
    let odd = json!({ "findings": [], "rollback": { "rolled_back": true } });
    assert!(matches!(
        record_of(&check(odd), &app(), &build()).rollback,
        Rollback::NotRolledBack { .. }
    ));
}

/// Three errors are quoted, each cut, and the rest counted, the runner's own
/// omitted ones included.
#[test]
fn the_errors_are_capped_and_the_rest_counted() {
    let mut listed = errors(5);
    listed[0]["error"] = json!("E".repeat(1000));
    let detail = json!({ "findings": [], "new_errors": listed, "new_errors_omitted": 40 });
    let record = record_of(&check(detail), &app(), &build());
    assert_eq!(record.errors.len(), MAX_ERRORS);
    assert_eq!(record.total, 45);
    let first = record.errors[0].trim_start_matches("/stores at desk: ");
    assert_eq!(first.chars().count(), MAX_ERROR_CHARS);
    assert!(first.ends_with('…'));
    let body = words(&record, None).body.unwrap();
    assert!(body.contains("shows 45 new errors."), "{body}");
    assert!(body.contains("(and 42 more)."), "{body}");
    assert!(body.chars().count() < 800, "{}", body.chars().count());
    // A row with no detail at all still reads: the build and a count of none.
    let mut bare = check(json!({}));
    bare.detail = None;
    let record = record_of(&bare, &app(), &build());
    assert_eq!((record.total, record.previous.clone()), (0, None));
    assert_eq!(
        words(&record, None).body.as_deref(),
        Some(
            "Build b-42 is live and production shows 0 new errors. Roll back or fix forward \
             from the app's Releases page."
        )
    );
}

/// With no baseline from before the move the notice says what the check
/// found, errors on production, and never calls them new; everything else is
/// worded as with one. A row that says it had one, and a row written before
/// the platform recorded it, read as new errors.
#[test]
fn with_no_baseline_it_says_errors_on_production_never_new_ones() {
    let detail = json!({ "findings": [], "baseline_at": null, "new_errors": errors(2),
        "move": { "action": "publish", "by": null, "baseline": false,
            "previous": { "id": Uuid::from_u128(9), "build_id": "b-41" } },
        "rollback": { "rolled_back": false,
            "reason": "production had no baseline from before the move to compare its errors with",
            "at": "2026-11-22T10:00:00Z" } });
    let record = record_of(&check(detail), &app(), &build());
    assert!(!record.baselined);
    let said = words(&record, Some("acme"));
    assert_eq!(
        said.title,
        "Store Ops: errors on production after build b-42"
    );
    let body = said.body.expect("a body");
    assert_eq!(
        body,
        "Build b-42 is live and production shows 2 errors since it replaced build b-41. \
         /stores at desk: Uncaught TypeError: e0; /stores at desk: Uncaught TypeError: e1. \
         Production was not rolled back automatically: production had no baseline from \
         before the move to compare its errors with."
    );
    assert!(!body.contains("new"), "{body}");
    assert_eq!(
        said.href.as_deref(),
        Some("/admin/apps/acme/store-ops/releases")
    );

    for moved in [json!({ "baseline": true }), json!({ "action": "promote" })] {
        let detail = json!({ "findings": [], "new_errors": errors(1), "move": moved.clone() });
        let record = record_of(&check(detail), &app(), &build());
        assert!(record.baselined, "{moved}");
        let said = words(&record, None);
        assert_eq!(
            said.title,
            "Store Ops: new errors on production after build b-42"
        );
        assert!(said.body.unwrap().contains("shows 1 new error."));
    }
}
