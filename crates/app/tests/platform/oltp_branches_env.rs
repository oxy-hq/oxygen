//! The environment-branch resolver on the Neon-shaped fixture from
//! `oltp_branches.rs`. The rows are written straight into
//! `oltp_env_branches`: the cut that really makes them is a later task, and
//! what is asserted here is only what the resolver does with a row.

use oxy::adapters::secrets::envelope;
use oxy_oltp::branches::CutScope;
use oxy_oltp::entity::env_branch_roles;
use oxy_oltp::entity::env_branches::{self, EnvBranchMode, EnvBranchStatus};
use oxy_oltp::resolver::{EnvBranchAccept, ResolveError, resolve_env_branch_writer_for_org};
use oxy_oltp::{GrantLevel, schema};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use uuid::Uuid;

use crate::oltp_branches::{Neon, host_of, neon, password_of};

const ENV: &str = "dev-a1";
const BRANCH_HOST: &str = "ep-env-1.example.test";
const BRANCH_PASSWORD: &str = "env-branch-writer-password";

/// An `oltp_env_branches` row for `ENV` of `app_id`, cut at its own endpoint
/// under `status` — the shape a finished cut leaves.
async fn insert_row(
    n: &Neon,
    app_id: Uuid,
    status: EnvBranchStatus,
    edit: impl FnOnce(&mut env_branches::ActiveModel),
) -> env_branches::Model {
    let tenant = n.tenant().await;
    let now = chrono::Utc::now().fixed_offset();
    let mut row = env_branches::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        tenant_row_id: ActiveValue::Set(tenant.id),
        app_id: ActiveValue::Set(app_id),
        env_name: ActiveValue::Set(ENV.into()),
        mode: ActiveValue::Set(EnvBranchMode::Branch),
        status: ActiveValue::Set(status),
        error: ActiveValue::Set(None),
        provider_branch_id: ActiveValue::Set(Some("br-env-1".into())),
        parent_branch_id: ActiveValue::Set(tenant.branch_id.clone()),
        host: ActiveValue::Set(Some(BRANCH_HOST.into())),
        database_name: ActiveValue::Set(Some(tenant.database_name.clone())),
        owner_role: ActiveValue::Set(Some("env_owner".into())),
        owner_password_ciphertext: ActiveValue::Set(None),
        fork_at: ActiveValue::Set(now),
        expires_at: ActiveValue::Set(now + chrono::Duration::days(30)),
        created_at: ActiveValue::Set(now),
        last_reset_at: ActiveValue::Set(None),
        updated_at: ActiveValue::Set(now),
    };
    edit(&mut row);
    row.insert(&n.db).await.expect("seed env branch")
}

/// The writer's sealed password on the environment's branch.
async fn mint_writer_password(n: &Neon, row: &env_branches::Model) {
    let tenant = n.tenant().await;
    let role = schema::qualify_role(
        &tenant.provider,
        &tenant.database_name,
        &n.writer.role_name(GrantLevel::ReadWrite),
    );
    env_branch_roles::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        env_branch_row_id: ActiveValue::Set(row.id),
        role_name: ActiveValue::Set(role),
        password_ciphertext: ActiveValue::Set(envelope::seal(BRANCH_PASSWORD.as_bytes()).unwrap()),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(&n.db)
    .await
    .expect("seed env branch role");
}

async fn resolve(
    n: &Neon,
    app_id: Uuid,
    accept: EnvBranchAccept,
) -> Result<Option<oxy_oltp::resolver::BranchWriter>, ResolveError> {
    resolve_env_branch_writer_for_org(&n.db, n.org_id, app_id, ENV, &n.writer, accept).await
}

/// A row recording production's database is refused however it names it — by
/// the provider's branch id, or by production's own endpoint and database
/// under an innocent id — and whatever its status.
#[tokio::test]
async fn the_env_resolver_refuses_a_row_naming_production() {
    let n = neon().await;
    let tenant = n.tenant().await;
    let app_id = Uuid::new_v4();
    let row = insert_row(&n, app_id, EnvBranchStatus::Ready, |r| {
        r.host = ActiveValue::Set(Some(tenant.host.clone()));
    })
    .await;
    mint_writer_password(&n, &row).await;

    let by_endpoint = resolve(&n, app_id, EnvBranchAccept::Ready).await;
    assert!(
        matches!(by_endpoint, Err(ResolveError::EnvBranchIsProduction { .. })),
        "{by_endpoint:?}"
    );

    let mut by_id: env_branches::ActiveModel = row.into();
    by_id.host = ActiveValue::Set(Some(BRANCH_HOST.into()));
    by_id.provider_branch_id = ActiveValue::Set(Some(tenant.branch_id.clone()));
    by_id.status = ActiveValue::Set(EnvBranchStatus::Cutting);
    by_id.update(&n.db).await.unwrap();
    let by_id = resolve(&n, app_id, EnvBranchAccept::ReadyOrMigrating).await;
    assert!(
        matches!(by_id, Err(ResolveError::EnvBranchIsProduction { .. })),
        "production is refused before the status gate: {by_id:?}"
    );
}

