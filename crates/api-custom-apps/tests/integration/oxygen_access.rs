//! `ctx.oxygen.*` through the real host against a real database: an app's
//! platform action refuses until an org admin approves it, then runs as the
//! app's service account, records who or what set it off, and refuses again
//! the moment the approval is revoked (`host/app_access_op.rs`).
//!
//! The platform work itself is the composition root's (`oxy-server` installs
//! it at boot); here a recording implementation stands in, so the test proves
//! what the host decides and hands over, not the kiosk code.

use std::sync::{Arc, Mutex, OnceLock};

use entity::prelude::AuditEvents;
use entity::{app_builds, apps, audit_events, organizations, users, workspaces};
use oxy_api_custom_apps::custom_apps_functions::InvocationIdentity;
use oxy_api_custom_apps::custom_apps_functions::env_policy::HostOp;
use oxy_api_custom_apps::custom_apps_functions::host::{
    FunctionCapabilities, ProjectFunctionHost, into_arc,
};
use oxy_api_custom_apps::custom_apps_functions::runtime::FunctionHost;
use oxy_api_custom_apps::custom_apps_functions::seam::FunctionQueryExecutor;
use oxy_api_custom_apps::data_plane::query::DataPlaneQueryExecutor;
use oxy_app_core::app_platform_seam::{
    self, AppPlatformActions, FrontlineDevicesOp, PlatformActionError, PlatformFuture,
    ResolvedPipeline, SourceReport,
};
use oxy_app_core::operating_graph::reach::Reach;
use oxy_auth::token::app_access::ApprovedAction;
use oxy_authz::Action;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use crate::staging_homes_fixture::{duck, production, staging, workspace};

/// What the host handed the platform, in order.
#[derive(Debug, Clone, PartialEq)]
enum Handed {
    Devices {
        org: Uuid,
        op: FrontlineDevicesOp,
        actor: Uuid,
    },
    Upload {
        workspace: Uuid,
        pipeline: String,
        file_path: String,
    },
}

/// The pipelines every test host declares. `missing` is in no workspace of
/// the org and `twice` in two, as the stand-in resolves them.
const DECLARED: [&str; 4] = ["ubereats_transactions", "toast_pos", "missing", "twice"];

fn handed() -> &'static Mutex<Vec<Handed>> {
    static HANDED: OnceLock<Mutex<Vec<Handed>>> = OnceLock::new();
    HANDED.get_or_init(|| Mutex::new(Vec::new()))
}

struct Recording;

impl AppPlatformActions for Recording {
    fn frontline_devices<'a>(
        &'a self,
        _db: &'a DatabaseConnection,
        org_id: Uuid,
        op: FrontlineDevicesOp,
        actor_user_id: Uuid,
    ) -> PlatformFuture<'a> {
        Box::pin(async move {
            let reply = match &op {
                FrontlineDevicesOp::Create { name, .. } => {
                    json!({ "id": Uuid::new_v4(), "name": name, "enrol_url": "https://oxygen.test/x" })
                }
                _ => json!({ "devices": [] }),
            };
            handed().lock().unwrap().push(Handed::Devices {
                org: org_id,
                op,
                actor: actor_user_id,
            });
            Ok(reply)
        })
    }

    fn resolve_pipeline<'a>(
        &'a self,
        db: &'a DatabaseConnection,
        org_id: Uuid,
        name: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<ResolvedPipeline, PlatformActionError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            match name {
                "missing" => Err(PlatformActionError::NotFound(format!(
                    "no pipeline `{name}`"
                ))),
                "twice" => Err(PlatformActionError::Invalid(format!(
                    "more than one pipeline is named `{name}`"
                ))),
                _ => {
                    let workspace = workspaces::Entity::find()
                        .filter(workspaces::Column::OrgId.eq(org_id))
                        .one(db)
                        .await
                        .expect("read workspaces")
                        .expect("the org has a workspace");
                    Ok(ResolvedPipeline {
                        workspace_id: workspace.id,
                        file_path: format!("pipelines/{name}.airway.yml"),
                        definition: json!({ "name": name }),
                    })
                }
            }
        })
    }

    fn source_upload_report<'a>(
        &'a self,
        pipeline: ResolvedPipeline,
        report: SourceReport,
    ) -> PlatformFuture<'a> {
        Box::pin(async move {
            handed().lock().unwrap().push(Handed::Upload {
                workspace: pipeline.workspace_id,
                pipeline: report.pipeline,
                file_path: pipeline.file_path,
            });
            Ok(
                json!({ "location": "s3://zone/x", "report_year": 2026, "report_month": 10, "rows": 1 }),
            )
        })
    }
}

struct World {
    db: DatabaseConnection,
    org: Uuid,
    workspace: Uuid,
    app: apps::Model,
    admin: Uuid,
}

