//! What a publish to a sandbox refuses, and that a refusal leaves nothing
//! behind: `promote` with it, a caller without non-production reach, a
//! sandbox or an app that does not exist, a sandbox being deleted; the
//! sandbox's **own** refusal of a machine or app-scoped publish token, past
//! `authorize_publish`; and a sandbox deleted after the publish was admitted
//! and before its pointer moved.

use axum::http::StatusCode;
use entity::{app_builds, apps};
use oxy_app::server::api::custom_apps_publish::{
    PublishError, PublishResult, PublishTarget, publish_to,
};
use oxy_app::server::api::custom_apps_sandboxes::{TeardownReason, ops};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement, TransactionTrait,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{BUILD_ID, Tenant, publish_app, seeded_tenant};
use crate::sandbox_publish::{
    app_row, app_with_two_sandboxes, build_pk, bundle, input, noop, publish_to_sandbox, sandbox,
};
use crate::staging_functions::make_guest_staff;

/// Wait until a session of this database is blocked on a lock. `done` says
/// the work under test finished without ever waiting — then nothing will.
pub(crate) async fn until_blocked(db: &DatabaseConnection, done: impl Fn() -> bool) {
    let waiting_on_a_lock = "SELECT count(*) AS n FROM pg_stat_activity \
         WHERE datname = current_database() AND wait_event_type = 'Lock'";
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let waiting: i64 = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                waiting_on_a_lock,
            ))
            .await
            .expect("read pg_stat_activity")
            .expect("a row")
            .try_get("", "n")
            .expect("n");
        if waiting >= 1 {
            return;
        }
        assert!(!done(), "it finished without waiting on the held lock");
        assert!(
            std::time::Instant::now() < deadline,
            "nothing reached the held lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// A publish token is its app's, not a staff identity: a sandbox refuses it
/// itself. The org consents to machine publishes here, so `authorize_publish`
/// lets both through — the control publishes the same token to staging — and
/// the refusal asserted is the sandbox's, `SandboxRefused`, not a missing
/// consent's.
#[tokio::test]
async fn a_machine_or_app_scoped_token_is_refused_by_the_sandbox_itself() {
    let t = seeded_tenant().await;
    let slug = "sbx-machine";
    let app = app_with_two_sandboxes(&t, slug).await;
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO partner_publish_consent (org_id, enabled, granted_by, updated_at) \
         VALUES ($1, true, $2, now()) ON CONFLICT (org_id) DO UPDATE SET enabled = true",
        [t.org_id.into(), t.guest_id.into()],
    ))
    .await
    .expect("the org consents to machine publishes");
    let tarball = || {
        bundle(
            slug,
            &[noop("noop", json!({ "route": true }))],
            json!({}),
            &[],
        )
    };
    // An OIDC-minted machine token: no user, a workflow identity.
    let machine = |build_id: &str| {
        let mut publish = input(&t, slug, build_id, tarball());
        publish.published_by = None;
        publish.published_by_email = None;
        publish.machine_app_id = Some(app.id);
        publish.published_via = Some("repo:oxy-hq/customer-apps:ref:refs/heads/main".into());
        publish
    };
    // A partner-minted token scoped to this app: a real user behind it.
    let scoped = |build_id: &str| {
        let mut publish = input(&t, slug, build_id, tarball());
        publish.machine_app_id = Some(app.id);
        publish
    };

    publish_to(machine("machine-draft"), PublishTarget::Channels)
        .await
        .expect("control: consent lets the machine token publish to staging");
    for (publish, who) in [
        (machine("machine-sandbox"), "a machine token"),
        (scoped("scoped-sandbox"), "an app-scoped token"),
    ] {
        let refused = publish_to(publish, PublishTarget::Sandbox(sandbox("a1")))
            .await
            .expect_err(who);
        assert!(
            matches!(refused, PublishError::SandboxRefused),
            "{who}: {refused}"
        );
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    }
    for build_id in ["machine-sandbox", "scoped-sandbox"] {
        assert_eq!(build_pk(&t.db, app.id, build_id).await, None, "{build_id}");
    }
}

