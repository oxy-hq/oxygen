//! Analyst visibility: the grants that let the analyst role read a writer's
//! schema, and that withdraw it.
//!
//! Exposing a schema to the analyst needs **two connections**, because two
//! different roles own the objects involved:
//!
//! - the *schema* is owned by the database owner → it grants `USAGE`
//! - the *tables* are owned by the writer that created them → it grants `SELECT`
//!
//! Postgres requires you to own an object (or hold GRANT OPTION) to grant on it,
//! so a single connection cannot issue both. `ALTER DEFAULT PRIVILEGES FOR ROLE
//! <writer>` likewise requires membership in that role, which the database owner
//! does not have on a managed provider.

use super::{SchemaError, WriterRef, quote_ident, validate_name};

/// One `GRANT`/`REVOKE` applied to every table in `schema` the *executing role*
/// owns.
///
/// `GRANT ... ON ALL TABLES IN SCHEMA` is the obvious spelling and the wrong
/// one: it also covers tables owned by someone else, and Postgres answers a
/// grant you hold no grant option for with a **warning rather than an error**.
/// The statement reports success and changes nothing. Filtering on
/// `current_user` keeps the set to exactly what this connection can act on.
fn each_owned_table_sql(verb: &str, schema: &str, preposition: &str, grantee: &str) -> String {
    format!(
        "DO $$ DECLARE r record; BEGIN \
           FOR r IN SELECT tablename FROM pg_tables \
                    WHERE schemaname = '{schema}' AND tableowner = current_user \
           LOOP EXECUTE format('{verb} SELECT ON TABLE %I.%I {preposition} %I', \
                               '{schema}', r.tablename, '{grantee}'); \
           END LOOP; END $$"
    )
}

/// Run **as the database owner**: let the analyst read the tables *migrations*
/// created.
///
/// The companion to [`grant_analyst_tables_sql`], which runs as the writer and
/// so only ever reaches tables the writer itself made. Since migrations run as
/// the owner, that is nearly none of them — without this, opting an app into
/// analytics grants schema visibility over an empty-looking schema.
pub fn grant_analyst_owner_tables_sql(
    writer: &WriterRef,
    owner_role: &str,
    analyst_role: &str,
) -> Result<Vec<String>, SchemaError> {
    validate_name(owner_role)?;
    validate_name(analyst_role)?;
    let schema = quote_ident(&writer.schema_name());
    let owner = quote_ident(owner_role);
    let analyst = quote_ident(analyst_role);
    Ok(vec![
        each_owned_table_sql("GRANT", &writer.schema_name(), "TO", analyst_role),
        format!(
            "ALTER DEFAULT PRIVILEGES FOR ROLE {owner} IN SCHEMA {schema} \
             GRANT SELECT ON TABLES TO {analyst}"
        ),
    ])
}

/// Run **as the database owner**: withdraw what [`grant_analyst_owner_tables_sql`]
/// gave.
///
/// Revoking schema `USAGE` alone would *mask* these grants rather than remove
/// them — access stops, but the `SELECT` survives, so a later opt-in silently
/// restores reads this revoke reported having withdrawn.
pub fn revoke_analyst_owner_tables_sql(
    writer: &WriterRef,
    owner_role: &str,
    analyst_role: &str,
) -> Result<Vec<String>, SchemaError> {
    validate_name(owner_role)?;
    validate_name(analyst_role)?;
    let schema = quote_ident(&writer.schema_name());
    let owner = quote_ident(owner_role);
    let analyst = quote_ident(analyst_role);
    Ok(vec![
        format!(
            "ALTER DEFAULT PRIVILEGES FOR ROLE {owner} IN SCHEMA {schema} \
             REVOKE SELECT ON TABLES FROM {analyst}"
        ),
        each_owned_table_sql("REVOKE", &writer.schema_name(), "FROM", analyst_role),
    ])
}

/// Run **as the database owner**: let the analyst see into the schema.
pub fn grant_analyst_schema_sql(writer: &WriterRef, analyst_role: &str) -> Vec<String> {
    let schema = quote_ident(&writer.schema_name());
    let analyst = quote_ident(analyst_role);
    vec![format!("GRANT USAGE ON SCHEMA {schema} TO {analyst}")]
}

/// Run **as the writer**: let the analyst read its tables, present and future.
///
/// The `ALTER DEFAULT PRIVILEGES` clause is what covers tables created later.
/// Without it, analytics silently stops seeing anything the app adds after
/// this ran — a bug that looks like missing data rather than missing grants.
pub fn grant_analyst_tables_sql(writer: &WriterRef, analyst_role: &str) -> Vec<String> {
    let schema = quote_ident(&writer.schema_name());
    let analyst = quote_ident(analyst_role);
    vec![
        format!("GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO {analyst}"),
        format!("ALTER DEFAULT PRIVILEGES IN SCHEMA {schema} GRANT SELECT ON TABLES TO {analyst}"),
    ]
}

/// Run **as the writer**: withdraw table access.
///
/// Revoking the default privileges first matters — dropping only the table
/// grants would leave future tables silently readable again.
pub fn revoke_analyst_tables_sql(writer: &WriterRef, analyst_role: &str) -> Vec<String> {
    let schema = quote_ident(&writer.schema_name());
    let analyst = quote_ident(analyst_role);
    vec![
        format!(
            "ALTER DEFAULT PRIVILEGES IN SCHEMA {schema} REVOKE SELECT ON TABLES FROM {analyst}"
        ),
        format!("REVOKE SELECT ON ALL TABLES IN SCHEMA {schema} FROM {analyst}"),
    ]
}

/// Run **as the database owner**: withdraw schema visibility.
pub fn revoke_analyst_schema_sql(writer: &WriterRef, analyst_role: &str) -> Vec<String> {
    let schema = quote_ident(&writer.schema_name());
    let analyst = quote_ident(analyst_role);
    vec![format!("REVOKE USAGE ON SCHEMA {schema} FROM {analyst}")]
}
