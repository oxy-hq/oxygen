//! HTTP surface for experiments — pricing (PR 3), run status and abort (PR 4),
//! the sealed result read and write-back apply (PR 5).
//!
//! Thin by construction: extract, call `server::experiment`, serialize. Every
//! route reads or writes Postgres records (`experiments`, `experiment_designs`,
//! `experiment_proposals`, `experiment_runs`) or enqueues a `TaskSpec`, so those
//! are `FleetOk`; so is the PR 5 result read
//! (`result.rs`). Write-back apply (`write_back.rs`) creates a git worktree and
//! branch in the working copy, so it is `IdeOnly`.

pub mod designs;
pub mod interim;
pub mod list;
pub mod price;
pub mod proposals;
pub mod result;
pub mod retrospective;
pub mod run;
pub mod write_back;

use axum::http::StatusCode;
use sea_orm::DatabaseConnection;

pub use designs::{
    create_experiment, duplicate_experiment, get_design, post_register, put_design, use_proposal,
};
pub use interim::{get_interim, post_interim};
pub use list::list_experiments;
pub use price::post_price;
pub use proposals::list_proposals;
pub use result::get_result;
pub use retrospective::post_retrospective;
pub use run::{abort_run, get_run};
pub use write_back::apply_write_back;

pub(crate) type ApiError = (StatusCode, String);

pub(crate) async fn connect() -> Result<DatabaseConnection, ApiError> {
    oxy::database::client::establish_connection()
        .await
        .map_err(internal("connect"))
}

pub(crate) fn internal<E: std::fmt::Display>(what: &'static str) -> impl Fn(E) -> ApiError {
    move |e| {
        tracing::error!(error = %e, "experiment: {what} failed");
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{what} failed"))
    }
}
