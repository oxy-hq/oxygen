//! `/admin/compiles/data-apps` — the Data App headcount staff read instead of an
//! assume-role session. It reads the promoted revision's `app_definitions` rows and
//! must hand back counts and names only, narrowed to the caller's grant like every
//! other listing on this console.

use axum::extract::Query;
use axum::http::StatusCode;
use chrono::Utc;
use entity::{app_definitions, revisions, workspaces};
use oxy_app::server::api::admin::compiles::data_apps::{DataAppsQuery, list_data_apps};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use super::fixture::{Reply, World, as_actor, reply, world};

/// A `ready` `main` revision of `workspace`; `promote` makes it the one the
/// workspace serves.
async fn revision(db: &DatabaseConnection, workspace: Uuid, promote: bool) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(workspace),
        git_sha: ActiveValue::Set(id.simple().to_string()),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("main".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(0),
        file_count_compiled: ActiveValue::Set(0),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    if promote {
        let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(workspace)
            .one(db)
            .await
            .expect("read workspace")
            .expect("workspace exists")
            .into();
        ws.current_revision_id = ActiveValue::Set(Some(id));
        ws.update(db).await.expect("promote revision");
    }
    id
}

async fn data_app(db: &DatabaseConnection, revision_id: Uuid, name: &str, published: bool) {
    app_definitions::ActiveModel {
        revision_id: ActiveValue::Set(revision_id),
        file_path: ActiveValue::Set(format!("apps/{name}.app.yml")),
        name: ActiveValue::Set(name.into()),
        definition: ActiveValue::Set(serde_json::json!({ "name": name, "secret": "tenant-sql" })),
        published: ActiveValue::Set(published),
    }
    .insert(db)
    .await
    .expect("seed data app");
}

/// Org A's workspace: a promoted revision with three Data Apps, two published, and a
/// newer revision that is not promoted with five more that must not be counted.
/// Org B's workspace: one unpublished Data App. The org-less workspace: one published.
async fn seed(w: &World) {
    let a = revision(&w.db, w.ws_a, true).await;
    data_app(&w.db, a, "sales", true).await;
    data_app(&w.db, a, "ops", true).await;
    data_app(&w.db, a, "draft", false).await;
    let not_promoted = revision(&w.db, w.ws_a, false).await;
    for i in 0..5 {
        data_app(&w.db, not_promoted, &format!("branch-{i}"), true).await;
    }
    let b = revision(&w.db, w.ws_b, true).await;
    data_app(&w.db, b, "b-draft", false).await;
    let orphan = revision(&w.db, w.ws_orphan, true).await;
    data_app(&w.db, orphan, "orphan", true).await;
}

async fn ask(actor: &oxy_auth::types::AuthenticatedUser, with_apps: Option<bool>) -> Reply {
    reply(list_data_apps(as_actor(actor), Query(DataAppsQuery { with_apps })).await).await
}

fn row<'a>(reply: &'a Reply, workspace: Uuid) -> &'a serde_json::Value {
    reply.body["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["workspace_id"] == workspace.to_string())
        .unwrap_or_else(|| panic!("workspace {workspace} not listed in {}", reply.body))
}

#[tokio::test]
async fn counts_the_promoted_revision_only_and_reports_names_and_counts_alone() {
    let w = world().await;
    seed(&w).await;
    let uncompiled = super::fixture::seed_workspace(&w.db, "uncompiled", Some(w.org_a)).await;

    let all = ask(&w.unbounded, None).await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.body);

    let a = row(&all, w.ws_a);
    assert_eq!(
        a["data_app_count"], 3,
        "a revision that is not promoted was counted"
    );
    assert_eq!(a["published_count"], 2);
    assert_eq!(a["compiled"], true);
    assert_eq!(a["org_id"], w.org_a.to_string());
    assert!(
        a["org_name"]
            .as_str()
            .is_some_and(|n| n.starts_with("Scope a")),
        "{a}"
    );
    assert_eq!(a["workspace_name"], "ws-a");

    let b = row(&all, w.ws_b);
    assert_eq!(
        (b["data_app_count"].clone(), b["published_count"].clone()),
        (1.into(), 0.into())
    );
    let orphan = row(&all, w.ws_orphan);
    assert_eq!(orphan["published_count"], 1);
    assert!(orphan["org_name"].is_null() && orphan["org_id"].is_null());

    // Not compiled: counted as zero, and says so rather than reading as "none".
    let u = row(&all, uncompiled);
    assert_eq!(u["compiled"], false);
    assert_eq!(u["data_app_count"], 0);
    assert_eq!(all.body["uncompiled_workspaces"], 1);
    assert_eq!(all.body["total_workspaces"], 4);
    assert_eq!(all.body["total_data_apps"], 5);
    assert_eq!(all.body["total_published"], 3);

    // Nothing of the apps themselves: no name, path or definition, in any row or total.
    let text = all.body.to_string();
    for leaked in [
        "sales",
        "draft",
        "tenant-sql",
        ".app.yml",
        "definition",
        "file_path",
    ] {
        assert!(
            !text.contains(leaked),
            "the response carries {leaked:?}: {text}"
        );
    }
    let mut keys: Vec<_> = a.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "compiled",
            "data_app_count",
            "org_id",
            "org_name",
            "published_count",
            "workspace_id",
            "workspace_name"
        ]
    );

    // `with_apps` drops workspaces that hold none.
    let only = ask(&w.unbounded, Some(true)).await;
    assert_eq!(only.body["total_workspaces"], 3);
    assert!(
        !only
            .column(Some("rows"), "workspace_id")
            .contains(&uncompiled.to_string()),
        "a workspace with no Data Apps survived `with_apps`"
    );

    // Deleting a promoted revision's rows is what a removal does: the count follows.
    app_definitions::Entity::delete_many()
        .filter(
            app_definitions::Column::RevisionId.eq(workspaces::Entity::find_by_id(w.ws_b)
                .one(&w.db)
                .await
                .unwrap()
                .unwrap()
                .current_revision_id
                .unwrap()),
        )
        .exec(&w.db)
        .await
        .unwrap();
    assert_eq!(
        row(&ask(&w.unbounded, None).await, w.ws_b)["data_app_count"],
        0
    );
}

/// The grant scope rule every listing here follows: a bounded grant counts its own
/// orgs, never another tenant's, and not the org-less workspace either.
#[tokio::test]
async fn a_bounded_grant_counts_only_its_own_orgs_workspaces() {
    let w = world().await;
    seed(&w).await;

    let bounded = ask(&w.bounded, None).await;
    assert_eq!(bounded.status, StatusCode::OK, "{}", bounded.body);
    assert_eq!(
        bounded.column(Some("rows"), "workspace_id"),
        vec![w.ws_a.to_string()],
        "org B's workspace, or the org-less one, is counted"
    );
    assert_eq!(bounded.body["total_data_apps"], 3);
    assert_eq!(bounded.body["total_published"], 2);

    for (who, actor) in w.everything_readers() {
        let all = ask(actor, None).await;
        let listed = all.column(Some("rows"), "workspace_id");
        for ws in [w.ws_a, w.ws_b, w.ws_orphan] {
            assert!(listed.contains(&ws.to_string()), "{who} lost {ws}");
        }
        assert_eq!(all.body["total_published"], 3, "{who}");
    }
}
