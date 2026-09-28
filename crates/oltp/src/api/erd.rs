//! Read-only entity-relationship view of a per-org OLTP database.
//!
//! Deliberately **not** a table editor. A Supabase-style grid means a human
//! writing to production rows, which is exactly what
//! [`crate::schema::ANALYST_ROLE`] exists to prevent. This shows structure and
//! nothing else — every query below runs as the read-only analyst.
//!
//! Why this and not the generic `/databases/{name}/schema` endpoint: that one
//! returns bare table names with no namespace and no foreign keys. The whole
//! point of a per-org OLTP database is that each writer owns a schema, so a
//! diagram that cannot say which schema a table belongs to has nothing to show.

use std::collections::HashMap;

use axum::extract::{Json, Query};
use axum::http::StatusCode;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_platform::db::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use tracing::{error, instrument};

use super::handlers::{WorkspaceQuery, resolve_caller_org};
use crate::entity::roles::{self as oltp_roles, Entity as OltpRoles, WriterKind};
use crate::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};
use crate::resolver::resolve_analyst_connection;

#[derive(Debug, Serialize)]
pub struct ErdColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    /// Part of the table's primary key.
    pub is_primary_key: bool,
}

#[derive(Debug, Serialize)]
pub struct ErdTable {
    pub name: String,
    pub columns: Vec<ErdColumn>,
}

#[derive(Debug, Serialize)]
pub struct ErdSchema {
    pub name: String,
    /// `app`, `pipeline`, or `other` when no writer row claims it.
    pub kind: String,
    /// Writer that owns this schema, when known.
    pub writer_name: Option<String>,
    pub tables: Vec<ErdTable>,
}

/// A foreign key, as an edge between two columns.
#[derive(Debug, Serialize)]
pub struct ErdRelationship {
    pub from_schema: String,
    pub from_table: String,
    pub from_column: String,
    pub to_schema: String,
    pub to_table: String,
    pub to_column: String,
}

#[derive(Debug, Serialize)]
pub struct ErdResponse {
    pub database: String,
    pub schemas: Vec<ErdSchema>,
    pub relationships: Vec<ErdRelationship>,
    /// The role these queries ran as. Always the analyst — surfaced so the UI
    /// can state plainly that the diagram is read-only.
    pub read_as_role: String,
}

