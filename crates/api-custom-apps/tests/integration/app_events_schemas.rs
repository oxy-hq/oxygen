//! Event schema revisions against Postgres, and the publish that records them.
//!
//! A revision is the platform's immutable copy of a schema a build declared:
//! the same schema reuses its row, a changed one is the next revision, and
//! every publish in any environment records one per declared event. A schema
//! that is not valid JSON Schema refuses the publish before anything is stored.

use chrono::Utc;
use entity::{apps, organizations, workspaces};
use oxy_api_custom_apps::app_events::schema::{live_revision, revision_for};
use oxy_api_custom_apps::custom_apps_publish::{OrgRef, PublishError, PublishInput, publish};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::test_db;

async fn seed_app(db: &DatabaseConnection) -> Uuid {
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set("Schemas Org".into()),
        slug: Set(format!("schemas-{org_id}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set("Schemas Workspace".into()),
        org_id: Set(Some(org_id)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app_id = Uuid::new_v4();
    apps::ActiveModel {
        id: Set(app_id),
        slug: Set(format!("schemas-{app_id}")),
        name: Set("Schemas".into()),
        org_id: Set(org_id),
        project_id: Set(workspace_id),
        branch: Set("main".into()),
        source_repo: Set("app-events/schemas".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        published_at: Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    app_id
}

fn schema(extra: Option<&str>) -> Value {
    let mut properties = json!({"period": {"type": "string"}});
    if let Some(name) = extra {
        properties[name] = json!({"type": "string"});
    }
    json!({"type": "object", "properties": properties, "required": ["period"]})
}

#[tokio::test]
async fn revision_for_reuses_an_identical_schema() {
    let db = test_db().await;
    let app_id = seed_app(&db).await;

    let first = revision_for(&db, app_id, "statement.uploaded", &schema(None))
        .await
        .expect("first revision");
    assert_eq!(first.revision, 1);

    // The same schema with its keys in another order is the same schema.
    let reordered = json!({
        "required": ["period"],
        "properties": {"period": {"type": "string"}},
        "type": "object"
    });
    let again = revision_for(&db, app_id, "statement.uploaded", &reordered)
        .await
        .expect("same schema");
    assert_eq!(again.id, first.id);
    assert_eq!(again.revision, 1);

    let changed = revision_for(&db, app_id, "statement.uploaded", &schema(Some("note")))
        .await
        .expect("changed schema");
    assert_eq!(changed.revision, 2);
    assert_ne!(changed.id, first.id);

    // Another event of the same app counts its own revisions.
    let other = revision_for(&db, app_id, "report.ready", &schema(None))
        .await
        .expect("other event");
    assert_eq!(other.revision, 1);
}

// ── Publishing ───────────────────────────────────────────────────────────────

struct Tenant {
    org_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    email: String,
}

async fn seed_tenant(db: &DatabaseConnection) -> Tenant {
    use entity::{org_members, org_members::OrgRole, users};
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set("Schema Publish Org".into()),
        slug: Set(format!("schema-pub-{}", &org_id.simple().to_string()[..12])),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let user_id = Uuid::new_v4();
    let email = format!("publisher-{user_id}@example.com");
    users::ActiveModel {
        id: Set(user_id),
        email: Set(Some(email.clone())),
        name: Set("Publisher".into()),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        user_id: Set(user_id),
        role: Set(OrgRole::Admin),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org member");
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set("Schema Publish Workspace".into()),
        org_id: Set(Some(org_id)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    Tenant {
        org_id,
        workspace_id,
        user_id,
        email,
    }
}

/// The smallest bundle `validate_bundle` accepts: an index.html with a head.
fn bundle() -> Vec<u8> {
    let html = b"<!doctype html><html><head><title>t</title></head><body></body></html>";
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(html.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "index.html", &html[..])
        .expect("append index.html");
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

fn input(t: &Tenant, slug: &str, build_id: &str, manifest: Value) -> PublishInput {
    PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: t.workspace_id,
        branch: None,
        build_id: build_id.to_string(),
        name: None,
        promote: false,
        tarball: bundle(),
        manifest: Some(manifest),
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.user_id),
        published_by_email: Some(t.email.clone()),
        publisher: Some(oxy_server_authz::Caller::without_credential(
            t.user_id, &t.email,
        )),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
        confirm_schedule_removals: Vec::new(),
    }
}

fn manifest(events: Value) -> Value {
    json!({"events": events})
}

#[tokio::test]
async fn publishing_a_build_records_a_revision_per_event() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let slug = "records-revisions";
    let events = |statement: Value| {
        manifest(json!({
            "statement.uploaded": {"schema": statement},
            "report.ready": {"schema": schema(None)}
        }))
    };

    let first = publish(input(&t, slug, "build-1", events(schema(None))))
        .await
        .expect("first publish");
    let app_id = first.app_id;
    // A publish to staging records its revisions too; a changed schema is a
    // new revision and an unchanged one is not.
    publish(input(&t, slug, "build-2", events(schema(Some("note")))))
        .await
        .expect("second publish");

    let statement = live_revision(&db, app_id, "staging", "statement.uploaded")
        .await
        .expect("query")
        .expect("staging serves the second build's revision");
    assert_eq!(statement.revision, 2);
    assert_eq!(statement.schema, schema(Some("note")));
    let report = live_revision(&db, app_id, "staging", "report.ready")
        .await
        .expect("query")
        .expect("report.ready has a revision");
    assert_eq!(report.revision, 1, "an unchanged schema is not re-recorded");
    // Production serves no build yet, and an undeclared event has no revision.
    assert!(
        live_revision(&db, app_id, "production", "statement.uploaded")
            .await
            .expect("query")
            .is_none()
    );
    assert!(
        live_revision(&db, app_id, "staging", "nothing.here")
            .await
            .expect("query")
            .is_none()
    );
}

#[tokio::test]
async fn a_publish_with_an_invalid_schema_is_refused_naming_the_event() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let bad = manifest(json!({
        "statement.uploaded": {"schema": {"type": "object", "properties": {}}},
        "report.ready": {"schema": {"type": "string"}}
    }));
    let err = publish(input(&t, "invalid-schema", "build-1", bad))
        .await
        .expect_err("a non-object root is refused");
    assert!(matches!(err, PublishError::Invalid(_)), "{err:?}");
    assert!(err.to_string().contains("report.ready"), "{err}");
}

/// A reuse races retention: the sweep deletes the revision after
/// `revision_for` read it and before its reuse stamp lands. The stamp then
/// updates nothing, and `revision_for` records the schema afresh rather than
/// hand back a revision that no longer exists.
#[tokio::test]
async fn a_reuse_whose_revision_the_sweep_took_records_it_afresh() {
    use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement, TransactionTrait};

    let db = test_db().await;
    let app_id = seed_app(&db).await;
    let schema = json!({"type":"object","properties":{"period":{"type":"string"}}});
    let first = revision_for(&db, app_id, "statement.uploaded", &schema)
        .await
        .expect("record");

    // The sweep's delete, holding the row until it commits.
    let sweep = db.begin().await.expect("begin the sweep");
    sweep
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM app_event_schemas WHERE id = $1",
            [first.id.into()],
        ))
        .await
        .expect("delete the revision");

    // The reuse reads the row (the delete is not committed) and its stamp
    // waits on the sweep's lock.
    let reuse = tokio::spawn({
        let db = db.clone();
        let schema = schema.clone();
        async move { revision_for(&db, app_id, "statement.uploaded", &schema).await }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let waiting: i64 = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT count(*) AS n FROM pg_stat_activity \
                  WHERE datname = current_database() AND wait_event_type = 'Lock' \
                    AND query LIKE 'UPDATE%app_event_schemas%'",
            ))
            .await
            .expect("read the activity")
            .expect("a row")
            .try_get("", "n")
            .expect("n");
        if waiting > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the reuse never waited on the sweep's lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    sweep.commit().await.expect("commit the sweep");

    let reused = reuse.await.expect("join").expect("the reuse");
    let stored = entity::app_event_schemas::Entity::find_by_id(reused.id)
        .one(&db)
        .await
        .expect("read the revision");
    assert!(
        stored.is_some(),
        "the revision handed back exists: {reused:?}"
    );
    assert_ne!(reused.id, first.id, "a fresh row, not the deleted one");
}
