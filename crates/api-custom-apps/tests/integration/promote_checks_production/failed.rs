//! What follows a failed `production_smoke`: the notice, and the automatic
//! rollback where the app asks for one.

use agentic_core::delegation::{TaskOutcome, TaskSpec};
use entity::org_members::{self, OrgRole};
use entity::{notifications, users};
use oxy_api_custom_apps::custom_apps_build_checks::PRODUCTION_FAILED_KIND;
use oxy_api_custom_apps::custom_apps_build_checks::production::failed::{
    ProductionFailedTask, enqueue,
};
use oxy_api_custom_apps::custom_apps_build_checks::production::followup::{
    recipients, run_guarded,
};
use oxy_api_custom_apps::custom_apps_build_checks::production::rollback::NO_BASELINE;
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, Statement};

use super::*;
use crate::promote_rollback::{audit_rows, fixture_builds, pointers, rollback};

const NOTICE: &str = "oxy.production_check_failed";
const AUTO_ROLLED_BACK: &str = "app.auto_rolled_back";

/// The app's queued follow-ups of a failed production check, oldest first.
async fn follow_ups(agent: &Agent) -> Vec<ProductionFailedTask> {
    let rows = agent
        .t
        .db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue \
             WHERE spec->>'kind' = $1 AND spec->'payload'->>'app_id' = $2 \
             ORDER BY created_at, task_id",
            [
                PRODUCTION_FAILED_KIND.into(),
                agent.app.id.to_string().into(),
            ],
        ))
        .await
        .expect("read the queue");
    rows.iter()
        .map(|row| {
            let spec: Value = row.try_get("", "spec").expect("spec");
            let spec: TaskSpec = serde_json::from_value(spec).expect("a TaskSpec");
            ProductionFailedTask::from_spec(&spec).expect("a follow-up")
        })
        .collect()
}

/// A person in the fixture's org with `role`; `None` is an account in no org.
async fn someone(agent: &Agent, role: Option<OrgRole>) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("pc-{id}@example.com"))),
        name: ActiveValue::Set("Promote Checks Test".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(&agent.t.db)
    .await
    .expect("seed a user");
    if let Some(role) = role {
        org_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(agent.app.org_id),
            user_id: ActiveValue::Set(id),
            role: ActiveValue::Set(role),
            ..Default::default()
        }
        .insert(&agent.t.db)
        .await
        .expect("seed a member");
    }
    id
}

/// The checker claims `build`'s newest production attempt and reports
/// `production_smoke` with `status`, naming one new error.
async fn report_production(agent: &Agent, build: Uuid, status: &str) -> Value {
    let base = checks(agent, build);
    let (code, claimed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("production")),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{claimed}");
    let report = json!({ "attempt": claimed["attempt"], "environment": "production", "checks": [{
        "kind": "production_smoke", "status": status,
        "summary": "1 new error on 1 of 2 screens",
        "detail": { "findings": [], "baseline_at": null, "screens": [],
            "new_errors": [{ "path": "stores", "viewport": "desk", "error": "TypeError: new" }] },
    }] });
    let (code, reported) = checker("POST", &format!("{base}/report"), Some(report)).await;
    assert_eq!(code, StatusCode::OK, "{reported}");
    claimed
}

/// What `build`'s own staging comparison saw on production before the move:
/// a passed `changes` row of a staging attempt, the baseline a production
/// attempt of the build is judged against (`production::baseline`). The
/// fixture's builds were published before the gate was on, so they have no
/// staging attempt of their own to report one through.
async fn with_a_baseline(agent: &Agent, build: Uuid) {
    let at = chrono::Utc::now().fixed_offset();
    app_build_checks::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(agent.app.id),
        build_id: ActiveValue::Set(build),
        environment: ActiveValue::Set("staging".into()),
        kind: ActiveValue::Set("changes".into()),
        attempt: ActiveValue::Set(1),
        status: ActiveValue::Set("passed".into()),
        advisory: ActiveValue::Set(true),
        summary: ActiveValue::Set(Some("2 screens compared, 1 changed".into())),
        detail: ActiveValue::Set(Some(changes_detail(json!([])))),
        notes: ActiveValue::Set(json!([])),
        run_id: ActiveValue::Set(None),
        runner: ActiveValue::Set(None),
        created_at: ActiveValue::Set(at),
        started_at: ActiveValue::Set(Some(at)),
        finished_at: ActiveValue::Set(Some(at)),
    }
    .insert(&agent.t.db)
    .await
    .expect("seed the build's comparison");
}

