//! What a role may reach in its tenant's database, read from Postgres.
//!
//! Schema-per-writer is a boundary because each writer's role holds grants on
//! its own schema and nothing else ([`crate::schema`]). That containment lives
//! entirely in Postgres grants, so nothing above them would notice one that
//! drifted: a hand-run `GRANT SELECT ON app_bookings.patients TO
//! app_marketing_rw`, a table a migration created with the wrong owner, or an
//! analyst left holding `INSERT`. Writers' identities are derived from the
//! app's own slug, so `oxy-authz` has no decision to make here; a mis-granted
//! role is the remaining way one app reaches another's rows. This module is
//! the check for that.
//!
//! **One query, two readers.** [`role_findings_sql`] lists every privilege a
//! role holds outside the schemas it may use. Provisioning wraps it in
//! [`assert_role_grants_sql`] and refuses a writer that holds any (`OXY03`),
//! as [`crate::roles::assert_confined_sql`] refuses an over-privileged role;
//! `oxy oltp audit` runs it for every writer and the analyst, and
//! [`public_findings_sql`] for `PUBLIC`.
//!
//! **Effective privileges, not ACL entries.** It asks `has_*_privilege`, so a
//! privilege reached through `PUBLIC`, ownership or a grant option counts.
//! In the audit, what `PUBLIC` holds is reported once under `PUBLIC` and left
//! out of each role's findings, so one bad `GRANT … TO PUBLIC` is one finding
//! rather than one per role. Provisioning also refuses on what `PUBLIC` holds
//! in writer and Oxy schemas, since every writer holds that too.
//!
//! **What a role may use.** A writer: its own schema. A pipeline writer also
//! holds `CREATE ON DATABASE` by design (Airway creates its dataset schema on
//! every load; see [`crate::roles::grant_schema_creation_sql`]) and so may own
//! schemas it created, except ones named like another writer's (`app_*`,
//! `raw_*`), which is the namespace squat
//! [`crate::roles::assert_schema_owned_sql`] exists for. The analyst: `SELECT`
//! on the schemas exposed to analytics, and no write or `CREATE` anywhere.
//! System schemas (`pg_*`, `information_schema`) are out of scope.

use crate::roles::quote_literal;
use crate::schema::{SchemaError, validate_name};

/// SQLSTATE raised by [`assert_role_grants_sql`], beside `OXY01` (not
/// confined) and `OXY02` (schema owned by another role).
pub const GRANT_DRIFT_SQLSTATE: &str = "OXY03";

/// What one role is allowed to use, and therefore what is a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantScope {
    /// The role's real (qualified) name.
    pub role: String,
    /// Schemas the role may use: a writer's own, or the analyst's exposed set.
    pub schemas: Vec<String>,
    /// Holds no write or `CREATE` privilege anywhere, its own schemas included:
    /// the analyst, or a read-only writer.
    pub read_only: bool,
    /// May hold `CREATE ON DATABASE` and own schemas it created (a pipeline
    /// writer).
    pub may_create_schemas: bool,
}

impl GrantScope {
    /// An app writer: its own schema, read-write, nothing else.
    pub fn app_writer(role: &str, schema: &str) -> Self {
        Self {
            role: role.to_string(),
            schemas: vec![schema.to_string()],
            read_only: false,
            may_create_schemas: false,
        }
    }

    /// A pipeline writer: its own schema, plus schemas it created that do not
    /// squat another writer's name.
    pub fn pipeline_writer(role: &str, schema: &str) -> Self {
        Self {
            may_create_schemas: true,
            ..Self::app_writer(role, schema)
        }
    }

    /// The scope of `writer`'s role `role` at `grant`: what provisioning has
    /// just granted it, and nothing more.
    pub fn for_writer(
        writer: &crate::schema::WriterRef,
        grant: crate::schema::GrantLevel,
        role: &str,
    ) -> Self {
        let schema = writer.schema_name();
        let scope = match writer {
            crate::schema::WriterRef::App(_) => Self::app_writer(role, &schema),
            crate::schema::WriterRef::Pipeline(_) => Self::pipeline_writer(role, &schema),
        };
        Self {
            read_only: grant == crate::schema::GrantLevel::ReadOnly,
            ..scope
        }
    }

    /// The analyst: read-only over `exposed` schemas.
    pub fn analyst(role: &str, exposed: Vec<String>) -> Self {
        Self {
            role: role.to_string(),
            schemas: exposed,
            read_only: true,
            may_create_schemas: false,
        }
    }

    fn validate(&self) -> Result<(), SchemaError> {
        validate_name(&self.role)?;
        self.schemas.iter().try_for_each(|s| validate_name(s))
    }
}

