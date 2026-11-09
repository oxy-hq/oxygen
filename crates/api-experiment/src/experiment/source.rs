//! Where a panel — and everything else pricing reads — comes from. The job
//! depends on this trait so the warehouse, the one external boundary, can be a
//! DuckDB fixture in tests and a workspace's real connector in production.

use async_trait::async_trait;
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;

use super::panel::{PanelContext, PanelOutcome, PanelRequest, load_panel};
use super::reads::{ReadAsk, Readings, fetch_readings};

#[async_trait]
pub trait PanelSource: Send + Sync {
    /// `Ok(NoHistory)` for an empty answer; `Ok(Refused)` for a refusal as a
    /// value; `Err(OxyError::ValidationError)` for a model refusal a person can
    /// act on; any other `Err` is an infrastructure fault.
    async fn load(&self, ctx: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError>;

    /// Everything pricing reads for one candidate in one round trip: the group
    /// membership under `assign_by`, the panel, the lag and spread of the
    /// measures the file watches, and the driver edge's slope.
    ///
    /// The default loads the panel only and says so, and refuses `assign_by`: a
    /// source written for a caller that only needs [`PanelSource::load`] (PR 5's
    /// estimate job) keeps compiling, and one that cannot resolve groups fails
    /// loudly rather than pricing a unit design nobody asked for.
    async fn read(&self, ctx: &PanelContext, ask: &ReadAsk) -> Result<Readings, OxyError> {
        if ask.assign_by.is_some() {
            return Err(OxyError::RuntimeError(
                "this panel source cannot resolve groups, so it cannot price an `assign_by` candidate".to_string(),
            ));
        }
        let panel = self.load(ctx, &ask.panel).await?;
        Ok(Readings::panel_only(
            panel,
            "this panel source reads no lag, spread or prior",
        ))
    }
}

/// The production source: the workspace's own warehouse, through its promoted
/// semantic model. Which workspace, revision and user come with each request.
pub struct WorkspacePanelSource {
    pub db: DatabaseConnection,
}

#[async_trait]
impl PanelSource for WorkspacePanelSource {
    async fn load(&self, ctx: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        load_panel(&self.db, ctx, req).await
    }

    /// One workspace open, one blocking thread, every query pricing needs.
    async fn read(&self, ctx: &PanelContext, ask: &ReadAsk) -> Result<Readings, OxyError> {
        let opened = super::panel_workspace::open(&self.db, ctx).await?;
        let ask = ask.clone();
        let (user, handle) = (ctx.requested_by, tokio::runtime::Handle::current());
        tokio::task::spawn_blocking(move || {
            let (layer, executor) = opened.into_executor(user, handle);
            fetch_readings(&layer, &*executor, &ask)
        })
        .await
        .map_err(|e| OxyError::RuntimeError(format!("the pricing read panicked: {e}")))?
    }
}
