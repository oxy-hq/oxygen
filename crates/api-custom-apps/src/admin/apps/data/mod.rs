//! `GET /api/customer-apps/{id}/data/tables` and `…/data/rows` — a
//! **read-only** browser of an app's own OLTP data, in the console's
//! environment (`?environment=`, production when absent).
//!
//! - `tables`: the app's schema's tables, each with Postgres's row estimate
//!   and its columns and types — or, for an environment with no store of its
//!   own, why not (`store.state = "absent"`).
//! - `rows?table=<name>&limit=&offset=&order=<column>&dir=asc|desc`: one page
//!   of one of those tables, at most 200 rows.
//!
//! **Nothing here writes.** There is no write route, the reads run in a
//! `READ ONLY` transaction, and the SQL is the catalog read and one `SELECT`
//! built from catalog names ([`read`]).
//!
//! **Confined to the app's own schema.** The store and its schema come from
//! the app and the environment ([`home`]): the app's writer, whose schema is
//! derived from its slug. A table or column a request names is checked
//! against that schema's catalog and only the catalog's spelling reaches SQL
//! ([`catalog`]), so another app's or org's tables cannot be named.
//!
//! **Gated like the tail of an app's function output, and locked by a
//! lockdown** ([`admission`]), and **every read that reaches a store is
//! audited before its result leaves** ([`audit`]).
//!
//! Postgres reads only — no workspace filesystem — so FleetOk with the nest.

mod admission;
mod audit;
mod catalog;
mod error;
mod home;
mod page;
mod read;

use axum::Json;
use axum::extract::{Extension, Path, Query};
use oxy_app_core::audit::RequestActor;
use oxy_auth::types::AppPublishTokenAuth;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::environment_scope::{EnvironmentQuery, ScopeError};
use admission::{Admitted, admit};
use catalog::{Column, Table};
use home::{Absent, Store, store_for};
use page::{Caps, Cell, OrderTerm, PageRequest};

/// Where the data is, or why there is none.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StoreState {
    Ready { schema: String },
    Absent(Absent),
}

#[derive(Debug, Serialize)]
pub struct TablesResponse {
    pub environment: String,
    pub store: StoreState,
    pub tables: Vec<Table>,
    pub caps: Caps,
}

/// `?table=` and the page, beside `?environment=`.
#[derive(Debug, Deserialize)]
pub struct RowsQuery {
    #[serde(default)]
    pub environment: Option<String>,
    pub table: String,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RowsResponse {
    pub environment: String,
    pub schema: String,
    pub table: String,
    pub columns: Vec<Column>,
    /// One array per row, one cell per column, in `columns`' order.
    pub rows: Vec<Vec<Cell>>,
    /// The page size read: the one asked for, or fewer for a wide table.
    pub limit: u32,
    pub requested_limit: u32,
    pub offset: u32,
    /// The order read in: the column asked for, then the primary key.
    pub order: Vec<OrderTerm>,
    pub has_more: bool,
    pub caps: Caps,
}

/// The app's tables in the environment, or why it has none.
pub async fn list_tables(
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    Query(query): Query<EnvironmentQuery>,
) -> Result<Json<TablesResponse>, ScopeError> {
    let raw = query.environment.as_deref();
    let admitted = admit(&actor.user, marker.is_some(), id, raw).await?;
    let Admitted {
        db,
        app,
        environment,
    } = &admitted;
    let (store, tables) = match store_for(db, app, environment).await? {
        Store::Absent(why) => (StoreState::Absent(why), Vec::new()),
        Store::Ready(connection) => {
            let tables = read::tables(&connection).await;
            let entry = audit::entry(
                &actor,
                app,
                environment,
                &connection.schema,
                audit::Read::Tables,
            );
            let tables = audit::settle(db, entry, tables).await?;
            let schema = connection.schema;
            (StoreState::Ready { schema }, tables)
        }
    };
    Ok(Json(TablesResponse {
        environment: environment.name(),
        store,
        tables,
        caps: Caps::current(),
    }))
}

/// One page of one of the app's tables in the environment.
pub async fn list_rows(
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    Query(query): Query<RowsQuery>,
) -> Result<Json<RowsResponse>, ScopeError> {
    let raw = query.environment.as_deref();
    let admitted = admit(&actor.user, marker.is_some(), id, raw).await?;
    let request = PageRequest::parse(
        query.limit,
        query.offset,
        query.order.as_deref(),
        query.dir.as_deref(),
    )?;
    let Admitted {
        db,
        app,
        environment,
    } = &admitted;
    let connection = match store_for(db, app, environment).await? {
        Store::Ready(connection) => connection,
        Store::Absent(why) => return Err(error::no_store(&why.reason)),
    };
    let outcome = read::rows(&connection, &query.table, &request).await;
    let asked = asked_order(&request);
    let (limit, order) = match &outcome {
        Ok(page) => (page.limit, page.order.as_slice()),
        Err(_) => (request.limit, asked.as_slice()),
    };
    let what = audit::Read::Rows {
        table: &query.table,
        limit,
        offset: request.offset,
        order,
    };
    let entry = audit::entry(&actor, app, environment, &connection.schema, what);
    let page = audit::settle(db, entry, outcome).await?;
    Ok(Json(RowsResponse {
        environment: environment.name(),
        schema: connection.schema,
        table: page.table.name,
        columns: page.table.columns,
        rows: page.rows,
        limit: page.limit,
        requested_limit: request.limit,
        offset: request.offset,
        order: page.order,
        has_more: page.has_more,
        caps: Caps::current(),
    }))
}

/// The order a request asked for, for the audit row of a read that failed
/// before its order was checked.
fn asked_order(request: &PageRequest) -> Vec<OrderTerm> {
    request
        .order
        .iter()
        .map(|(column, dir)| OrderTerm {
            column: column.clone(),
            dir: *dir,
        })
        .collect()
}

#[cfg(test)]
#[path = "page_tests.rs"]
mod page_tests;
