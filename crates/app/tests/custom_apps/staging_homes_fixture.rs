//! The harness the P5b staging-home tests share: a real `ProjectFunctionHost`
//! over a workspace whose `config.yml` names real engines, built for staging or
//! production with the build's `nonProduction.destinations`. No V8, as in
//! `warehouse_writes_on_engines`. Holds no tests.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use agentic_core::result::CellValue;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::{OnMissing, WorkingCopy};
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::api::custom_apps_functions::InvocationIdentity;
use oxy_app::server::api::custom_apps_functions::env_policy::EnvPolicy;
use oxy_app::server::api::custom_apps_functions::host::{
    FunctionCapabilities, ProjectFunctionHost, WriterCapability, into_arc,
};
use oxy_app::server::api::custom_apps_functions::runtime::FunctionHost;
use oxy_app::server::api::custom_apps_functions::seam::{
    FunctionProjectContext, FunctionQueryExecutor,
};
use oxy_app::server::api::projects::query::DataPlaneQueryExecutor;
use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

/// The app every host here runs as; its Airhouse schema and staging's sibling.
pub(crate) const APP_SLUG: &str = "receiving";
pub(crate) const APP_SCHEMA: &str = "app_receiving";
pub(crate) const SIBLING: &str = "app_receiving__staging";

/// A workspace whose `config.yml` declares `databases`, and its project context.
pub(crate) struct Workspace {
    pub(crate) project: Arc<OxyProjectContext>,
    _root: tempfile::TempDir,
}

pub(crate) async fn workspace(databases: &str) -> Workspace {
    let root = tempfile::tempdir().expect("workspace dir");
    std::fs::write(
        root.path().join("config.yml"),
        format!("databases:\n{databases}models: []\n"),
    )
    .expect("write config.yml");
    let project = Arc::new(OxyProjectContext::new(manager(root.path()).await));
    Workspace {
        project,
        _root: root,
    }
}

pub(crate) async fn manager(root: &Path) -> WorkspaceManager<WorkingCopy> {
    WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(root, None, OnMissing::Fail)
        .await
        .expect("config.yml loads")
        .build()
        .await
        .expect("workspace manager")
}

pub(crate) fn duck(name: &str) -> String {
    format!("  - name: {name}\n    type: duckdb\n    path: {name}.duckdb\n")
}

/// Staging's policy, with the build's `nonProduction.destinations`.
pub(crate) fn staging(mapping: &[(&str, &str)]) -> EnvPolicy {
    EnvPolicy::for_environment(AppEnvironment::Staging).with_destinations(
        mapping
            .iter()
            .map(|(from, to)| (from.to_string(), to.to_string()))
            .collect(),
    )
}

/// Production's policy, handed the same mapping — which it ignores.
pub(crate) fn production(mapping: &[(&str, &str)]) -> EnvPolicy {
    EnvPolicy::production().with_destinations(
        mapping
            .iter()
            .map(|(from, to)| (from.to_string(), to.to_string()))
            .collect(),
    )
}

/// A host whose function may write `allowed`, each a customer warehouse with a
/// reason when it is in `reasons`, and whose `ctx.airhouse` is enabled. Its
/// control-plane database is disconnected: only an audit row touches it, and
/// the buffered ones are written after these tests look.
pub(crate) fn host(
    project: Arc<dyn FunctionProjectContext>,
    allowed: &[&str],
    reasons: &[&str],
    policy: EnvPolicy,
) -> Arc<dyn FunctionHost> {
    host_on(
        sea_orm::DatabaseConnection::default(),
        project,
        allowed,
        reasons,
        policy,
    )
}

/// [`host`] on a real control-plane database — a `ctx.tx` commit writes its
/// audit row as it commits.
pub(crate) fn host_on(
    db: sea_orm::DatabaseConnection,
    project: Arc<dyn FunctionProjectContext>,
    allowed: &[&str],
    reasons: &[&str],
    policy: EnvPolicy,
) -> Arc<dyn FunctionHost> {
    let caps = FunctionCapabilities {
        customer_warehouse_writes: reasons
            .iter()
            .map(|d| (d.to_string(), "the P5b differential test writes it".into()))
            .collect::<BTreeMap<_, _>>(),
        airhouse: WriterCapability::resolve(true, APP_SLUG),
        ..Default::default()
    };
    into_arc(ProjectFunctionHost::new(
        project,
        Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>,
        db,
        allowed.iter().map(|d| d.to_string()).collect(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::nil(),
        "Receiving".into(),
        caps,
        Default::default(),
        oxy_app::server::api::operating_graph::reach::Reach::nowhere(),
        InvocationIdentity {
            invocation_id: Uuid::new_v4(),
            function_name: "submit-receiving-report".into(),
            mode: "route".into(),
            request_id: None,
            app_slug: APP_SLUG.into(),
            user_id: None,
            user_email: None,
        },
        policy,
    ))
}

pub(crate) async fn exec(connector: &dyn DatabaseConnector, sql: &str) {
    connector
        .execute_statement(sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// The `a` column of `table`, sorted.
pub(crate) async fn ids(connector: &dyn DatabaseConnector, table: &str) -> Vec<i64> {
    let result = connector
        .execute_query(&format!("SELECT a FROM {table} ORDER BY a"), 100)
        .await
        .unwrap_or_else(|e| panic!("read {table}: {e}"));
    result
        .result
        .rows
        .iter()
        .map(|row| match &row.0[0] {
            CellValue::Number(n) => *n as i64,
            CellValue::Text(s) => s.parse().expect("an integer"),
            other => panic!("unexpected cell {other:?}"),
        })
        .collect()
}

impl Workspace {
    pub(crate) async fn connector(&self, database: &str) -> Arc<dyn DatabaseConnector> {
        self.project
            .build_connector_for(database)
            .await
            .expect("connector")
    }

    pub(crate) async fn ids(&self, database: &str) -> Vec<i64> {
        ids(&*self.connector(database).await, "t").await
    }

    pub(crate) fn ctx(&self) -> Arc<dyn FunctionProjectContext> {
        self.project.clone() as Arc<dyn FunctionProjectContext>
    }
}

/// A per-test Postgres database as a `config.yml` entry named `name`, reached
/// as a login role of its own — a staging database on **its own credential**,
/// which the host requires of a mapped destination (the same host and user
/// as production's is refused). The password goes by reference, as
/// `warehouse_writes_on_engines::postgres_entry` explains.
pub(crate) async fn postgres_entry_as_own_role(name: &str) -> String {
    use sea_orm::ConnectionTrait;
    let (db, url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    let url = url::Url::parse(&url).expect("fresh_db hands back a URL");
    let database = url.path().trim_start_matches('/').to_string();
    let role = format!("p5b_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let password = Uuid::new_v4().simple().to_string();
    for sql in [
        format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"),
        format!("GRANT ALL ON DATABASE \"{database}\" TO {role}"),
        format!("GRANT ALL ON SCHEMA public TO {role}"),
    ] {
        db.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let var = format!("OXY_TEST_P5B_PG_PASSWORD_{}", role.to_ascii_uppercase());
    // SAFETY: nextest runs each test in its own process, and this is set before
    // any connector in it resolves the variable.
    unsafe { std::env::set_var(&var, &password) };
    format!(
        "  - name: {name}\n    type: postgres\n    host: {host}\n    port: \"{port}\"\n    \
         user: {role}\n    password_var: {var}\n    database: {database}\n",
        host = url.host_str().expect("host"),
        port = url.port().unwrap_or(5432),
    )
}
