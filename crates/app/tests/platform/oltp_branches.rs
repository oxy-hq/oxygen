//! The org's OLTP staging branch through the real `OltpProvisioner` and the
//! real control plane, on a Neon-shaped provider.
//!
//! `MockProvider` stands in for Neon — own endpoint and owner password per
//! branch, adoption by name, reset in place — and a recording executor stands
//! in for the tenant SQL, so what is asserted is the part that can go wrong
//! here: which database each statement ran on and as whom, what was sealed,
//! and what the resolver hands back. The local-cluster half (a real copied
//! database) is `oltp_branches_local.rs`.

use std::sync::{Arc, Mutex};

use entity::custom_app_migrations;
use oxy_oltp::entity::branch_roles::Entity as BranchRoles;
use oxy_oltp::entity::branches::{self as oltp_branches, BranchStatus, Entity as OltpBranches};
use oxy_oltp::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};
use oxy_oltp::provider::MockProvider;
use oxy_oltp::resolver::{
    ResolveError, resolve_branch_writer_connection_for_org, resolve_writer_connection_for_org,
};
use oxy_oltp::sql::{SqlError, TenantSqlExecutor};
use oxy_oltp::{GrantLevel, OltpBranch, OltpProvisioner, ProvisionerError, WriterRef};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

pub(crate) const STAGING: OltpBranch = OltpBranch::Staging;

/// Records `(dsn, statements)` instead of running them.
#[derive(Default)]
pub(crate) struct Recorder(pub(crate) Mutex<Vec<(String, Vec<String>)>>);

#[async_trait::async_trait]
impl TenantSqlExecutor for Recorder {
    async fn execute_batch(&self, dsn: &str, statements: &[String]) -> Result<(), SqlError> {
        self.0
            .lock()
            .unwrap()
            .push((dsn.to_string(), statements.to_vec()));
        Ok(())
    }
}

impl Recorder {
    pub(crate) fn on_host(&self, host: &str) -> Vec<(String, Vec<String>)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(dsn, _)| host_of(dsn) == host)
            .cloned()
            .collect()
    }
}

pub(crate) fn host_of(dsn: &str) -> String {
    let cfg: tokio_postgres::Config = dsn.parse().expect("a DSN the provisioner built");
    match &cfg.get_hosts()[0] {
        tokio_postgres::config::Host::Tcp(h) => h.clone(),
        other => panic!("unexpected host {other:?}"),
    }
}

pub(crate) fn password_of(dsn: &str) -> String {
    let cfg: tokio_postgres::Config = dsn.parse().expect("a DSN the provisioner built");
    String::from_utf8(cfg.get_password().expect("has a password").to_vec()).unwrap()
}

pub(crate) struct Neon {
    pub(crate) db: DatabaseConnection,
    pub(crate) provider: Arc<MockProvider>,
    pub(crate) sql: Arc<Recorder>,
    pub(crate) prov: OltpProvisioner,
    pub(crate) org_id: Uuid,
    pub(crate) writer: WriterRef,
}

impl Neon {
    pub(crate) async fn tenant(&self) -> oltp_tenants::Model {
        OltpTenants::find()
            .filter(oltp_tenants::Column::OrgId.eq(self.org_id))
            .one(&self.db)
            .await
            .unwrap()
            .expect("tenant")
    }

    pub(crate) async fn branch(&self) -> Option<oltp_branches::Model> {
        let tenant = self.tenant().await;
        OltpBranches::find()
            .filter(oltp_branches::Column::TenantRowId.eq(tenant.id))
            .one(&self.db)
            .await
            .unwrap()
    }

    pub(crate) async fn staging_dsn(&self) -> Result<Option<String>, ResolveError> {
        resolve_branch_writer_connection_for_org(&self.db, self.org_id, STAGING, &self.writer)
            .await
            .map(|c| c.map(|c| c.dsn))
    }
}

/// An org with a provisioned database and one app writer, `store_ops`.
pub(crate) async fn neon() -> Neon {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::CentralOltp).await;
    neon_on(db).await
}

