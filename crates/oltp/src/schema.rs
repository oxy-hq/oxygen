//! Schema + role model for a per-org OLTP database.
//!
//! Oxy owns the database and every schema in it. Each **writer** — an Airway
//! pipeline or a custom app — gets exactly one schema and one role with grants
//! on that schema only.
//!
//! ```text
//! <org database>                          all schemas owned by the DB owner
//!   ├── public              (locked down; nobody may CREATE here)
//!   ├── raw_toast           CREATE granted to raw_toast_rw       ← Airway pipeline
//!   ├── raw_quickbooks      CREATE granted to raw_quickbooks_rw  ← Airway pipeline
//!   └── app_bookings        CREATE granted to app_bookings_rw    ← custom app
//! ```
//!
//! # Invariants
//!
//! - **No writer is superuser**, and no writer holds `CREATE` on `public`.
//! - A read-write writer holds `CREATE` on its schema, so it has full DDL
//!   *inside* that schema and none outside it. The schema itself stays owned by
//!   the database owner, so a compromised writer cannot drop it.
//! - **Tables normally belong to the database owner, not the writer**, because
//!   migrations run as the owner. A writer can still create its own, so grants
//!   are issued per-connection over `tableowner = current_user` rather than via
//!   `ON ALL TABLES`: you may only grant on what you own, and Postgres refuses
//!   that with a warning rather than an error.
//! - Identifiers are **validated and rejected**, never escaped into safety. A
//!   name that doesn't match [`IDENT_RE_DESCRIPTION`] is an error, not something
//!   to quote around. Quoting is still applied on top, as defence in depth.
//!
//! Role *creation* is not here: providers like Neon create roles through their
//! REST API (which is what returns the password). This module only generates the
//! SQL that grants an already-created role its access.

use std::fmt;

/// Postgres identifiers are capped at 63 bytes. Role names are the longest
/// derived form (`<prefix>_<name>_rw`), so the raw name is bounded well below.
const MAX_IDENT_LEN: usize = 63;
const APP_PREFIX: &str = "app_";
const RAW_PREFIX: &str = "raw_";
const RW_SUFFIX: &str = "_rw";
const RO_SUFFIX: &str = "_ro";

/// Longest prefix + longest suffix that can wrap a caller-supplied name.
pub const MAX_NAME_LEN: usize = MAX_IDENT_LEN - APP_PREFIX.len() - RW_SUFFIX.len();

/// The single read-only role every human and agent query resolves to.
///
/// **No human ever gets a writable connection to a per-org OLTP database.**
/// `airhouse_managed` escalates its warehouse role from the caller's
/// `effective_role`; this must not. That database holds a customer's live
/// business records, and an ad-hoc `UPDATE` typed into the SQL IDE is a
/// catastrophe rather than a feature. Writes come only from published code —
/// an app function or an Airway pipeline.
pub const ANALYST_ROLE: &str = "oxy_analyst_ro";

/// Whether a provider puts every tenant on ONE Postgres cluster.
///
/// Roles are cluster-global while `oxy_analyst_ro` and `app_<slug>_rw` are
/// fixed names, so on a shared cluster two tenants would resolve to one role —
/// one password, last write wins, and the second tenant's writer authenticates
/// against the first tenant's database. That is the isolation boundary this
/// crate exists to enforce, so on such a provider every role name is qualified.
///
/// Keyed off the provider *string* on the tenant row rather than the provider
/// object, so [`crate::resolver`] can derive the same name without one — the
/// same reason [`crate::provisioner::sslmode_for`] takes a `&str`.
pub fn shares_role_namespace(provider: &str) -> bool {
    provider == "local"
}

/// Short, stable, identifier-safe tag for a tenant.
///
/// FNV-1a rather than a real hash: this only has to separate tenants on one
/// developer's cluster, and it avoids a dependency for eight characters.
///
/// Also the short hash suffix on a `LocalProvider` branch database
/// ([`crate::provider::branch_database_name`]), fed the database and the
/// branch name together.
pub(crate) fn tenant_tag(database: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in database.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{:08x}", (h & 0xffff_ffff) as u32)
}

/// The analyst role's real name for this tenant.
///
/// [`ANALYST_ROLE`] verbatim on a provider that gives each tenant its own
/// cluster (Neon), qualified on one that does not.
pub fn analyst_role_for(provider: &str, database: &str) -> String {
    qualify_role(provider, database, ANALYST_ROLE)
}

/// The stored name for any role on this tenant. See [`shares_role_namespace`].
pub fn qualify_role(provider: &str, database: &str, role: &str) -> String {
    if shares_role_namespace(provider) {
        // 63-byte identifier cap: the base names here are well under 50, and
        // the tag is a fixed 8, so this cannot overflow in practice.
        format!("{role}_{}", tenant_tag(database))
    } else {
        role.to_string()
    }
}