/// Table-like relation kinds, and the privileges checked on each.
const TABLE_KINDS: &str = "'r','p','v','m','f'";
const TABLE_PRIVILEGES: &str =
    "ARRAY['SELECT','INSERT','UPDATE','DELETE','TRUNCATE','REFERENCES','TRIGGER']";
const SEQUENCE_PRIVILEGES: &str = "ARRAY['USAGE','SELECT','UPDATE']";

/// The schemas in scope: everything but Postgres's own.
const USER_SCHEMAS: &str = "SELECT n.oid, n.nspname, n.nspowner FROM pg_namespace n \
     WHERE n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema'";

/// A `SELECT` of one text column, one row per privilege `scope.role` holds
/// outside what it may use, each a phrase such as `SELECT on
/// app_bookings.patients`. No rows means the role is clean. A role that does
/// not exist yields no rows.
pub fn role_findings_sql(scope: &GrantScope) -> Result<String, SchemaError> {
    scope.validate()?;
    let role = quote_literal(&scope.role);
    let allowed = text_array(&scope.schemas);
    let read_only = scope.read_only;
    let may_create = scope.may_create_schemas;
    Ok(format!(
        "WITH me AS (SELECT oid FROM pg_roles WHERE rolname = {role}), \
         user_schema AS ({USER_SCHEMAS}), \
         mine AS ( \
           SELECT s.oid FROM user_schema s WHERE s.nspname = ANY({allowed}) \
           UNION SELECT s.oid FROM user_schema s, me \
             WHERE {may_create} AND s.nspowner = me.oid \
               AND s.nspname NOT LIKE 'app\\_%' AND s.nspname NOT LIKE 'raw\\_%' \
         ) \
         SELECT f FROM ( \
           SELECT 'USAGE on schema ' || quote_ident(s.nspname) AS f FROM user_schema s, me \
             WHERE s.oid NOT IN (SELECT oid FROM mine) \
               AND has_schema_privilege(me.oid, s.oid, 'USAGE') \
               AND NOT has_schema_privilege('public', s.oid, 'USAGE') \
           UNION ALL SELECT 'CREATE on schema ' || quote_ident(s.nspname) FROM user_schema s, me \
             WHERE (s.oid NOT IN (SELECT oid FROM mine) OR {read_only}) \
               AND has_schema_privilege(me.oid, s.oid, 'CREATE') \
               AND NOT has_schema_privilege('public', s.oid, 'CREATE') \
           UNION ALL SELECT 'owns schema ' || quote_ident(s.nspname) FROM user_schema s, me \
             WHERE s.nspowner = me.oid AND s.oid NOT IN (SELECT oid FROM mine) \
           UNION ALL SELECT p.priv || ' on ' || quote_ident(s.nspname) || '.' || quote_ident(c.relname) \
             FROM pg_class c JOIN user_schema s ON s.oid = c.relnamespace \
             CROSS JOIN unnest({TABLE_PRIVILEGES}) AS p(priv) CROSS JOIN me \
             WHERE c.relkind IN ({TABLE_KINDS}) \
               AND (s.oid NOT IN (SELECT oid FROM mine) OR ({read_only} AND p.priv <> 'SELECT')) \
               AND has_table_privilege(me.oid, c.oid, p.priv) \
               AND NOT has_table_privilege('public', c.oid, p.priv) \
           UNION ALL SELECT p.priv || ' on sequence ' || quote_ident(s.nspname) || '.' \
               || quote_ident(c.relname) \
             FROM pg_class c JOIN user_schema s ON s.oid = c.relnamespace \
             CROSS JOIN unnest({SEQUENCE_PRIVILEGES}) AS p(priv) CROSS JOIN me \
             WHERE c.relkind = 'S' \
               AND (s.oid NOT IN (SELECT oid FROM mine) OR ({read_only} AND p.priv = 'UPDATE')) \
               AND has_sequence_privilege(me.oid, c.oid, p.priv) \
               AND NOT has_sequence_privilege('public', c.oid, p.priv) \
           UNION ALL SELECT 'CREATE on database ' || quote_ident(current_database()) FROM me \
             WHERE NOT {may_create} \
               AND has_database_privilege(me.oid, current_database(), 'CREATE') \
               AND NOT has_database_privilege('public', current_database(), 'CREATE') \
         ) findings ORDER BY f"
    ))
}