/// Who holds the notice about `check`, by user id, sorted.
async fn told(agent: &Agent, check: Uuid) -> Vec<Uuid> {
    let mut who: Vec<Uuid> = notifications::Entity::find()
        .filter(notifications::Column::Kind.eq(NOTICE))
        .filter(notifications::Column::SubjectId.eq(check.to_string()))
        .all(&agent.t.db)
        .await
        .expect("read the notices")
        .into_iter()
        .inspect(|n| {
            assert_eq!(n.subject_kind.as_deref(), Some("app_build_check"));
            assert_eq!((n.org_id, n.app_id), (agent.app.org_id, None));
        })
        .map(|n| n.user_id)
        .collect();
    who.sort();
    who
}

async fn smoke_row(agent: &Agent, build: Uuid, attempt: i32) -> app_build_checks::Model {
    rows(agent, &key(build, "production", attempt))
        .await
        .remove(0)
}

/// The mover and the org's owners and admins are told, once, with what the
/// notice is worded from left on the check's row; production stays where it
/// is, because the app does not roll back automatically.
#[tokio::test]
async fn a_failed_production_check_tells_the_mover_and_the_orgs_owners_and_admins() {
    let agent = gated().await;
    let (prod, stg) = fixture_builds(&agent).await;
    settings(&agent, json!({ "required": [] })).await;
    let admin = someone(&agent, Some(OrgRole::Admin)).await;
    let member = someone(&agent, Some(OrgRole::Member)).await;
    promote(&agent).await;
    assert!(
        follow_ups(&agent).await.is_empty(),
        "nothing has failed yet"
    );

    report_production(&agent, stg, "failed").await;
    let queued = follow_ups(&agent).await;
    assert_eq!(queued.len(), 1, "one follow-up per failed attempt");
    assert_eq!((queued[0].build_pk, queued[0].attempt), (stg, 1));
    let outcome = run_guarded(&agent.t.db, &queued[0]).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the follow-up failed: {outcome:?}");
    };
    assert_eq!(answer, "2 told; production left as it is");

    let check = smoke_row(&agent, stg, 1).await;
    let mut expected = vec![agent.t.guest_id, admin];
    expected.sort();
    assert_eq!(
        told(&agent, check.id).await,
        expected,
        "not the plain member"
    );
    assert!(!expected.contains(&member));
    // What the notice says is read from the row: the build before, by name.
    let detail = check.detail.expect("the detail");
    assert_eq!(detail["move"]["action"], "promote");
    assert_eq!(detail["move"]["by"], json!(agent.t.guest_id));
    assert_eq!(
        detail["move"]["previous"],
        json!({ "id": prod, "build_id": "agent-prod" })
    );
    assert_eq!(
        detail["new_errors"][0]["error"], "TypeError: new",
        "the runner's evidence stays"
    );
    assert!(
        detail.get("rollback").is_none(),
        "the app does not roll back"
    );
    assert_eq!(pointers(&agent).await.0, Some(stg), "production stays");
    assert!(audit_rows(&agent, AUTO_ROLLED_BACK).await.is_empty());

    // The task again tells nobody twice.
    let outcome = run_guarded(&agent.t.db, &queued[0]).await;
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(told(&agent, check.id).await, expected);

    // Whoever moved production is told even when they hold no role in the org.
    let outsider = someone(&agent, None).await;
    let who = recipients(&agent.t.db, agent.app.org_id, Some(outsider))
        .await
        .unwrap();
    assert_eq!(who[0], outsider);
    assert!(who.contains(&admin) && who.contains(&agent.t.guest_id) && !who.contains(&member));
}