pub const IDENT_RE_DESCRIPTION: &str =
    "lowercase letter followed by lowercase alphanumerics or underscores";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SchemaError {
    #[error(
        "invalid writer name {0:?}: must be 1-{max} chars, {desc}",
        max = MAX_NAME_LEN,
        desc = IDENT_RE_DESCRIPTION
    )]
    InvalidName(String),
}

/// What a writer is allowed to do in its schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantLevel {
    /// Owns the schema: full DDL and DML inside it.
    ReadWrite,
    /// `USAGE` + `SELECT`, including on tables created later.
    ReadOnly,
}

impl GrantLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            GrantLevel::ReadWrite => "rw",
            GrantLevel::ReadOnly => "ro",
        }
    }

    fn role_suffix(&self) -> &'static str {
        match self {
            GrantLevel::ReadWrite => RW_SUFFIX,
            GrantLevel::ReadOnly => RO_SUFFIX,
        }
    }
}

/// Who is writing. Determines the schema namespace, so that an app named
/// `toast` and an Airway source named `toast` can coexist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriterRef {
    /// A custom app, by slug → `app_<slug>`.
    App(String),
    /// An Airway pipeline, by source name → `raw_<source>`.
    Pipeline(String),
}

impl WriterRef {
    pub fn app(slug: impl Into<String>) -> Result<Self, SchemaError> {
        let slug = slug.into();
        validate_name(&slug)?;
        Ok(WriterRef::App(slug))
    }

    pub fn pipeline(source: impl Into<String>) -> Result<Self, SchemaError> {
        let source = source.into();
        validate_name(&source)?;
        Ok(WriterRef::Pipeline(source))
    }

    /// Schema this writer owns. Already validated by construction.
    pub fn schema_name(&self) -> String {
        match self {
            WriterRef::App(slug) => format!("{APP_PREFIX}{slug}"),
            WriterRef::Pipeline(source) => format!("{RAW_PREFIX}{source}"),
        }
    }

    /// Role name for a given access level. Deterministic, so an orphan role
    /// from a failed provision can be found and reconciled rather than
    /// duplicated — the lesson `airhouse` learned the expensive way.
    pub fn role_name(&self, grant: GrantLevel) -> String {
        format!("{}{}", self.schema_name(), grant.role_suffix())
    }

    /// Whether analysts see this writer's schema without an explicit opt-in.
    ///
    /// A pipeline's `raw_*` schema holds ETL'd data that exists *in order to*
    /// be analysed, so it is visible by default. An app's `app_*` schema holds
    /// live application state — bookings, patient records — so it stays hidden
    /// until the app asks to be analysed. Defaulting that one the other way
    /// would make every org member with SQL-IDE access a reader of every app's
    /// production data, which is not a decision to make implicitly.
    pub fn analytics_visible_by_default(&self) -> bool {
        match self {
            WriterRef::Pipeline(_) => true,
            WriterRef::App(_) => false,
        }
    }
}

/// The bare OLTP writer name an app owns, DERIVED from its own slug — the
/// binding that stops one app naming another app's schema. `ctx.oltp` resolves
/// this and only this; an app's manifest merely gates whether `ctx.oltp` is on,
/// it never names the target.
///
/// App slugs allow hyphens; writer/schema identifiers do not
/// ([`validate_name`]), so `-` becomes `_` — `oltp-bookings` → `oltp_bookings`
/// → schema `app_oltp_bookings`. Returns `None` if the slug cannot normalise to
/// a valid identifier (fail closed — `ctx.oltp` then reads as unavailable rather
/// than resolving something unintended). Provisioning must use the same name:
/// `oxy oltp provision --writer app:<this>`.
///
/// **`-` → `_` is injective ONLY over a hyphen-free-of-underscores slug**, which
/// is what `is_valid_slug` (lowercase alphanumerics + hyphens) guarantees. A
/// slug that ALREADY contains `_` is outside that set — only the unvalidated
/// `/publish` multipart field can produce one — and `my_app` would then alias
/// the hyphenated sibling `my-app` onto ONE schema, role and sealed password:
/// the cross-app reach this derivation exists to prevent, needing no rename or
/// delete, just two apps existing at once. So refuse an underscore here rather
/// than depend on a validation rule living in another crate.
pub fn app_writer_name(app_slug: &str) -> Option<String> {
    if app_slug.contains('_') {
        return None;
    }
    let name = app_slug.replace('-', "_");
    validate_name(&name).ok().map(|()| name)
}

impl fmt::Display for WriterRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriterRef::App(slug) => write!(f, "app:{slug}"),
            WriterRef::Pipeline(source) => write!(f, "pipeline:{source}"),
        }
    }
}

