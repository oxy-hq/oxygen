//! A real `ProjectFunctionHost` over a workspace, built where the caller says —
//! inside a staging pin or not, over a manager at a given revision or the
//! working copy. Only `ctx.airway.run` is exercised.

use std::path::Path;
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::OnMissing;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::api::custom_apps_functions::InvocationIdentity;
use oxy_app::server::api::custom_apps_functions::env_policy::EnvPolicy;
use oxy_app::server::api::custom_apps_functions::host::{
    FunctionCapabilities, ProjectFunctionHost, into_arc,
};
use oxy_app::server::api::custom_apps_functions::runtime::FunctionHost;
use oxy_app::server::api::custom_apps_functions::seam::{
    FunctionProjectContext, FunctionQueryExecutor,
};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

struct NoQueries;

#[async_trait::async_trait]
impl FunctionQueryExecutor for NoQueries {
    async fn execute(
        &self,
        _connector: Arc<dyn DatabaseConnector>,
        _sql: &str,
        _max_rows: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        Err("not exercised".into())
    }
}

/// A host for `workspace`, its manager built at `revision` (`None`: the
/// working copy at `root`).
pub(super) async fn host(
    db: &DatabaseConnection,
    workspace: Uuid,
    root: &Path,
    revision: Option<Uuid>,
) -> Arc<dyn FunctionHost> {
    let manager = WorkspaceBuilder::new(workspace)
        .with_working_copy(root, revision, OnMissing::Fail)
        .await
        .expect("config loads")
        .build()
        .await
        .expect("workspace manager");
    let project = Arc::new(OxyProjectContext::new(manager));
    into_arc(ProjectFunctionHost::new(
        project as Arc<dyn FunctionProjectContext>,
        Arc::new(NoQueries),
        db.clone(),
        vec![],
        workspace,
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::nil(),
        "Staging".into(),
        FunctionCapabilities::default(),
        Default::default(),
        oxy_app::server::api::operating_graph::reach::Reach::nowhere(),
        InvocationIdentity {
            invocation_id: Uuid::new_v4(),
            function_name: "kick-off-elt".into(),
            mode: "route".into(),
            request_id: None,
            app_slug: "staging".into(),
            user_id: None,
            user_email: None,
            credential_token_id: None,
        },
        // Admitted to production: what makes a branch read here S9's case,
        // decided by `EnvPolicy::decide_on_branch`.
        EnvPolicy::production(),
    ))
}
