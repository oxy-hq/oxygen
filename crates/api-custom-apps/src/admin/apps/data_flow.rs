//! `GET /api/customer-apps/data-flow?org_id=&workspace_id=&focus=&depth=&level=`
//! — how data moves through one org: ingestion sources, Airway pipelines,
//! Airhouse schemas and the org's OLTP store, and the custom apps that write,
//! ship and watch them. The admin console's **Data flow** page draws it.
//!
//! The console's app map (`recent_traces`) answers "what does this app reach?"
//! for one app, from its recorded runs. This answers "where does this org's
//! data come from and where does it go?" across every app and pipeline, from
//! what each one **declares**. Phase 1 is declared edges only: every edge here
//! is `declared: true`, and nothing reads a trace or a run. What the runs
//! observed (and where they drift from the declarations) is a later overlay.
//!
//! **Where each edge comes from** (all of it is the platform DB):
//!
//! | Declared by | Edge |
//! | --- | --- |
//! | a compiled `.airway.yml` (`airway_pipelines.definition`, the workspace's promoted revision) | source `reads` → pipeline, pipeline `writes` → its destination schema |
//! | the live build's `airhouseMigrations` | app `writes` → `app_<writer>` in Airhouse |
//! | the live build's `migrations` | app `writes` → `app_<writer>` in the org's OLTP store |
//! | the live build's `oltpChanges` | OLTP table `ships` → `oltp_<writer>` in Airhouse |
//! | a function's `destinations` | function `writes` → that database |
//! | a function's `airhouse` / `oltp` capability | function `writes` → the app's own schema |
//! | a function's `on: { table }` | the watched table `triggers` → function |
//! | a function's `oxygen.sourceUploads` | function `triggers` → pipeline, `approved` when an org admin's grant (`app_platform_grants`) stands |
//! | the caller's `calls` | caller app `calls` → the callee's function (`ctx.apps.call` and `ctx.apps.send` share the declaration) |
//! | a function's `on: "<app>/<event>"` | each function whose `events.publish` names the event `publishes` → the subscriber |
//!
//! The app-to-app edges are [`messaging`]'s; a declaration the platform would
//! refuse is drawn with its reason in `problems`.
//!
//! Edges point the way the data moves. The live build is production's, else
//! staging's for an app never promoted; each app node says which.
//!
//! **Shape.** Collapsed to schemas by default (`level=table` splits an edge
//! that names tables into one per table). Functions fold into their app unless
//! the focus is that app or one of its functions. `focus` + `depth` (default
//! [`DEFAULT_DEPTH`], at most [`MAX_DEPTH`]) keep the neighbourhood of one
//! node; past [`MAX_NODES`] nodes the graph is cut and `truncated` is set.
//!
//! **Fences.** The surface's staff guards, then this route's own
//! `Action::PlatformOperate` (the page sits beside Airway, and an App Operator
//! ships apps, not pipelines); then the org must be in the caller's grant —
//! `404`, never `403`, so ids cannot be probed. A publish token is refused:
//! this reads beyond the custom-apps surface.
//!
//! **Bounds.** After the grant check, eight statements, each scoped to the one
//! org and none of them per row. DB-only (FleetOk).

mod assemble;
mod load;
mod messaging;
mod shape;

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::Query;
use axum::extract::rejection::QueryRejection;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::environment_scope::{self, ScopeError};
use super::handlers::org_in_scope;

/// Hops from the focus when the caller names none.
pub(crate) const DEFAULT_DEPTH: u32 = 2;
/// The most hops one read walks from the focus.
pub(crate) const MAX_DEPTH: u32 = 6;
/// The most nodes one answer carries; past it the graph is cut, `truncated`.
pub(crate) const MAX_NODES: usize = 500;
/// The longest `focus` accepted: a node id is a prefix, a UUID and a name.
const MAX_FOCUS_LEN: usize = 256;

/// `?org_id=&workspace_id=&focus=&depth=&level=`.
#[derive(Debug, Default, Deserialize)]
pub struct DataFlowQuery {
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub focus: Option<String>,
    pub depth: Option<u32>,
    pub level: Option<String>,
}

/// The org's declared data flow.
#[derive(Debug, Serialize)]
pub struct DataFlow {
    pub generated_at: DateTime<Utc>,
    pub org: OrgRef,
    /// Every workspace of the org, by name, whatever `workspace_id` filtered.
    pub workspaces: Vec<WorkspaceRef>,
    /// More nodes qualified than [`MAX_NODES`]; the rest were left out.
    pub truncated: bool,
    pub level: Level,
    /// In id order.
    pub nodes: Vec<FlowNode>,
    /// In `(from, to, kind)` order.
    pub edges: Vec<FlowEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrgRef {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkspaceRef {
    pub id: Uuid,
    pub name: String,
}

/// How finely the stores are drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// One node per schema; the tables an edge names ride in its `tables`.
    #[default]
    Schema,
    /// One node per named table; a schema stays a node where no table is named.
    Table,
}

/// One node. Ids are stable strings: `source:<ws>:<pipeline>`,
/// `pipe:<ws>:<name>`, `schema:<ws>[:<database>]:<schema>`,
/// `table:<ws>[:<database>]:<schema>.<table>`, `oltp:<schema>[.<table>]`,
/// `store:<ws>:<database>`, `app:<id>`, `fn:<app>:<name>`. A warehouse schema
/// carries the database it is named in (a pipeline's `database:`, a watch's
/// `<database>:`), so one schema name in two databases is two nodes; without
/// one it is the workspace's own Airhouse.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: NodeKind,
    pub label: String,
    /// A function's app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub meta: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Source,
    Pipeline,
    /// A database named whole: a function's write destination, or a
    /// pipeline's inline connector.
    Store,
    Schema,
    Table,
    App,
    Function,
}