/// Reject anything that isn't a plain lowercase Postgres identifier.
///
/// Rejecting rather than escaping is deliberate: these names reach `CREATE
/// SCHEMA` and `GRANT`, where Postgres has no parameter binding. The only safe
/// input is one that needs no escaping.
pub(crate) fn validate_name(name: &str) -> Result<(), SchemaError> {
    let invalid = || SchemaError::InvalidName(name.to_string());

    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(invalid());
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return Err(invalid()),
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(invalid());
    }
    Ok(())
}

/// Double-quote an identifier. Defence in depth only — every identifier
/// reaching here has already passed [`validate_name`], so the escape should
/// never fire.
pub(crate) fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Idempotent SQL to give `writer` its schema at `grant` level.
///
/// Ordering matters: the schema must exist before it can be granted, and
/// `ALTER DEFAULT PRIVILEGES` must follow the table grants so that both
/// existing and future tables are covered.
pub fn ensure_writer_sql(
    writer: &WriterRef,
    grant: GrantLevel,
    owner_role: &str,
    // The role's REAL name, which on a shared-cluster provider is qualified —
    // deriving it from `writer` here would silently target another tenant's
    // role. See `qualify_role`.
    role_name: &str,
) -> Result<Vec<String>, SchemaError> {
    validate_name(owner_role)?;
    validate_name(role_name)?;

    let schema_raw = writer.schema_name();
    let role_raw = role_name.to_string();
    let _ = grant;
    let schema = quote_ident(&schema_raw);
    let role = quote_ident(&role_raw);
    let owner = quote_ident(owner_role);

    // Created as the owner role first, then handed over — a schema can't be
    // created directly AUTHORIZATION a role the creator can't SET ROLE to on
    // every provider.
    let mut sql = vec![
        format!("CREATE SCHEMA IF NOT EXISTS {schema} AUTHORIZATION {owner}"),
        // …and then check that it really is ours. `IF NOT EXISTS` is a no-op on
        // an existing schema, so without this a namespace another writer
        // created first keeps that writer as its owner — able to read, alter
        // and drop everything in it — while every grant below still succeeds
        // and reads as correct. See `roles::assert_schema_owned_sql`.
        crate::roles::assert_schema_owned_sql(&schema_raw, owner_role)?,
    ];

    match grant {
        GrantLevel::ReadWrite => {
            // `CREATE` on the schema is what confers DDL, and it is enough: the
            // writer creates its own tables and therefore owns them, so it can
            // ALTER and DROP them freely. Scoped to this schema only — the role
            // gets nothing on `public` or any sibling.
            //
            // Schema *ownership* deliberately stays with the database owner.
            // Transferring it would need `ALTER SCHEMA … OWNER TO`, which
            // requires the executing role to be a member of the target role —
            // a non-superuser database owner is not, so that statement fails on
            // Neon exactly as it does locally. Keeping ownership also means a
            // compromised writer cannot drop its own schema out from under Oxy.
            sql.push(format!("GRANT USAGE, CREATE ON SCHEMA {schema} TO {role}"));
        }
        GrantLevel::ReadOnly => {
            sql.push(format!("GRANT USAGE ON SCHEMA {schema} TO {role}"));
            sql.push(format!(
                "GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO {role}"
            ));
            sql.push(format!(
                "ALTER DEFAULT PRIVILEGES IN SCHEMA {schema} GRANT SELECT ON TABLES TO {role}"
            ));
        }
    }

    // Belt and braces: no writer may create objects in `public`, whatever the
    // provider's defaults were.
    sql.push(format!("REVOKE CREATE ON SCHEMA public FROM {role}"));

    Ok(sql)
}

/// Database-level default `search_path`, covering every writer schema.
///
/// Without this the analyst connects on Postgres's default (`"$user", public`)
/// and no unqualified name resolves, because every table lives in a writer's
/// schema. The IDE lists bare table names and generates `SELECT * FROM
/// "inventory"` when you click one, so the browser would be handing you SQL it
/// cannot run.
///
/// Safe alongside [`search_path_option`]: connection-level options outrank a
/// database default, so a writer keeps its single-schema containment. This only
/// changes what a connection setting nothing sees — the analyst.
///
/// `ALTER DATABASE` is used rather than `ALTER ROLE … SET`, which needs
/// superuser; the database owner may always alter its own database.
///
/// Ordering is caller-supplied and load-bearing on collision: if two schemas
/// both held `orders`, an unqualified query silently takes the first. Analysis
/// should still qualify — this is for ergonomics, not a guarantee.
pub fn database_search_path_sql(
    database: &str,
    schemas: &[String],
) -> Result<Vec<String>, SchemaError> {
    validate_name(database)?;
    let mut path = vec!["\"$user\"".to_string(), "public".to_string()];
    for s in schemas {
        validate_name(s)?;
        path.push(quote_ident(s));
    }
    Ok(vec![format!(
        "ALTER DATABASE {} SET search_path = {}",
        quote_ident(database),
        path.join(", ")
    )])
}