/// [`neon`] over a control-plane connection the caller prepared.
pub(crate) async fn neon_on(db: DatabaseConnection) -> Neon {
    let org_id = crate::oltp_provisioner::seed_org(&db).await;
    let provider = Arc::new(MockProvider::new());
    let sql = Arc::new(Recorder::default());
    let prov = OltpProvisioner::new(
        db.clone(),
        provider.clone(),
        sql.clone(),
        "aws-us-east-2",
        17,
    );
    prov.provision(org_id).await.expect("provision");
    let writer = WriterRef::app("store_ops").unwrap();
    prov.ensure_writer(org_id, &writer, GrantLevel::ReadWrite, None)
        .await
        .expect("writer");
    Neon {
        db,
        provider,
        sql,
        prov,
        org_id,
        writer,
    }
}

pub(crate) async fn ledger_rows(db: &DatabaseConnection, target: &str) -> usize {
    custom_app_migrations::Entity::find()
        .filter(custom_app_migrations::Column::Target.eq(target))
        .all(db)
        .await
        .unwrap()
        .len()
}

/// An app in the org with an applied migration on production AND on `target`.
pub(crate) async fn app_with_ledger(
    db: &DatabaseConnection,
    org_id: Uuid,
    targets: &[&str],
) -> Uuid {
    let workspace = Uuid::new_v4();
    entity::workspaces::ActiveModel {
        id: ActiveValue::Set(workspace),
        org_id: ActiveValue::Set(Some(org_id)),
        name: ActiveValue::Set("ws".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app_id = Uuid::new_v4();
    entity::apps::ActiveModel {
        id: ActiveValue::Set(app_id),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace),
        slug: ActiveValue::Set("store-ops".into()),
        name: ActiveValue::Set("Store Ops".into()),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("git@example.com:acme/store-ops.git".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("git".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        visibility: ActiveValue::Set("org".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    for target in targets {
        custom_app_migrations::ActiveModel {
            app_id: ActiveValue::Set(app_id),
            store: ActiveValue::Set("oltp".into()),
            target: ActiveValue::Set((*target).to_string()),
            filename: ActiveValue::Set("0001_init.sql".into()),
            checksum: ActiveValue::Set("abc".into()),
            applied_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
            applied_by_build: ActiveValue::Set(None),
        }
        .insert(db)
        .await
        .expect("seed ledger row");
    }
    app_id
}

#[tokio::test]
async fn a_branch_gets_its_own_endpoint_and_credentials_and_production_is_untouched() {
    let n = neon().await;
    let production_before = resolve_writer_connection_for_org(&n.db, n.org_id, &n.writer)
        .await
        .unwrap()
        .dsn;

    let row = n
        .prov
        .provision_branch(n.org_id, STAGING)
        .await
        .expect("branch");
    let tenant = n.tenant().await;
    assert_eq!(row.status, BranchStatus::Active);
    assert_ne!(
        row.host, tenant.host,
        "a branch is reached on its own endpoint"
    );
    assert_eq!(
        row.parent_branch_id, tenant.branch_id,
        "cut from production"
    );

    // Every statement aimed at the branch ran as the BRANCH's owner — never
    // with production's owner password, which would mean the branch still
    // accepts it.
    let branch_owner = n
        .provider
        .peek_password(&tenant.project_id, &row.provider_branch_id, "oxy_owner")
        .unwrap();
    let production_owner = n
        .provider
        .peek_password(&tenant.project_id, &tenant.branch_id, "oxy_owner")
        .unwrap();
    assert_ne!(branch_owner, production_owner);
    let on_branch = n.sql.on_host(&row.host);
    assert!(!on_branch.is_empty(), "the branch was converged");
    for (dsn, _) in &on_branch {
        assert_eq!(password_of(dsn), branch_owner, "{dsn}");
    }

    // The writer's branch password is its own, and is the one set ON the branch.
    let staging = n.staging_dsn().await.unwrap().expect("a branch connection");
    assert_eq!(host_of(&staging), row.host);
    let staging_pw = password_of(&staging);
    assert_ne!(staging_pw, password_of(&production_before));
    assert!(
        on_branch
            .iter()
            .flat_map(|(_, s)| s)
            .any(|s| s.contains("app_store_ops_rw") && s.contains(&staging_pw)),
        "the sealed branch password must be the one minted on the branch"
    );
    assert_eq!(
        BranchRoles::find().all(&n.db).await.unwrap().len(),
        2,
        "the analyst and the writer"
    );

    // Production resolution is exactly what it was.
    let production_after = resolve_writer_connection_for_org(&n.db, n.org_id, &n.writer)
        .await
        .unwrap()
        .dsn;
    assert_eq!(production_after, production_before);
}

#[tokio::test]
async fn provisioning_again_mints_nothing_and_buys_nothing() {
    let n = neon().await;
    n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let dsn = n.staging_dsn().await.unwrap().unwrap();
    let mints = |r: &Recorder| {
        r.0.lock()
            .unwrap()
            .iter()
            .flat_map(|(_, s)| s.clone())
            .filter(|s| s.contains("WITH LOGIN PASSWORD"))
            .count()
    };
    let before = mints(&n.sql);

    n.prov.provision_branch(n.org_id, STAGING).await.unwrap();

    assert_eq!(n.provider.branch_count(), 1);
    assert_eq!(
        mints(&n.sql),
        before,
        "a re-run must not rotate a live staging credential"
    );
    assert_eq!(n.staging_dsn().await.unwrap().unwrap(), dsn);
}

#[tokio::test]
async fn resolution_is_none_without_a_branch_and_refuses_one_mid_reset() {
    let n = neon().await;
    assert!(
        n.staging_dsn().await.unwrap().is_none(),
        "no branch: the caller keeps production read-only"
    );

    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let mut resetting: oltp_branches::ActiveModel = row.into();
    resetting.status = ActiveValue::Set(BranchStatus::Resetting);
    let row = resetting.update(&n.db).await.unwrap();
    assert!(matches!(
        n.staging_dsn().await,
        Err(ResolveError::BranchNotActive(
            _,
            OltpBranch::Staging,
            "resetting"
        ))
    ));
    // And a provision will not paper over a half-finished reset.
    assert!(matches!(
        n.prov.provision_branch(n.org_id, STAGING).await,
        Err(ProvisionerError::BranchNotReady { .. })
    ));

    let mut active: oltp_branches::ActiveModel = row.into();
    active.status = ActiveValue::Set(BranchStatus::Active);
    let row = active.update(&n.db).await.unwrap();
    BranchRoles::delete_many()
        .filter(oxy_oltp::entity::branch_roles::Column::BranchRowId.eq(row.id))
        .exec(&n.db)
        .await
        .unwrap();
    assert!(matches!(
        n.staging_dsn().await,
        Err(ResolveError::BranchCredentialMissing { .. })
    ));
}

#[tokio::test]
async fn a_branch_needs_a_provisioned_database_and_a_reset_needs_a_branch() {
    let n = neon().await;
    let err = n.prov.reset_branch(n.org_id, STAGING).await.unwrap_err();
    assert!(
        matches!(err, ProvisionerError::BranchNotProvisioned(..)),
        "{err}"
    );
    assert!(err.to_string().contains("--branch staging"), "{err}");

    let other = crate::oltp_provisioner::seed_org(&n.db).await;
    let err = n.prov.provision_branch(other, STAGING).await.unwrap_err();
    assert!(
        matches!(err, ProvisionerError::NotProvisioned(id) if id == other),
        "{err}"
    );
    assert_eq!(n.provider.branch_count(), 0, "nothing was bought for it");
}

#[tokio::test]
async fn the_status_read_names_the_apps_and_pipelines_a_reset_would_hit() {
    let n = neon().await;
    app_with_ledger(&n.db, n.org_id, &[]).await;
    let pipeline = WriterRef::pipeline("toast").unwrap();
    n.prov
        .ensure_writer(n.org_id, &pipeline, GrantLevel::ReadWrite, None)
        .await
        .expect("pipeline writer");
    n.prov.provision_branch(n.org_id, STAGING).await.unwrap();

    let status = oxy_oltp::api::branches::status_for(&n.db, n.org_id, STAGING)
        .await
        .unwrap();

    assert!(status.provisioned);
    assert_eq!(status.age_days, Some(0));
    assert!(!status.stale);
    let slugs: Vec<&str> = status
        .affected_apps
        .iter()
        .map(|a| a.slug.as_str())
        .collect();
    assert_eq!(slugs, ["store-ops"]);
    let raw: Vec<(&str, &str)> = status
        .affected_pipelines
        .iter()
        .map(|p| (p.source.as_str(), p.schema.as_str()))
        .collect();
    assert_eq!(
        raw,
        [("toast", "raw_toast")],
        "a reset discards Airway's staging copy too"
    );
}