/// Which way the data moves along an edge, and who moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// `to` reads `from`.
    Reads,
    /// `from` writes into `to`.
    Writes,
    /// `from` starts `to`: a watched table wakes a function, an upload feeds a
    /// pipeline's landing zone.
    Triggers,
    /// The platform copies `from` into `to` (the OLTP change stream relay).
    Ships,
    /// `from` runs `to`, another app's function: `ctx.apps.call` (answered
    /// now) or `ctx.apps.send` (queued). One `calls` declaration admits both.
    Calls,
    /// `from` publishes events `to` listens to (`on: "<app>/<event>"`),
    /// delivered later by the event relay.
    Publishes,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    /// Always `true` in phase 1: every edge is a declaration.
    pub declared: bool,
    /// What declared it: `airway`, `airhouseMigrations`, `migrations`,
    /// `oltpChanges`, `destinations`, `airhouse`, `oltp`, `on`,
    /// `oxygen.sourceUploads`, `calls`, `events`.
    pub via: BTreeSet<String>,
    /// The tables it names, at schema level.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub tables: BTreeSet<String>,
    /// The functions folded into the `from` app: on a `publishes` edge, the
    /// publishers.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub from_functions: BTreeSet<String>,
    /// The functions folded into the `to` app: on a `publishes` edge, the
    /// subscribers; on a `calls` edge, the functions called.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub to_functions: BTreeSet<String>,
    /// For an upload into a pipeline: whether an org admin approved it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved: Option<bool>,
    /// The event types a `publishes` edge carries.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub events: BTreeSet<String>,
    /// Why the platform would refuse what this edge declares:
    /// `app_not_found`, `function_not_found`, `not_callable`, `not_in_catalog`,
    /// `never_published`. Empty for an edge that would run.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub problems: BTreeSet<String>,
}

/// The query, checked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Params {
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub focus: Option<String>,
    pub depth: u32,
    pub level: Level,
}

fn bad_request(code: &'static str, message: impl Into<String>) -> ScopeError {
    ScopeError::new(StatusCode::BAD_REQUEST, code, message)
}

fn not_found(code: &'static str, message: &str) -> ScopeError {
    ScopeError::new(StatusCode::NOT_FOUND, code, message)
}

impl Params {
    /// Out of range is refused rather than clamped, as `parse_per_function`
    /// reasons: a caller asking for more than it gets should hear so.
    pub(crate) fn parse(q: DataFlowQuery) -> Result<Self, ScopeError> {
        let org_id = q
            .org_id
            .ok_or_else(|| bad_request("missing_org_id", "org_id is required"))?;
        let depth = match q.depth {
            None => DEFAULT_DEPTH,
            Some(d) if (1..=MAX_DEPTH).contains(&d) => d,
            Some(d) => {
                return Err(bad_request(
                    "invalid_depth",
                    format!("depth must be between 1 and {MAX_DEPTH}; got {d}"),
                ));
            }
        };
        let level = match q.level.as_deref() {
            None | Some("schema") => Level::Schema,
            Some("table") => Level::Table,
            Some(other) => {
                return Err(bad_request(
                    "invalid_level",
                    format!("level must be `schema` or `table`; got `{other}`"),
                ));
            }
        };
        let focus = q.focus.filter(|f| !f.trim().is_empty());
        if focus.as_ref().is_some_and(|f| f.len() > MAX_FOCUS_LEN) {
            return Err(bad_request("invalid_focus", "focus is not a node id"));
        }
        Ok(Self {
            org_id,
            workspace_id: q.workspace_id,
            focus,
            depth,
            level,
        })
    }
}

/// `GET /data-flow` — the org's declared graph, shaped by `focus`, `depth`
/// and `level`.
pub async fn get_data_flow(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    query: Result<Query<DataFlowQuery>, QueryRejection>,
) -> Result<Json<DataFlow>, ScopeError> {
    if marker.is_some() {
        return Err(not_found("not_found", "no such route for a publish token"));
    }
    let Query(q) = query.map_err(|e| bad_request("invalid_query", e.to_string()))?;
    let params = Params::parse(q)?;
    let db = environment_scope::connect().await?;
    let in_scope = org_in_scope(&db, &user, params.org_id)
        .await
        .map_err(|_| ScopeError::internal("platform grant unreadable", "data-flow scope"))?;
    if !in_scope {
        return Err(not_found("org_not_found", "no such organization"));
    }
    let load::Loaded { org, inputs } = load::inputs(&db, &params).await?;
    let graph = assemble::assemble(&inputs);
    let shaped = shape::shape(graph, params.focus.as_deref(), params.depth, MAX_NODES)
        .ok_or_else(|| not_found("focus_not_found", "the focus is not a node of this graph"))?;
    Ok(Json(DataFlow {
        generated_at: Utc::now(),
        org,
        workspaces: inputs
            .workspaces
            .iter()
            .map(|w| WorkspaceRef {
                id: w.id,
                name: w.name.clone(),
            })
            .collect(),
        truncated: shaped.truncated,
        level: params.level,
        nodes: shaped.nodes,
        edges: shaped.edges,
    }))
}

#[cfg(test)]
mod tests;