/// Production goes back to the build it served before, once: audited as the
/// platform's move, written on the check's row, never repeated for the
/// attempt, and never answered by another automatic rollback.
#[tokio::test]
async fn with_auto_rollback_production_is_rolled_back_once_and_never_in_a_loop() {
    let agent = gated().await;
    let (prod, stg) = fixture_builds(&agent).await;
    settings(&agent, json!({ "required": [], "auto_rollback": true })).await;
    promote(&agent).await;
    with_a_baseline(&agent, stg).await;
    report_production(&agent, stg, "failed").await;
    let task = follow_ups(&agent).await.remove(0);

    let outcome = run_guarded(&agent.t.db, &task).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the follow-up failed: {outcome:?}");
    };
    assert_eq!(answer, "1 told; production rolled back");
    assert_eq!(
        pointers(&agent).await.0,
        Some(prod),
        "back on the build before"
    );
    let check = smoke_row(&agent, stg, 1).await;
    let recorded = check.detail.clone().expect("the detail")["rollback"].clone();
    assert_eq!(recorded["rolled_back"], true);
    assert_eq!(
        check.detail.clone().unwrap()["move"]["baseline"],
        true,
        "the platform read the build's baseline"
    );
    assert_eq!(
        recorded["to"],
        json!({ "id": prod, "build_id": "agent-prod" })
    );
    let audited = audit_rows(&agent, AUTO_ROLLED_BACK).await;
    assert_eq!(audited.len(), 1);
    let row = &audited[0];
    assert_eq!(
        (row.actor_type.as_str(), row.actor_email.as_str()),
        ("system", "system:promote-checks")
    );
    assert_eq!(row.actor_user_id, None);
    assert_eq!(
        row.metadata,
        json!({ "from": null, "build": prod, "previous": stg,
            "reason": "production_smoke failed", "check": check.id, "attempt": 1,
            "moved_by": agent.t.guest_id })
    );
    assert_eq!(told(&agent, check.id).await, [agent.t.guest_id]);

    // The same task again, and the same attempt queued again: no second move.
    let outcome = run_guarded(&agent.t.db, &task).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the follow-up failed: {outcome:?}");
    };
    assert_eq!(
        answer, "0 told; production rolled back",
        "what the row records"
    );
    assert!(
        !enqueue(&agent.t.db, &task).await.unwrap(),
        "already queued"
    );
    assert_eq!(follow_ups(&agent).await.len(), 1);
    assert_eq!(audit_rows(&agent, AUTO_ROLLED_BACK).await.len(), 1);
    assert_eq!(pointers(&agent).await.0, Some(prod));
    assert_eq!(
        smoke_row(&agent, stg, 1).await.detail.unwrap()["rollback"],
        recorded
    );

    // The rollback is a move: the build it went back to is checked too. When
    // that check fails as well, production is NOT sent back again.
    assert_eq!(
        statuses(&agent, &key(prod, "production", 1)).await,
        [("production_smoke".to_string(), "queued".to_string())]
    );
    report_production(&agent, prod, "failed").await;
    let second = follow_ups(&agent).await.remove(1);
    assert_eq!((second.build_pk, second.attempt), (prod, 1));
    let outcome = run_guarded(&agent.t.db, &second).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the follow-up failed: {outcome:?}");
    };
    assert_eq!(answer, "1 told; production left as it is");
    assert_eq!(pointers(&agent).await.0, Some(prod), "no loop");
    assert_eq!(audit_rows(&agent, AUTO_ROLLED_BACK).await.len(), 1);
    let left = smoke_row(&agent, prod, 1).await.detail.unwrap();
    assert_eq!(left["rollback"]["rolled_back"], false);
    assert_eq!(
        left["rollback"]["reason"],
        "the build reached production by a rollback, and a rollback is never rolled back \
         automatically"
    );
    assert_eq!(left["move"]["by"], json!(null), "the platform moved it");
}