/// libpq connection options pinning `search_path` to the writer's own schema.
///
/// Containment, not convenience: an app writing `UPDATE customers` resolves to
/// its own table and cannot reach a same-named table in a sibling schema.
/// `pg_catalog` stays implicitly reachable; `public` is deliberately excluded.
///
/// Carried on the **DSN** rather than set with `ALTER ROLE … SET search_path`,
/// for two reasons. Practically, altering another role's settings requires
/// superuser — the database owner is not one on a managed provider, so that
/// statement fails. More importantly, a DSN-borne setting is issued fresh by
/// whoever mints the credential, where role-level state persists and could be
/// altered away.
pub fn search_path_option(writer: &WriterRef) -> String {
    // `=` must be percent-encoded inside a URI query parameter.
    format!("options=-csearch_path%3D{}", writer.schema_name())
}

/// Append [`search_path_option`] to a base DSN.
pub fn with_search_path(dsn: &str, writer: &WriterRef) -> String {
    with_search_path_to(dsn, &writer.schema_name())
}

/// `dsn` with `schema` as its one `search_path` entry. `schema` is a name
/// this crate derived and validated — a writer's schema, or a sandbox's
/// (`crate::sandbox_schema::SandboxSchema`) — never caller text.
pub(crate) fn with_search_path_to(dsn: &str, schema: &str) -> String {
    let sep = if dsn.contains('?') { '&' } else { '?' };
    // `=` must be percent-encoded inside a URI query parameter.
    format!("{dsn}{sep}options=-csearch_path%3D{schema}")
}

// ── Analyst visibility ───────────────────────────────────────────────────────
//
// Exposing a schema to the analyst needs **two connections**, because two
// different roles own the objects involved:
//
// - the *schema* is owned by the database owner → it grants `USAGE`
// - the *tables* are owned by the writer that created them → it grants `SELECT`
//
// Postgres requires you to own an object (or hold GRANT OPTION) to grant on it,
// so a single connection cannot issue both. `ALTER DEFAULT PRIVILEGES FOR ROLE
// <writer>` likewise requires membership in that role, which the database owner
// does not have on a managed provider.

/// Run **as the database owner**, after a migration.
///
/// Migrations execute as the owner, so any table they create is owned by the
/// owner — not by the writer. The writer's grants and default privileges only
/// ever covered tables the *writer* created, so without this a migration
/// produces tables the app cannot read or write and the analyst cannot see.
/// That failure is silent until someone queries.
///
/// Sequences matter as much as tables: a `BIGSERIAL` column is backed by a
/// sequence, and an `INSERT` fails without `USAGE` on it.
///
/// The `ALTER DEFAULT PRIVILEGES FOR ROLE <owner>` lines are the durable half —
/// they cover every table a *future* migration creates, so this stops being a
/// catch-up step and becomes a standing rule.
pub fn reconcile_migration_grants_sql(
    writer: &WriterRef,
    owner_role: &str,
    analyst_visible: bool,
    analyst_role: &str,
    writer_role: &str,
) -> Result<Vec<String>, SchemaError> {
    validate_name(owner_role)?;
    let schema = quote_ident(&writer.schema_name());
    let rw = quote_ident(writer_role);
    let owner = quote_ident(owner_role);

    // `GRANT ... ON ALL TABLES IN SCHEMA` would reach tables the *writer* owns,
    // where the owner has no grant option — and Postgres skips those with a
    // warning rather than an error, so the statement would report success
    // having done less than it says. Iterate only what the owner actually owns,
    // which is exactly the set a migration created.
    let schema_lit = writer.schema_name();
    let rw_lit = writer_role.to_string();
    let mut sql = vec![
        format!(
            "DO $$ DECLARE r record; BEGIN \
               FOR r IN SELECT tablename FROM pg_tables \
                        WHERE schemaname = '{schema_lit}' AND tableowner = current_user \
               LOOP EXECUTE format('GRANT ALL ON TABLE %I.%I TO %I', \
                                   '{schema_lit}', r.tablename, '{rw_lit}'); \
               END LOOP; END $$"
        ),
        format!(
            "DO $$ DECLARE r record; BEGIN \
               FOR r IN SELECT sequencename FROM pg_sequences \
                        WHERE schemaname = '{schema_lit}' AND sequenceowner = current_user \
               LOOP EXECUTE format('GRANT ALL ON SEQUENCE %I.%I TO %I', \
                                   '{schema_lit}', r.sequencename, '{rw_lit}'); \
               END LOOP; END $$"
        ),
        format!(
            "ALTER DEFAULT PRIVILEGES FOR ROLE {owner} IN SCHEMA {schema} \
             GRANT ALL ON TABLES TO {rw}"
        ),
        format!(
            "ALTER DEFAULT PRIVILEGES FOR ROLE {owner} IN SCHEMA {schema} \
             GRANT ALL ON SEQUENCES TO {rw}"
        ),
    ];

    if analyst_visible {
        sql.extend(grant_analyst_owner_tables_sql(
            writer,
            owner_role,
            analyst_role,
        )?);
    }
    Ok(sql)
}

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