#[tokio::test]
async fn the_env_resolver_serves_only_ready_unless_migrating_is_accepted() {
    let n = neon().await;
    let app_id = Uuid::new_v4();
    assert!(
        resolve(&n, app_id, EnvBranchAccept::Ready)
            .await
            .unwrap()
            .is_none(),
        "no row: the caller keeps its fallback"
    );

    let row = insert_row(&n, app_id, EnvBranchStatus::Cutting, |_| {}).await;
    mint_writer_password(&n, &row).await;
    let set_status = |status: EnvBranchStatus| {
        let mut m: env_branches::ActiveModel = row.clone().into();
        m.status = ActiveValue::Set(status);
        m.update(&n.db)
    };
    for (status, name) in [
        (EnvBranchStatus::Cutting, "cutting"),
        (EnvBranchStatus::Resetting, "resetting"),
        (EnvBranchStatus::Failed, "failed"),
        (EnvBranchStatus::Migrating, "migrating"),
    ] {
        set_status(status).await.unwrap();
        let err = resolve(&n, app_id, EnvBranchAccept::Ready)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ResolveError::EnvBranchNotReady { status, .. } if *status == name),
            "{name}: {err}"
        );
    }

    // The migration apply is served while migrating, and not while cutting.
    let writer = resolve(&n, app_id, EnvBranchAccept::ReadyOrMigrating)
        .await
        .unwrap()
        .expect("served while migrating");
    assert_eq!(writer.cut.scope, CutScope::Environment);
    assert_eq!(writer.cut.provider_branch_id, "br-env-1");
    set_status(EnvBranchStatus::Cutting).await.unwrap();
    assert!(matches!(
        resolve(&n, app_id, EnvBranchAccept::ReadyOrMigrating).await,
        Err(ResolveError::EnvBranchNotReady {
            status: "cutting",
            ..
        })
    ));

    set_status(EnvBranchStatus::Ready).await.unwrap();
    let dsn = resolve(&n, app_id, EnvBranchAccept::Ready)
        .await
        .unwrap()
        .expect("served when ready")
        .connection
        .dsn;
    assert_eq!(host_of(&dsn), BRANCH_HOST, "the environment's own endpoint");
    assert_eq!(
        password_of(&dsn),
        BRANCH_PASSWORD,
        "its own sealed password"
    );
    assert!(dsn.contains(&n.writer.schema_name()), "{dsn}");
}

/// A row the provider has not answered has no cut; however its status reads,
/// it is not ready — and it is never production's connection.
#[tokio::test]
async fn an_uncut_row_is_not_ready_and_a_missing_credential_is_named() {
    let n = neon().await;
    let app_id = Uuid::new_v4();
    let row = insert_row(&n, app_id, EnvBranchStatus::Ready, |r| {
        r.provider_branch_id = ActiveValue::Set(None);
        r.host = ActiveValue::Set(None);
        r.database_name = ActiveValue::Set(None);
        r.owner_role = ActiveValue::Set(None);
    })
    .await;
    assert!(matches!(
        resolve(&n, app_id, EnvBranchAccept::Ready).await,
        Err(ResolveError::EnvBranchNotReady {
            status: "ready",
            ..
        })
    ));

    let tenant = n.tenant().await;
    let mut cut: env_branches::ActiveModel = row.into();
    cut.provider_branch_id = ActiveValue::Set(Some("br-env-1".into()));
    cut.host = ActiveValue::Set(Some(BRANCH_HOST.into()));
    cut.database_name = ActiveValue::Set(Some(tenant.database_name.clone()));
    cut.owner_role = ActiveValue::Set(Some("env_owner".into()));
    cut.update(&n.db).await.unwrap();
    let err = resolve(&n, app_id, EnvBranchAccept::Ready)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ResolveError::EnvBranchCredentialMissing { .. }),
        "a cut whose writer was never minted a password: {err}"
    );
    assert!(
        env_branch_roles::Entity::find()
            .all(&n.db)
            .await
            .unwrap()
            .is_empty()
    );
}
