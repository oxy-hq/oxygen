//! `FunctionProjectContext` impl for [`OxyProjectContext`].
//!
//! The Oxy Functions runtime (`custom_apps_functions`) depends on the
//! `FunctionProjectContext` trait in the always-compiled `custom_apps_functions::seam`
//! module, not on this adapter — so its files don't import `agentic_wiring`. This is
//! the production impl, named
//! here (outside the custom-apps boundary) and reaching the runtime as a trait
//! object built from the context `custom_apps_gates::build_project_context`
//! already resolves per invocation.

use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use agentic_pipeline::airway_run::{StartAirwayRequest, start_airway_run};
use agentic_pipeline::platform::ProjectContext;
use agentic_runtime::crud::TaskScope;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;

use super::OxyProjectContext;
use crate::server::api::custom_apps_functions::seam::FunctionProjectContext;
use oxy::config::WorkingCopy;

#[async_trait::async_trait]
impl FunctionProjectContext for OxyProjectContext {
    fn workspace_manager(&self) -> &WorkspaceManager<WorkingCopy> {
        // Inherent method — disambiguated from this trait method of the same name.
        OxyProjectContext::workspace_manager(self)
    }

    async fn build_connector_for(
        &self,
        db_name: &str,
    ) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
        OxyProjectContext::build_connector_for(self, db_name).await
    }

    async fn build_app_airhouse_connector(
        &self,
        app_slug: &str,
        schema: &str,
    ) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
        crate::agentic_wiring::app_airhouse::connector(self.workspace_id(), app_slug, schema).await
    }

    async fn start_airway_seed(
        &self,
        db: &DatabaseConnection,
        request: StartAirwayRequest,
    ) -> Result<String, String> {
        // `TaskScope::Global` seeds the run on the global queue with NO
        // co-located coordinator — the worker fleet drives it to completion —
        // so this returns the run id immediately (an ELT run routinely
        // outlasts a function's timeout; see the caller in `host::airway_run`).
        let workspace_id = self.workspace_id();
        start_airway_run(db, self, request, TaskScope::Global, workspace_id)
            .await
            .map_err(|e| format!("failed to start airway run: {e}"))
    }
}