/// Tear down a writer's schema and everything in it.
///
/// `CASCADE` is intentional — the schema is the unit of ownership, so dropping
/// it half-way would strand tables no remaining role can reach. Callers are
/// responsible for having meant it.
/// Drop a writer's schema (CASCADE, so its tables go too) and clear the grants
/// that would block dropping its role. Takes the **real** `role_name` rather than
/// deriving it, because on a shared-cluster provider the bare name is another
/// tenant's role — the same reason [`ensure_writer_sql`] takes it explicitly. The
/// schema name is per-database and needs no qualification. Role deletion itself
/// is [`crate::roles::drop_role_plan`]; this only clears its schema and grants.
pub fn drop_writer_sql(writer: &WriterRef, role_name: &str) -> Result<Vec<String>, SchemaError> {
    validate_name(role_name)?;
    let schema = quote_ident(&writer.schema_name());
    let role = quote_ident(role_name);
    Ok(vec![
        format!("DROP SCHEMA IF EXISTS {schema} CASCADE"),
        format!("REVOKE ALL ON SCHEMA public FROM {role}"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two tenants on ONE cluster must never resolve to the same role.
    ///
    /// Roles are cluster-global while `oxy_analyst_ro` and `app_<slug>_rw` are
    /// fixed names. Before qualification the second tenant's mint failed with
    /// `permission denied to alter role` — loud, and wrong for the right
    /// reason. Handing role DDL a superuser removed that barrier without
    /// removing the collision, which would have made two tenants silently share
    /// one credential: one password, last mint wins, and tenant B's writer
    /// authenticating against tenant A's database.
    #[test]
    fn a_shared_cluster_gives_each_tenant_its_own_role_names() {
        let a = "oxy_org_11111111_1111_1111_1111_111111111111";
        let b = "oxy_org_22222222_2222_2222_2222_222222222222";

        assert_ne!(
            analyst_role_for("local", a),
            analyst_role_for("local", b),
            "two tenants on one cluster must not share the analyst role"
        );
        assert_ne!(
            qualify_role("local", a, "app_bookings_rw"),
            qualify_role("local", b, "app_bookings_rw"),
            "two apps with the same slug must not share a writer role"
        );

        // Deterministic — the resolver derives the same name without storing it.
        assert_eq!(analyst_role_for("local", a), analyst_role_for("local", a));

        // Neon gives each tenant its own cluster, so the clean name is safe and
        // qualifying it would only make DSNs harder to read.
        assert_eq!(analyst_role_for("neon", a), ANALYST_ROLE);
        assert_eq!(
            qualify_role("neon", a, "app_bookings_rw"),
            "app_bookings_rw"
        );

        // Still a legal identifier, inside Postgres's 63-byte cap.
        let qualified = qualify_role("local", a, "app_bookings_rw");
        assert!(qualified.len() <= 63, "{qualified}");
        assert!(validate_name(&qualified).is_ok(), "{qualified}");
    }

    #[test]
    fn app_and_pipeline_namespaces_do_not_collide() {
        let app = WriterRef::app("toast").unwrap();
        let pipeline = WriterRef::pipeline("toast").unwrap();
        assert_eq!(app.schema_name(), "app_toast");
        assert_eq!(pipeline.schema_name(), "raw_toast");
        assert_ne!(app.schema_name(), pipeline.schema_name());
    }

    #[test]
    fn role_names_are_deterministic_and_grant_specific() {
        let w = WriterRef::app("bookings").unwrap();
        assert_eq!(w.role_name(GrantLevel::ReadWrite), "app_bookings_rw");
        assert_eq!(w.role_name(GrantLevel::ReadOnly), "app_bookings_ro");
    }

    #[test]
    fn rejects_injection_shaped_names() {
        for bad in [
            "a\"; DROP SCHEMA public; --",
            "a'; DROP TABLE x; --",
            "app; SELECT 1",
            "has space",
            "Has-Upper",
            "has-hyphen",
            "1leading_digit",
            "_leading_underscore",
            "",
        ] {
            assert!(
                WriterRef::app(bad).is_err(),
                "expected rejection of {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_names_that_would_overflow_a_postgres_identifier() {
        let ok = "a".repeat(MAX_NAME_LEN);
        let too_long = "a".repeat(MAX_NAME_LEN + 1);
        assert!(WriterRef::app(&ok).is_ok());
        assert!(WriterRef::app(&too_long).is_err());

        // The longest derived form must still fit in 63 bytes.
        let w = WriterRef::app(&ok).unwrap();
        assert!(w.role_name(GrantLevel::ReadWrite).len() <= MAX_IDENT_LEN);
    }

    #[test]
    fn accepts_digits_and_underscores_after_the_first_char() {
        assert!(WriterRef::pipeline("toast_v2").is_ok());
        assert!(WriterRef::app("a").is_ok());
    }

    #[test]
    fn read_write_writer_gets_ddl_without_owning_the_schema() {
        let w = WriterRef::app("bookings").unwrap();
        let sql = ensure_writer_sql(
            &w,
            GrantLevel::ReadWrite,
            "oxy_owner",
            &w.role_name(GrantLevel::ReadWrite),
        )
        .unwrap();
        assert!(
            sql.iter().any(
                |s| s == r#"GRANT USAGE, CREATE ON SCHEMA "app_bookings" TO "app_bookings_rw""#
            ),
            "rw writer needs CREATE to do DDL, got: {sql:#?}"
        );
        // `ALTER SCHEMA … OWNER TO` requires the executing role to be a member
        // of the target role. The database owner is not a superuser on Neon, so
        // that statement fails there — and ownership is not needed anyway.
        assert!(
            !sql.iter().any(|s| s.contains("OWNER TO")),
            "schema ownership must stay with the database owner, got: {sql:#?}"
        );
    }

    #[test]
    fn read_only_writer_never_gets_ddl_or_ownership() {
        let w = WriterRef::pipeline("toast").unwrap();
        let sql = ensure_writer_sql(
            &w,
            GrantLevel::ReadOnly,
            "oxy_owner",
            &w.role_name(GrantLevel::ReadOnly),
        )
        .unwrap();
        let joined = sql.join("; ");
        assert!(!joined.contains("OWNER TO"), "got: {joined}");
        assert!(
            !joined.contains("CREATE ON SCHEMA \"raw_toast\""),
            "got: {joined}"
        );
        assert!(
            joined.contains("GRANT SELECT ON ALL TABLES"),
            "got: {joined}"
        );
        assert!(
            joined.contains("ALTER DEFAULT PRIVILEGES"),
            "future tables must be covered too, got: {joined}"
        );
    }

    #[test]
    fn every_writer_is_revoked_create_on_public() {
        for grant in [GrantLevel::ReadWrite, GrantLevel::ReadOnly] {
            let w = WriterRef::app("x").unwrap();
            let sql = ensure_writer_sql(&w, grant, "oxy_owner", &w.role_name(grant)).unwrap();
            assert!(
                sql.iter()
                    .any(|s| s.starts_with("REVOKE CREATE ON SCHEMA public")),
                "{grant:?} writer kept CREATE on public"
            );
        }
    }

    #[test]
    fn no_generated_sql_grants_superuser_or_touches_a_sibling_schema() {
        let w = WriterRef::app("bookings").unwrap();
        let all = ensure_writer_sql(
            &w,
            GrantLevel::ReadWrite,
            "oxy_owner",
            &w.role_name(GrantLevel::ReadWrite),
        )
        .unwrap();
        let joined = all.join("; ").to_ascii_lowercase();
        assert!(!joined.contains("superuser"));
        assert!(!joined.contains("rolsuper"));
        assert!(!joined.contains("app_other"));
        assert!(!joined.contains("raw_"));
    }

    #[test]
    fn schema_creation_precedes_every_grant_on_it() {
        let w = WriterRef::app("bookings").unwrap();
        let sql = ensure_writer_sql(
            &w,
            GrantLevel::ReadWrite,
            "oxy_owner",
            &w.role_name(GrantLevel::ReadWrite),
        )
        .unwrap();
        let create_at = sql
            .iter()
            .position(|s| s.starts_with("CREATE SCHEMA"))
            .expect("schema is created");
        let first_grant = sql
            .iter()
            .position(|s| s.contains("app_bookings\" OWNER") || s.starts_with("GRANT"))
            .expect("something is granted");
        assert!(create_at < first_grant, "got: {sql:#?}");
    }

    #[test]
    fn ensure_writer_rejects_an_invalid_owner_role() {
        let w = WriterRef::app("bookings").unwrap();
        assert!(ensure_writer_sql(&w, GrantLevel::ReadWrite, "oxy\"owner", "app_x_rw").is_err());
    }

    #[test]
    fn drop_is_idempotent_shaped() {
        let w = WriterRef::app("bookings").unwrap();
        // The qualified role is passed in (bare here, as on Neon), so the REVOKE
        // targets the same role the resolver/provisioner would.
        let sql = drop_writer_sql(&w, "app_bookings_rw").unwrap();
        assert!(sql[0].contains("DROP SCHEMA IF EXISTS \"app_bookings\" CASCADE"));
        assert!(sql[1].contains("REVOKE ALL ON SCHEMA public FROM \"app_bookings_rw\""));
        // An invalid role name is rejected, not escaped into safety.
        assert!(drop_writer_sql(&w, "bad role").is_err());
    }

    // ── analyst / search_path ────────────────────────────────────────────────

    #[test]
    fn pipelines_are_analytics_visible_by_default_and_apps_are_not() {
        assert!(
            WriterRef::pipeline("toast")
                .unwrap()
                .analytics_visible_by_default()
        );
        assert!(
            !WriterRef::app("bookings")
                .unwrap()
                .analytics_visible_by_default(),
            "live app state must not be readable without an explicit opt-in"
        );
    }

    #[test]
    fn search_path_rides_the_dsn_not_role_state() {
        let w = WriterRef::app("bookings").unwrap();
        let sql = ensure_writer_sql(
            &w,
            GrantLevel::ReadWrite,
            "oxy_owner",
            &w.role_name(GrantLevel::ReadWrite),
        )
        .unwrap();
        // ALTER ROLE … SET needs superuser, which the database owner is not on
        // a managed provider — so this must not appear in the DDL at all.
        assert!(
            !sql.iter().any(|s| s.contains("search_path")),
            "search_path must not be set via DDL, got: {sql:#?}"
        );

        let opt = search_path_option(&w);
        assert_eq!(opt, "options=-csearch_path%3Dapp_bookings");
        assert!(!opt.contains("public"), "public must be excluded: {opt}");
    }

    #[test]
    fn database_default_search_path_includes_every_writer_schema() {
        let sql = database_search_path_sql(
            "neondb",
            &["app_bookings".to_string(), "raw_toast".to_string()],
        )
        .unwrap();
        assert_eq!(
            sql[0],
            r#"ALTER DATABASE "neondb" SET search_path = "$user", public, "app_bookings", "raw_toast""#
        );
    }

    #[test]
    fn database_search_path_rejects_an_injected_schema_name() {
        assert!(
            database_search_path_sql("neondb", &["a\"; DROP SCHEMA public; --".to_string()])
                .is_err()
        );
        assert!(database_search_path_sql("neon\"db", &[]).is_err());
    }

    #[test]
    fn with_search_path_respects_an_existing_query_string() {
        let w = WriterRef::app("bookings").unwrap();
        let base = "postgres://u:p@h/db?sslmode=require";
        let dsn = with_search_path(base, &w);
        assert_eq!(
            dsn,
            "postgres://u:p@h/db?sslmode=require&options=-csearch_path%3Dapp_bookings"
        );
        assert!(with_search_path("postgres://u:p@h/db", &w).contains("?options="));
    }

    #[test]
    fn migration_grants_only_touch_objects_the_owner_owns() {
        let w = WriterRef::app("bookings").unwrap();
        let sql = reconcile_migration_grants_sql(&w, "oxy_owner", false, ANALYST_ROLE, "app_x_rw")
            .unwrap()
            .join("; ");
        // `GRANT ... ON ALL TABLES IN SCHEMA` errors on tables the writer owns,
        // because the owner has no grant option there. The iteration must be
        // filtered to current_user's own objects.
        assert!(
            !sql.contains("ON ALL TABLES IN SCHEMA"),
            "blanket grant errors on writer-owned tables: {sql}"
        );
        assert!(sql.contains("tableowner = current_user"), "got {sql}");
        // A BIGSERIAL column is backed by a sequence; INSERT fails without it.
        assert!(sql.contains("sequenceowner = current_user"), "got {sql}");
        // Future migrations must be covered without re-running this.
        assert!(
            sql.contains("ALTER DEFAULT PRIVILEGES FOR ROLE \"oxy_owner\""),
            "got {sql}"
        );
    }

    #[test]
    fn migration_grants_respect_the_analytics_opt_in() {
        let w = WriterRef::app("bookings").unwrap();
        let hidden =
            reconcile_migration_grants_sql(&w, "oxy_owner", false, ANALYST_ROLE, "app_x_rw")
                .unwrap()
                .join("; ");
        let shown = reconcile_migration_grants_sql(&w, "oxy_owner", true, ANALYST_ROLE, "app_x_rw")
            .unwrap()
            .join("; ");
        assert!(!hidden.contains(ANALYST_ROLE), "got {hidden}");
        assert!(shown.contains(ANALYST_ROLE), "got {shown}");
    }

    #[test]
    fn analyst_table_grants_cover_future_tables() {
        let w = WriterRef::pipeline("toast").unwrap();
        let sql = grant_analyst_tables_sql(&w, ANALYST_ROLE).join("; ");
        assert!(
            sql.contains(r#"ALTER DEFAULT PRIVILEGES IN SCHEMA "raw_toast""#),
            "without this, tables the app adds later are silently unreadable: {sql}"
        );
        assert!(sql.contains(r#"GRANT SELECT ON ALL TABLES IN SCHEMA "raw_toast""#));
    }

    #[test]
    fn analyst_grants_split_by_who_owns_the_object() {
        // Postgres requires ownership to grant. The schema belongs to the
        // database owner, the tables to the writer — so these cannot be one
        // batch on one connection.
        let w = WriterRef::pipeline("toast").unwrap();
        let schema_side = grant_analyst_schema_sql(&w, ANALYST_ROLE).join("; ");
        let table_side = grant_analyst_tables_sql(&w, ANALYST_ROLE).join("; ");

        assert!(schema_side.contains("GRANT USAGE ON SCHEMA"));
        assert!(!schema_side.contains("ON ALL TABLES"), "got {schema_side}");
        assert!(
            !table_side.contains("GRANT USAGE ON SCHEMA"),
            "got {table_side}"
        );

        // `ALTER DEFAULT PRIVILEGES FOR ROLE <writer>` would require membership
        // in that role, which the database owner lacks on a managed provider.
        assert!(!table_side.contains("FOR ROLE"), "got {table_side}");
    }

    #[test]
    fn analyst_is_never_granted_write_or_ddl() {
        let w = WriterRef::pipeline("toast").unwrap();
        let mut all = grant_analyst_tables_sql(&w, ANALYST_ROLE);
        all.extend(grant_analyst_schema_sql(&w, ANALYST_ROLE));
        let joined = all.join("; ");
        for forbidden in ["INSERT", "UPDATE", "DELETE", "TRUNCATE", "CREATE ON SCHEMA"] {
            assert!(
                !joined.contains(&format!("GRANT {forbidden}"))
                    && !joined.contains(&format!(", {forbidden}")),
                "analyst must never receive {forbidden}: {joined}"
            );
        }
    }

    #[test]
    fn revoking_analyst_access_clears_default_privileges_first() {
        let w = WriterRef::app("bookings").unwrap();
        let sql = revoke_analyst_tables_sql(&w, ANALYST_ROLE);
        assert!(
            sql[0].contains("ALTER DEFAULT PRIVILEGES"),
            "default privileges must be revoked first, or future tables stay \
             readable: {sql:#?}"
        );
        assert!(
            revoke_analyst_schema_sql(&w, ANALYST_ROLE)[0].contains("REVOKE USAGE ON SCHEMA"),
            "schema visibility is withdrawn by the schema owner"
        );
    }

    #[test]
    fn every_analyst_statement_names_the_analyst_role() {
        let w = WriterRef::app("bookings").unwrap();
        for s in grant_analyst_schema_sql(&w, ANALYST_ROLE)
            .iter()
            .chain(grant_analyst_tables_sql(&w, ANALYST_ROLE).iter())
            .chain(revoke_analyst_schema_sql(&w, ANALYST_ROLE).iter())
            .chain(revoke_analyst_tables_sql(&w, ANALYST_ROLE).iter())
        {
            assert!(s.contains(ANALYST_ROLE), "got {s}");
        }
    }

    #[test]
    fn app_writer_name_is_derived_and_validated_from_the_slug() {
        // Hyphens (legal in a slug) become underscores (required by an
        // identifier), and the result is what `ctx.oltp` resolves — the binding
        // that keeps one app out of another's schema.
        assert_eq!(
            app_writer_name("oltp-bookings").as_deref(),
            Some("oltp_bookings")
        );
        assert_eq!(app_writer_name("bookings").as_deref(), Some("bookings"));
        // And it composes to the app's own schema, nothing else.
        let w = WriterRef::app(app_writer_name("oltp-bookings").unwrap()).unwrap();
        assert_eq!(w.schema_name(), "app_oltp_bookings");

        // A slug that can't be a valid identifier fails closed (None), so
        // `ctx.oltp` reads as unavailable rather than resolving something odd.
        assert_eq!(app_writer_name("1bad"), None); // must start with a letter
        assert_eq!(app_writer_name("weird$name"), None); // invalid char
        assert_eq!(app_writer_name(""), None);

        // INJECTIVITY (the security property): distinct slugs must never derive
        // the same writer. `my-app` and `my_app` are distinct slugs; the second
        // is outside `is_valid_slug` and only the unvalidated `/publish` field
        // can produce it, so it must fail closed rather than alias `my-app`'s
        // schema, role and password. A future relaxation of the slug charset to
        // allow `_` would otherwise reopen the cross-app hole silently.
        assert_eq!(app_writer_name("my_app"), None, "underscore slugs refuse");
        assert_ne!(
            app_writer_name("my-app"),
            app_writer_name("my_app"),
            "distinct slugs must not collide onto one writer"
        );
    }
}