/// A sandbox deleted after a publish to it was admitted, and before the
/// publish moved its pointer: the publish answers `409`, and the build it had
/// already stored and recorded is rolled back — no row, no bytes.
///
/// The test holds the sandbox's row lock while the publish runs, so the
/// publish is admitted (a plain read), stores its build, and blocks at the
/// pointer move; the row is then marked deleting under that lock.
#[tokio::test]
async fn a_sandbox_deleted_after_admission_takes_no_build() {
    let t = seeded_tenant().await;
    let slug = "sbx-late-delete";
    let app = app_with_two_sandboxes(&t, slug).await;
    let url = std::env::var("OXY_DATABASE_URL").expect("test_db points the process at its db");
    let holder = sea_orm::Database::connect(&url).await.expect("connect");
    let held = holder.begin().await.expect("begin");
    held.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM app_environments WHERE app_id = $1 AND name = 'dev-a1' FOR UPDATE",
        [app.id.into()],
    ))
    .await
    .expect("hold dev-a1's row");

    let tarball = bundle(
        slug,
        &[noop("noop", json!({ "route": true }))],
        json!({}),
        &[],
    );
    let publish = publish_to(
        input(&t, slug, "raced", tarball),
        PublishTarget::Sandbox(sandbox("a1")),
    );
    let delete_under_the_lock = async {
        // Blocked at the pointer move: admitted, stored and recorded by now.
        until_blocked(&t.db, || false).await;
        assert!(
            build_pk(&t.db, app.id, "raced").await.is_some(),
            "the build was recorded before the pointer move"
        );
        held.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_environments SET deleting_at = now() \
              WHERE app_id = $1 AND name = 'dev-a1'",
            [app.id.into()],
        ))
        .await
        .expect("mark dev-a1 deleting");
        held.commit().await.expect("let the publish go on");
    };
    let (published, ()) = tokio::join!(publish, delete_under_the_lock);

    let refused = published.expect_err("the sandbox was deleted under the publish");
    assert!(
        matches!(&refused, PublishError::EnvironmentDeleting { name } if name == "dev-a1"),
        "{refused}"
    );
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(build_pk(&t.db, app.id, "raced").await, None, "rolled back");
    let stored =
        oxy_app::server::api::custom_apps_build_store::get_object(app.id, "raced", "index.html")
            .await;
    assert!(!matches!(stored, Ok(Some(_))), "its bytes were removed");
    let row = entity::app_environments::Entity::find_by_id((app.id, "dev-a1".to_string()))
        .one(&t.db)
        .await
        .expect("read dev-a1")
        .expect("the row is still there, deleting");
    assert_eq!(row.build_id, None, "the pointer never moved");
}