/// With no baseline from before the move (a promoting publish, a build
/// promoted before its comparison ran) a failed check found nothing it can
/// call the move's. With `auto_rollback` on, production still stays where it
/// is, whatever the runner reported: the platform reads the baseline itself.
/// People are still told, from a row that says there was none, which the
/// notice words as errors on production, not new ones.
#[tokio::test]
async fn with_no_baseline_a_failed_check_is_told_and_never_rolls_production_back() {
    let agent = gated().await;
    let (prod, stg) = fixture_builds(&agent).await;
    settings(&agent, json!({ "required": [], "auto_rollback": true })).await;
    promote(&agent).await;
    report_production(&agent, stg, "failed").await;
    let task = follow_ups(&agent).await.remove(0);

    let outcome = run_guarded(&agent.t.db, &task).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the follow-up failed: {outcome:?}");
    };
    assert_eq!(answer, "1 told; production left as it is");
    assert_eq!(pointers(&agent).await.0, Some(stg), "production stays");
    assert!(audit_rows(&agent, AUTO_ROLLED_BACK).await.is_empty());
    let check = smoke_row(&agent, stg, 1).await;
    assert_eq!(told(&agent, check.id).await, [agent.t.guest_id]);
    let detail = check.detail.expect("the detail");
    assert_eq!(
        detail["rollback"],
        json!({ "rolled_back": false, "at": detail["rollback"]["at"], "reason": NO_BASELINE })
    );
    assert_eq!(detail["move"]["baseline"], false, "what the notice reads");
    assert_eq!(
        detail["move"]["previous"],
        json!({ "id": prod, "build_id": "agent-prod" })
    );
}

/// A move someone made after the check started is theirs: it is not undone.
/// And a check that could not judge (`error`) follows nothing.
#[tokio::test]
async fn production_that_moved_since_is_left_alone_and_an_error_follows_nothing() {
    let agent = gated().await;
    let (prod, stg) = fixture_builds(&agent).await;
    settings(&agent, json!({ "required": [], "auto_rollback": true })).await;
    promote(&agent).await;
    report_production(&agent, stg, "failed").await;
    let task = follow_ups(&agent).await.remove(0);
    // Before the follow-up runs, a person rolls production back by hand.
    let (status, plan) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let outcome = run_guarded(&agent.t.db, &task).await;
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(
        pointers(&agent).await.0,
        Some(prod),
        "where the person left it"
    );
    assert!(audit_rows(&agent, AUTO_ROLLED_BACK).await.is_empty());
    let left = smoke_row(&agent, stg, 1).await.detail.unwrap();
    assert_eq!(
        left["rollback"],
        json!({ "rolled_back": false, "at": left["rollback"]["at"],
            "reason": "production has moved to another build since the check started" })
    );

    // The build the person rolled back to: its check cannot judge.
    report_production(&agent, prod, "error").await;
    assert_eq!(
        follow_ups(&agent).await.len(),
        1,
        "an error follows nothing"
    );
    assert!(
        told(&agent, smoke_row(&agent, prod, 1).await.id)
            .await
            .is_empty()
    );
}

/// The follow-up is the platform's own work: a daemon in the lists that tell
/// one from the tenant's work (the run feed and workspace health), a kind
/// `oxy-app` refuses to boot without, and handed to the worker fleet by
/// `oxy-server`. (The console's own copy of the feed's list is the web app's
/// to add it to.)
#[test]
fn the_follow_up_is_a_platform_daemon_the_fleet_is_handed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let quoted = format!("\"{PRODUCTION_FAILED_KIND}\"");
    for (file, list) in [
        (
            "../agentic/runtime/src/lifecycle/crud/system_sources.rs",
            "pub const SYSTEM_SOURCE_TYPES: &[&str] = &[",
        ),
        (
            "../usage-report/src/customer_runs.rs",
            "const NON_WORKSPACE_RUN_SOURCES: &[&str] = &[",
        ),
        (
            "../app-core/src/seams.rs",
            "pub const CUSTOM_APP_TASK_KINDS: [&str; ",
        ),
    ] {
        let source = std::fs::read_to_string(root.join(file)).expect(file);
        let start = source
            .find(list)
            .unwrap_or_else(|| panic!("{file} no longer declares `{list}`"));
        let body = &source[start..];
        let body = &body[..body.find("];").expect("the list ends")];
        assert!(
            body.contains(&quoted),
            "{file}: `{PRODUCTION_FAILED_KIND}` is missing"
        );
    }
    let seams = std::fs::read_to_string(root.join("../server/src/custom_apps_seams.rs"))
        .expect("read custom_apps_seams.rs");
    let squeezed: String = seams.split_whitespace().collect();
    assert!(
        squeezed.contains(
            "TaskSeam{kind:oxy_api_custom_apps::custom_apps_build_checks::PRODUCTION_FAILED_KIND,"
        ),
        "`custom_apps_seams::tasks` no longer hands in the follow-up's executor"
    );
}