/// Raise `OXY03` naming every privilege `scope.role` holds outside what it may
/// use, and every privilege `PUBLIC` holds in writer or Oxy schemas (which
/// this role holds too, through `PUBLIC`). Run as the owner once a writer is
/// granted and its credential sealed, so provisioning stops rather than hands
/// out a credential whose reach has drifted.
pub fn assert_role_grants_sql(scope: &GrantScope) -> Result<String, SchemaError> {
    let own = role_findings_sql(scope)?;
    let public = public_reach_sql();
    let role = quote_literal(&scope.role);
    Ok(format!(
        "DO $$ DECLARE bad text; BEGIN \
           SELECT string_agg(f, '; ') INTO bad FROM ( \
             SELECT f FROM ({own}) r \
             UNION ALL SELECT 'PUBLIC holds ' || f FROM ({public}) p \
           ) g; \
           IF bad IS NOT NULL THEN \
             RAISE EXCEPTION 'role % holds privileges outside its own schema: %', {role}, bad \
               USING ERRCODE = '{GRANT_DRIFT_SQLSTATE}', \
                     HINT = 'a grant was made by hand or a migration left an object with the \
                             wrong owner; revoke it (oxy oltp audit lists every finding) and \
                             provision again'; \
           END IF; \
         END $$"
    ))
}

/// One row per privilege `PUBLIC` holds that it should not: `CREATE` on any
/// schema or on the database, `USAGE` on any schema but `public`, and any
/// privilege on a table or sequence outside `public`. Anything here is held by
/// every role in the database, every writer included.
pub fn public_findings_sql() -> String {
    public_sql("TRUE", true)
}

/// The part of [`public_findings_sql`] that reaches a writer's data or Oxy's
/// own: privileges `PUBLIC` holds in an `app_*` or `raw_*` schema, or in
/// `oxy_meta` / `oxy_inbox`. Provisioning refuses on these. The rest (a
/// provider's own schema left readable, say) is for `oxy oltp audit` to report
/// rather than a reason to stop every provision on that provider.
fn public_reach_sql() -> String {
    let filter = format!(
        "(s.nspname LIKE 'app\\_%' OR s.nspname LIKE 'raw\\_%' OR s.nspname IN ({}, {}))",
        quote_literal(crate::platform::META_SCHEMA),
        quote_literal(crate::platform::INBOX_SCHEMA),
    );
    public_sql(&filter, false)
}

/// `PUBLIC`'s findings over schemas matching `filter` (a condition on `s`),
/// with the database-level check when `database` is set.
fn public_sql(filter: &str, database: bool) -> String {
    let database_check = if database {
        " UNION ALL SELECT 'CREATE on database ' || quote_ident(current_database()) \
           WHERE has_database_privilege('public', current_database(), 'CREATE')"
    } else {
        ""
    };
    format!(
        "WITH user_schema AS (SELECT n.oid, n.nspname FROM pg_namespace n \
           WHERE n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema') \
         SELECT f FROM ( \
           SELECT 'CREATE on schema ' || quote_ident(s.nspname) AS f FROM user_schema s \
             WHERE {filter} AND has_schema_privilege('public', s.oid, 'CREATE') \
           UNION ALL SELECT 'USAGE on schema ' || quote_ident(s.nspname) FROM user_schema s \
             WHERE {filter} AND s.nspname <> 'public' \
               AND has_schema_privilege('public', s.oid, 'USAGE') \
           UNION ALL SELECT p.priv || ' on ' || quote_ident(s.nspname) || '.' || quote_ident(c.relname) \
             FROM pg_class c JOIN user_schema s ON s.oid = c.relnamespace \
             CROSS JOIN unnest({TABLE_PRIVILEGES}) AS p(priv) \
             WHERE c.relkind IN ({TABLE_KINDS}) AND {filter} AND s.nspname <> 'public' \
               AND has_table_privilege('public', c.oid, p.priv) \
           UNION ALL SELECT p.priv || ' on sequence ' || quote_ident(s.nspname) || '.' \
               || quote_ident(c.relname) \
             FROM pg_class c JOIN user_schema s ON s.oid = c.relnamespace \
             CROSS JOIN unnest({SEQUENCE_PRIVILEGES}) AS p(priv) \
             WHERE c.relkind = 'S' AND {filter} AND s.nspname <> 'public' \
               AND has_sequence_privilege('public', c.oid, p.priv) \
           {database_check} \
         ) findings ORDER BY f"
    )
}