/// What a sandbox publish refuses, before a byte is stored: `promote` with
/// it; a sandbox nobody created; one being deleted; an app that does not
/// exist; a publisher without non-production reach; and a machine token.
#[tokio::test]
async fn a_sandbox_publish_is_refused_with_its_status_and_leaves_nothing_behind() {
    let t = seeded_tenant().await;
    let slug = "sbx-refuse";
    // Not yet staff: an org Owner may publish the app, and may not use its
    // sandboxes.
    let hourly = noop("tick", json!({ "schedule": "0 * * * *" }));
    let app_id = publish_app(&t, slug, demo_workspace_id(), &[hourly])
        .await
        .app_id;
    let app = app_row(&t.db, app_id).await;
    crate::app_environments::seed_sandbox(&t.db, app_id, "dev-a1", t.guest_id).await;
    let refused = publish_to_sandbox(&t, slug, "by-a-tenant", "a1")
        .await
        .expect_err("an org Owner is not staff");
    assert!(matches!(refused, PublishError::SandboxRefused), "{refused}");
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    // Refused before anyone is asked who they are: the request contradicts
    // itself.
    let tarball = || {
        bundle(
            slug,
            &[noop("noop", json!({ "route": true }))],
            json!({}),
            &[],
        )
    };
    let mut promoting = input(&t, slug, "promoted", tarball());
    promoting.promote = true;
    let refused = publish_to(promoting, PublishTarget::Sandbox(sandbox("a1")))
        .await
        .expect_err("promote");
    assert!(
        matches!(refused, PublishError::SandboxWithPromote),
        "{refused}"
    );
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

    let mut machine = input(&t, slug, "by-a-machine", tarball());
    machine.published_by = None;
    machine.published_by_email = None;
    machine.machine_app_id = Some(app_id);
    machine.published_via = Some("repo:oxy-hq/customer-apps:ref:refs/heads/main".into());
    let refused = publish_to(machine, PublishTarget::Sandbox(sandbox("a1")))
        .await
        .expect_err("a machine token");
    assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{refused}");

    // From here the publisher is staff (`publish_as_owner`).
    make_guest_staff();
    let unknown = publish_as_owner(&t, slug, "to-nobody", "nobody").await;
    let refused = unknown.expect_err("no such sandbox");
    assert!(
        matches!(&refused, PublishError::UnknownEnvironment { name } if name == "dev-nobody"),
        "{refused}"
    );
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);

    let refused = publish_as_owner(&t, "no-such-app", "to-no-app", "a1")
        .await
        .expect_err("no such app");
    assert!(
        matches!(refused, PublishError::UnknownEnvironment { .. }),
        "{refused}"
    );
    assert!(
        apps::Entity::find()
            .filter(apps::Column::Slug.eq("no-such-app"))
            .one(&t.db)
            .await
            .expect("read")
            .is_none(),
        "a sandbox publish never creates an app"
    );

    ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    let refused = publish_as_owner(&t, slug, "to-deleting", "a1")
        .await
        .expect_err("the sandbox is being deleted");
    assert!(
        matches!(&refused, PublishError::EnvironmentDeleting { name } if name == "dev-a1"),
        "{refused}"
    );
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    // Nothing any of them named was recorded or stored.
    let builds = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .all(&t.db)
        .await
        .expect("read the builds");
    let labels: Vec<&str> = builds.iter().map(|b| b.build_id.as_str()).collect();
    assert_eq!(labels, vec![BUILD_ID], "only the app's own publish");
    for build_id in ["by-a-tenant", "promoted", "to-nobody", "to-deleting"] {
        let stored = oxy_app::server::api::custom_apps_build_store::get_object(
            app_id,
            build_id,
            "index.html",
        )
        .await;
        assert!(
            !matches!(stored, Ok(Some(_))),
            "{build_id}: no bytes were stored"
        );
    }
}

/// A sandbox publish by a staff user who is not the guest — `OXY_OWNER` names
/// the guest's email, and the publish carries it.
async fn publish_as_owner(
    t: &Tenant,
    slug: &str,
    build_id: &str,
    handle: &str,
) -> Result<PublishResult, PublishError> {
    let owner = owner_twin(t).await;
    let tarball = bundle(
        slug,
        &[noop("noop", json!({ "route": true }))],
        json!({}),
        &[],
    );
    let mut publish = input(t, slug, build_id, tarball);
    publish.published_by = Some(owner);
    // Authority is asked of the publisher, so the twin is the caller too.
    publish.publisher = Some(oxy_app::server::authz::Caller::without_credential(
        owner,
        oxy_auth::user::LOCAL_GUEST_EMAIL,
    ));
    publish_to(publish, PublishTarget::Sandbox(sandbox(handle))).await
}

/// A second user row to publish as once the guest is staff: the non-production
/// decision is cached per (user, app) for a minute, and the guest's was made
/// while they were not.
async fn owner_twin(t: &Tenant) -> Uuid {
    crate::app_environments::seed_user(&t.db).await
}