/// `GET /oltp/me/erd` — structure of the caller's org OLTP database.
///
/// Returns no row data, only shape. 403 for a non-member, 409 when the org has
/// no provisioned database yet.
#[instrument(skip(user, query), fields(user_id = %user.id, workspace_id = %query.workspace_id))]
pub async fn get_erd(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Json<ErdResponse>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let org_id = resolve_caller_org(&db, query.workspace_id, user.id).await?;

    let conn = resolve_analyst_connection(&db, query.workspace_id)
        .await
        .map_err(|e| {
            error!("could not resolve OLTP analyst connection: {e}");
            // Shared mapping, so a status here matches every other route that
            // resolves a connection. The handler surfaces the code; the detail
            // is logged.
            super::resolve_status(e).0
        })?;

    // Writer metadata comes from Oxy's control plane, so a schema can be
    // labelled `app` / `pipeline` without asking the tenant database who owns
    // what — it has no idea.
    let tenant = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .one(&db)
        .await
        .map_err(|e| {
            error!("query oltp tenant: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::CONFLICT)?;
    let roles = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
        .all(&db)
        .await
        .map_err(|e| {
            error!("query oltp roles: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let owners: HashMap<String, (String, String)> = roles
        .into_iter()
        .map(|r| {
            let kind = match r.writer_kind {
                WriterKind::App => "app",
                WriterKind::Pipeline => "pipeline",
            };
            (r.schema_name, (kind.to_string(), r.writer_name))
        })
        .collect();

    let (schemas, relationships) = introspect(&conn.dsn(), &owners).await.map_err(|e| {
        error!("OLTP introspection failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    Ok(Json(ErdResponse {
        database: conn.database.clone(),
        schemas,
        relationships,
        read_as_role: conn.user.clone(),
    }))
}

/// Two queries as the analyst: columns, then foreign keys.
///
/// No privilege filtering is needed in the SQL — `information_schema` only
/// exposes objects the connected role may see, so `oxy_meta` and any schema the
/// analyst was never granted simply do not appear.
async fn introspect(
    dsn: &str,
    owners: &HashMap<String, (String, String)>,
) -> Result<(Vec<ErdSchema>, Vec<ErdRelationship>), String> {
    // TLS comes from `crate::connect`, the single place the rustls connector is
    // built for **tenant** DSNs — introspection, the resolver, the provisioner's
    // DDL. Whether TLS is demanded follows from the DSN's sslmode
    // (`sslmode_for`), so a local container and a managed provider both work
    // through this one path. `LocalProvider`'s two *admin* connections still use
    // `NoTls` deliberately: that provider exists to drive a local cluster, and a
    // managed provider is reached through its REST API rather than a superuser
    // session.
    let client = crate::connect::connect(dsn, "OLTP introspection")
        .await
        .map_err(|e| format!("connect: {e}"))?;

    // Columns come from information_schema **on purpose**: it exposes only what
    // the connected role may SELECT, so the analyst's grants keep `oxy_meta`
    // and any un-granted schema out of the diagram for free.
    const COLUMNS_SQL: &str = "
        SELECT c.table_schema, c.table_name, c.column_name, c.data_type,
               c.is_nullable = 'YES' AS nullable
        FROM information_schema.columns c
        WHERE c.table_schema NOT IN ('pg_catalog', 'information_schema')
        ORDER BY c.table_schema, c.table_name, c.ordinal_position";

    // Constraints must come from pg_catalog, NOT information_schema.
    // `information_schema.table_constraints` only lists constraints on tables
    // the role owns or holds a privilege *other than SELECT* on — the analyst
    // holds SELECT alone, so it sees none, and the diagram silently loses every
    // key and every edge.
    //
    // pg_catalog is readable by PUBLIC, which is the opposite problem: it would
    // reveal constraints in schemas the analyst cannot read. Both result sets
    // are therefore intersected with the columns query above, which is
    // privilege-filtered. The visible set stays exactly what SELECT allows.
    const PK_SQL: &str = "
        SELECT ns.nspname AS table_schema, cl.relname AS table_name,
               att.attname AS column_name
        FROM pg_constraint con
        JOIN pg_class cl ON cl.oid = con.conrelid
        JOIN pg_namespace ns ON ns.oid = cl.relnamespace
        JOIN unnest(con.conkey) AS k(attnum) ON true
        JOIN pg_attribute att ON att.attrelid = cl.oid AND att.attnum = k.attnum
        WHERE con.contype = 'p'
          AND ns.nspname NOT IN ('pg_catalog', 'information_schema')";

    // `WITH ORDINALITY` pairs each local column with the referenced column at
    // the same position, so composite foreign keys produce correct pairs rather
    // than a cross product.
    const FK_SQL: &str = "
        SELECT ns.nspname  AS from_schema, cl.relname  AS from_table,
               att.attname AS from_column,
               fns.nspname AS to_schema,   fcl.relname AS to_table,
               fatt.attname AS to_column
        FROM pg_constraint con
        JOIN pg_class cl ON cl.oid = con.conrelid
        JOIN pg_namespace ns ON ns.oid = cl.relnamespace
        JOIN unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) ON true
        JOIN pg_attribute att ON att.attrelid = cl.oid AND att.attnum = k.attnum
        JOIN pg_class fcl ON fcl.oid = con.confrelid
        JOIN pg_namespace fns ON fns.oid = fcl.relnamespace
        JOIN unnest(con.confkey) WITH ORDINALITY AS fk(attnum, ord) ON fk.ord = k.ord
        JOIN pg_attribute fatt ON fatt.attrelid = fcl.oid AND fatt.attnum = fk.attnum
        WHERE con.contype = 'f'
          AND ns.nspname NOT IN ('pg_catalog', 'information_schema')";

    let col_rows = client
        .query(COLUMNS_SQL, &[])
        .await
        .map_err(|e| format!("columns query: {e}"))?;

    let pk_rows = client
        .query(PK_SQL, &[])
        .await
        .map_err(|e| format!("primary key query: {e}"))?;
    let primary_keys: std::collections::HashSet<(String, String, String)> = pk_rows
        .into_iter()
        .map(|r| {
            (
                r.get("table_schema"),
                r.get("table_name"),
                r.get("column_name"),
            )
        })
        .collect();

    // Preserve query order so tables stay in ordinal_position order.
    let mut by_schema: Vec<(String, Vec<ErdTable>)> = Vec::new();
    for row in col_rows {
        let schema: String = row.get("table_schema");
        let table: String = row.get("table_name");
        let name: String = row.get("column_name");
        let column = ErdColumn {
            is_primary_key: primary_keys.contains(&(schema.clone(), table.clone(), name.clone())),
            name,
            data_type: row.get("data_type"),
            nullable: row.get("nullable"),
        };

        let tables = match by_schema.iter_mut().find(|(s, _)| *s == schema) {
            Some((_, t)) => t,
            None => {
                by_schema.push((schema.clone(), Vec::new()));
                &mut by_schema.last_mut().expect("just pushed").1
            }
        };
        match tables.iter_mut().find(|t| t.name == table) {
            Some(t) => t.columns.push(column),
            None => tables.push(ErdTable {
                name: table,
                columns: vec![column],
            }),
        }
    }

    // Everything the analyst may actually SELECT, per the columns query. Used
    // to clip the pg_catalog constraint results back down to that set.
    let visible: std::collections::HashSet<(String, String)> = by_schema
        .iter()
        .flat_map(|(s, tables)| tables.iter().map(move |t| (s.clone(), t.name.clone())))
        .collect();

    let schemas = by_schema
        .into_iter()
        .map(|(name, tables)| {
            let (kind, writer_name) = match owners.get(&name) {
                Some((k, w)) => (k.clone(), Some(w.clone())),
                None => ("other".to_string(), None),
            };
            ErdSchema {
                name,
                kind,
                writer_name,
                tables,
            }
        })
        .collect();

    let fk_rows = client
        .query(FK_SQL, &[])
        .await
        .map_err(|e| format!("foreign key query: {e}"))?;
    let relationships = fk_rows
        .into_iter()
        .map(|row| ErdRelationship {
            from_schema: row.get("from_schema"),
            from_table: row.get("from_table"),
            from_column: row.get("from_column"),
            to_schema: row.get("to_schema"),
            to_table: row.get("to_table"),
            to_column: row.get("to_column"),
        })
        // Both ends must be readable. pg_catalog would otherwise leak the
        // existence of tables the analyst has no grant on.
        .filter(|r| {
            visible.contains(&(r.from_schema.clone(), r.from_table.clone()))
                && visible.contains(&(r.to_schema.clone(), r.to_table.clone()))
        })
        .collect();

    Ok((schemas, relationships))
}