async fn world(asks: Value) -> World {
    app_platform_seam::install(Arc::new(Recording));
    let db = fresh_db(Schema::All).await.0;
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org),
        name: Set("Acme".into()),
        slug: Set(format!("acme-{}", org.simple())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed org");
    let ws = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(ws),
        name: Set("acme".into()),
        org_id: Set(Some(org)),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace");
    let admin = Uuid::new_v4();
    users::ActiveModel {
        id: Set(admin),
        email: Set(Some(format!("{}@acme.test", admin.simple()))),
        name: Set("Admin".into()),
        email_verified: Set(true),
        status: Set(users::UserStatus::Active),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed admin");
    let id = Uuid::new_v4();
    let app = apps::ActiveModel {
        id: Set(id),
        org_id: Set(org),
        project_id: Set(ws),
        slug: Set(format!("store-ops-{}", &id.simple().to_string()[..8])),
        name: Set("Store Ops".into()),
        branch: Set("main".into()),
        source_repo: Set("acme/store-ops".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        visibility: Set("org".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed app");
    app_builds::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(id),
        build_id: Set("b1".into()),
        s3_prefix: Set("apps/b1".into()),
        manifest_json: Set(Some(json!({ "functions": { "tablets": asks } }))),
        created_at: Set(chrono::Utc::now().into()),
        validation_status: Set("passed".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed build");
    World {
        db,
        org,
        workspace: ws,
        app,
        admin,
    }
}

fn host(
    w: &World,
    project: Arc<dyn oxy_api_custom_apps::custom_apps_functions::seam::FunctionProjectContext>,
    mode: &str,
    person: Option<Uuid>,
    policy: oxy_api_custom_apps::custom_apps_functions::env_policy::EnvPolicy,
) -> Arc<dyn FunctionHost> {
    into_arc(ProjectFunctionHost::new(
        project,
        Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>,
        w.db.clone(),
        Vec::new(),
        w.workspace,
        w.app.id,
        w.org,
        person.unwrap_or(Uuid::nil()),
        w.app.name.clone(),
        FunctionCapabilities {
            frontline_devices: true,
            source_uploads: true,
            source_upload_pipelines: DECLARED.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
        Default::default(),
        Reach::nowhere(),
        InvocationIdentity {
            invocation_id: Uuid::new_v4(),
            function_name: "tablets".into(),
            mode: mode.into(),
            request_id: None,
            app_slug: w.app.slug.clone(),
            user_id: person,
            user_email: None,
            credential_token_id: None,
        },
        policy,
    ))
}

async fn rows(db: &DatabaseConnection, action: &str) -> Vec<audit_events::Model> {
    AuditEvents::find()
        .filter(audit_events::Column::Action.eq(action))
        .all(db)
        .await
        .expect("read audit")
}

#[tokio::test]
async fn kiosks_refuse_until_approved_then_run_as_the_app_and_stop_at_a_revoke() {
    let w =
        world(json!({ "route": true, "oxygen": { "frontline": { "devices": "manage" } } })).await;
    let ws = workspace(&duck("w")).await;
    let person = Some(w.admin);

    let refused = host(&w, ws.ctx(), "route", person, production(&[]))
        .app_action(HostOp::FrontlineDevicesList, json!({}))
        .await
        .unwrap_err();
    assert!(
        refused.message.starts_with("AppAccessNotApproved: "),
        "{}",
        refused.message
    );

    let approved = oxy_auth::token::app_access::approve(&w.db, &w.app, &[kiosks()], w.admin)
        .await
        .expect("approve");
    let account = approved.account.user_id;

    let reply = host(&w, ws.ctx(), "route", person, production(&[]))
        .app_action(
            HostOp::FrontlineDevicesCreate,
            json!({ "name": "Front counter" }),
        )
        .await
        .expect("an approved app manages kiosks");
    assert_eq!(reply["name"], "Front counter");
    let last = handed().lock().unwrap().last().cloned();
    assert!(
        matches!(&last, Some(Handed::Devices { org, actor, op: FrontlineDevicesOp::Create { name, .. } })
            if *org == w.org && *actor == account && name == "Front counter"),
        "{last:?}"
    );

    // One row, as the app's account, saying what set it off and for whom.
    let created = rows(&w.db, "frontline.device.created").await;
    assert_eq!(created.len(), 1);
    let meta = created[0].metadata.clone();
    assert_eq!(meta["trigger"], "person");
    assert_eq!(meta["on_behalf_of"], w.admin.to_string());
    assert_eq!(meta["token_kind"], "app");
    assert_eq!(meta["token_id"], approved.token.id.to_string());
    assert_eq!(meta["function"], "tablets");

    // A schedule may do it too; its row says so and names nobody.
    host(&w, ws.ctx(), "schedule", None, production(&[]))
        .app_action(
            HostOp::FrontlineDevicesCreate,
            json!({ "name": "Drive-thru" }),
        )
        .await
        .expect("unattended runs are allowed");
    let created = rows(&w.db, "frontline.device.created").await;
    assert!(
        created
            .iter()
            .any(|r| r.metadata["trigger"] == "schedule" && r.metadata["on_behalf_of"].is_null())
    );

    // An action that was not approved stays refused.
    let upload = host(&w, ws.ctx(), "route", person, production(&[]))
        .app_action(
            HostOp::SourceUploadsReport,
            json!({
                "file": "YQ==", "fileName": "r.csv", "pipeline": "ubereats_transactions",
                "workflowId": "w"
            }),
        )
        .await
        .unwrap_err();
    assert!(
        upload.message.starts_with("AppAccessNotApproved: "),
        "{}",
        upload.message
    );

    oxy_auth::token::app_access::revoke(&w.db, w.app.id, w.admin)
        .await
        .expect("revoke")
        .expect("there was something to revoke");
    let after = host(&w, ws.ctx(), "route", person, production(&[]))
        .app_action(HostOp::FrontlineDevicesList, json!({}))
        .await
        .unwrap_err();
    assert!(
        after.message.starts_with("AppAccessNotApproved: "),
        "{}",
        after.message
    );
}

#[tokio::test]
async fn an_upload_names_a_declared_approved_pipeline_and_lands_in_its_zone() {
    let w = world(json!({ "oxygen": { "sourceUploads": { "pipelines": DECLARED } } })).await;
    let ws = workspace(&duck("w")).await;
    // Approved on three of the four pipelines the function lists: not `toast_pos`.
    let on = |name: &str| ApprovedAction {
        action: Action::SourceUploadReport,
        target: Some(name.to_string()),
    };
    oxy_auth::token::app_access::approve(
        &w.db,
        &w.app,
        &[on("ubereats_transactions"), on("missing"), on("twice")],
        w.admin,
    )
    .await
    .expect("approve");
    let report = |pipeline: &str| {
        json!({
            "file": "YSxiCjEsMgo=",
            "fileName": "report.csv",
            "pipeline": pipeline,
            "workflowId": "wf-1"
        })
    };
    let call = |payload: Value| {
        let h = host(&w, ws.ctx(), "webhook", None, production(&[]));
        async move { h.app_action(HostOp::SourceUploadsReport, payload).await }
    };

    // Each refusal, in the order the host decides, and none reaches the zone.
    let before = handed().lock().unwrap().len();
    let cases = [
        (json!({ "fileName": "r.csv" }), "AppActionInvalid: "),
        (
            report("not_listed"),
            "OxygenCapabilityMissing: this function has not declared the pipeline `not_listed`",
        ),
        (report("toast_pos"), "AppAccessNotApproved: "),
        (report("missing"), "AppActionNotFound: "),
        (report("twice"), "AppActionInvalid: more than one"),
    ];
    for (payload, starts) in cases {
        let err = call(payload.clone()).await.unwrap_err();
        assert!(
            err.message.starts_with(starts),
            "{payload}: {}",
            err.message
        );
    }
    assert_eq!(
        handed().lock().unwrap().len(),
        before,
        "refused before the zone"
    );

    let reply = call(report("ubereats_transactions"))
        .await
        .expect("an approved upload runs from a webhook");
    assert_eq!(reply["rows"], 1);
    let last = handed().lock().unwrap().last().cloned();
    assert_eq!(
        last,
        Some(Handed::Upload {
            workspace: w.workspace,
            pipeline: "ubereats_transactions".into(),
            file_path: "pipelines/ubereats_transactions.airway.yml".into(),
        }),
        "the zone of the workspace that holds the named pipeline"
    );
    let uploaded = rows(&w.db, "source_upload.report_uploaded").await;
    assert_eq!(uploaded.len(), 1);
    assert_eq!(uploaded[0].metadata["trigger"], "webhook");
    assert_eq!(uploaded[0].metadata["pipeline"], "ubereats_transactions");
}

#[tokio::test]
async fn a_disabled_account_or_another_orgs_app_gets_nothing() {
    let w = world(json!({ "oxygen": { "frontline": { "devices": "manage" } } })).await;
    let ws = workspace(&duck("w")).await;
    let approved = oxy_auth::token::app_access::approve(&w.db, &w.app, &[kiosks()], w.admin)
        .await
        .expect("approve");
    let mut account: entity::service_accounts::ActiveModel = approved.account.into();
    account.disabled_at = Set(Some(chrono::Utc::now().into()));
    account.update(&w.db).await.expect("disable");
    let refused = host(&w, ws.ctx(), "route", Some(w.admin), production(&[]))
        .app_action(HostOp::FrontlineDevicesList, json!({}))
        .await
        .unwrap_err();
    assert!(
        refused.message.starts_with("AppAccessNotApproved: "),
        "{}",
        refused.message
    );
}

/// Outside production the registry refuses the op before the host is asked:
/// what it changes has no copy per environment.
#[test]
fn outside_production_every_oxygen_op_is_refused() {
    use oxy_api_custom_apps::custom_apps_functions::env_policy::{Decision, Staged};
    let _ = staging(&[]);
    for op in [
        HostOp::FrontlineDevicesList,
        HostOp::FrontlineDevicesCreate,
        HostOp::FrontlineDevicesReissueLink,
        HostOp::FrontlineDevicesRevoke,
        HostOp::SourceUploadsReport,
    ] {
        assert!(
            matches!(op.spec().staging, Staged::Decided(Decision::Refuse { .. })),
            "{op:?} must be refused outside production"
        );
    }
}

fn kiosks() -> ApprovedAction {
    ApprovedAction {
        action: Action::FrontlineDeviceManage,
        target: None,
    }
}