/// One role's findings, for `oxy oltp audit`. `PUBLIC` is reported as the role
/// `PUBLIC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleFindings {
    pub role: String,
    pub findings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GrantAuditError {
    #[error("reading the tenant's roles: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("a recorded role or schema name is not valid: {0}")]
    Schema(#[from] SchemaError),
    #[error("running the audit query: {0}")]
    Query(String),
}

/// Every recorded writer of `tenant`, its analyst and `PUBLIC`, each with what
/// it holds outside its scope. `client` is the owner's connection to the
/// tenant's database; roles come from `oltp_roles`, the analyst's exposed set
/// from each writer's analytics visibility.
pub async fn tenant_findings(
    db: &sea_orm::DatabaseConnection,
    client: &tokio_postgres::Client,
    tenant: &crate::entity::tenants::Model,
) -> Result<Vec<RoleFindings>, GrantAuditError> {
    use crate::entity::roles::{self as oltp_roles, Entity as OltpRoles, WriterKind};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

    let writers = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
        .order_by_asc(oltp_roles::Column::RoleName)
        .all(db)
        .await?;
    let mut scopes: Vec<GrantScope> = writers
        .iter()
        .map(|w| GrantScope {
            read_only: w.grant_level == oltp_roles::GrantLevel::ReadOnly,
            ..match w.writer_kind {
                WriterKind::App => GrantScope::app_writer(&w.role_name, &w.schema_name),
                WriterKind::Pipeline => GrantScope::pipeline_writer(&w.role_name, &w.schema_name),
            }
        })
        .collect();
    let exposed = writers
        .iter()
        .filter(|w| crate::migrator::effective_visibility(w.analytics_visible, &w.writer_kind))
        .map(|w| w.schema_name.clone())
        .collect();
    let analyst = crate::schema::analyst_role_for(&tenant.provider, &tenant.database_name);
    scopes.push(GrantScope::analyst(&analyst, exposed));

    let mut report = Vec::with_capacity(scopes.len() + 1);
    for scope in &scopes {
        let findings = query_findings(client, &role_findings_sql(scope)?).await?;
        report.push(RoleFindings {
            role: scope.role.clone(),
            findings,
        });
    }
    report.push(RoleFindings {
        role: "PUBLIC".to_string(),
        findings: query_findings(client, &public_findings_sql()).await?,
    });
    Ok(report)
}

async fn query_findings(
    client: &tokio_postgres::Client,
    sql: &str,
) -> Result<Vec<String>, GrantAuditError> {
    let rows = client
        .query(sql, &[])
        .await
        .map_err(|e| GrantAuditError::Query(crate::connect::pg_detail(&e)))?;
    Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
}

/// `ARRAY['a','b']::text[]`, or an empty `text[]`. Every element has passed
/// [`validate_name`], so quoting is defence in depth.
fn text_array(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| quote_literal(s)).collect();
    format!("ARRAY[{}]::text[]", quoted.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_say_what_each_role_may_use() {
        let app = GrantScope::app_writer("app_bookings_rw", "app_bookings");
        assert_eq!(app.schemas, ["app_bookings"]);
        assert!(!app.read_only && !app.may_create_schemas);
        let pipe = GrantScope::pipeline_writer("raw_toast_rw", "raw_toast");
        assert!(pipe.may_create_schemas && !pipe.read_only);
        let analyst = GrantScope::analyst("oxy_analyst_ro", vec!["raw_toast".into()]);
        assert!(analyst.read_only && !analyst.may_create_schemas);
    }

    #[test]
    fn a_writers_scope_follows_its_kind_and_grant() {
        use crate::schema::{GrantLevel, WriterRef};
        let app = WriterRef::app("bookings").unwrap();
        let s = GrantScope::for_writer(&app, GrantLevel::ReadWrite, "app_bookings_rw");
        assert_eq!(s, GrantScope::app_writer("app_bookings_rw", "app_bookings"));
        let pipe = WriterRef::pipeline("toast").unwrap();
        let s = GrantScope::for_writer(&pipe, GrantLevel::ReadOnly, "raw_toast_ro");
        assert!(s.may_create_schemas && s.read_only);
        assert_eq!(s.schemas, ["raw_toast"]);
    }

    #[test]
    fn the_query_names_the_role_and_its_schemas_as_literals() {
        let sql =
            role_findings_sql(&GrantScope::app_writer("app_bookings_rw", "app_bookings")).unwrap();
        assert!(sql.contains("rolname = 'app_bookings_rw'"), "{sql}");
        assert!(sql.contains("ANY(ARRAY['app_bookings']::text[])"), "{sql}");
        let empty = role_findings_sql(&GrantScope::analyst("oxy_analyst_ro", vec![])).unwrap();
        assert!(empty.contains("ANY(ARRAY[]::text[])"), "{empty}");
    }

    #[test]
    fn names_are_validated_before_any_sql() {
        assert!(role_findings_sql(&GrantScope::app_writer("x'; DROP", "app_x")).is_err());
        assert!(role_findings_sql(&GrantScope::app_writer("app_x_rw", "a b")).is_err());
    }

    #[test]
    fn the_assertion_raises_its_own_sqlstate() {
        let sql =
            assert_role_grants_sql(&GrantScope::app_writer("app_bookings_rw", "app_bookings"))
                .unwrap();
        assert!(sql.starts_with("DO $$"), "{sql}");
        assert!(sql.contains("ERRCODE = 'OXY03'"), "{sql}");
        assert!(
            !sql[5..sql.len() - 3].contains("$$"),
            "body must not close the quote"
        );
    }
}
