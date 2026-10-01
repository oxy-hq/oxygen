//! Whether a staging mapping's database is production's under another name:
//! the host it reaches and the user it signs in as, as far as its config says.
//! Both the publish check (`check_mapping`) and the host, on every staging
//! write it isolates to a mapped database, refuse a mapping that is
//! (`mapping_refusal`) — the database config can change after a publish.
//!
//! A `*_var` field resolves through the workspace's secrets, as the connector
//! would resolve it. One that does not resolve compares by its **name**, so
//! two entries reading the same variable still match — the likeliest shape of
//! a staging entry copied from production's. At invocation an unresolved field
//! is a refusal instead ([`Unresolved::Refuse`]): the host fails closed rather
//! than send a write it could not tell apart from production's.

use oxy::adapters::secrets::SecretsManager;
use oxy::config::model::{Database, DatabaseType, DuckDBOptions};

/// A database's host and user, normalized for comparison.
#[derive(Debug, Clone)]
pub struct Identity {
    pub host: String,
    pub user: String,
    /// Every `*_var` behind `host` and `user` resolved.
    pub confirmed: bool,
}

impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host && self.user == other.user
    }
}

/// What an identity that could not be resolved means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unresolved {
    /// Compare by the variable's name (publish: a staging secret may be set
    /// after the bundle ships).
    CompareByName,
    /// Refuse (invocation: nothing is sent that could not be told apart).
    Refuse,
}

/// One config field: its literal value, or the variable it is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Field {
    Literal(String),
    Var(String),
    Unset,
}

impl Field {
    fn of(value: Option<&str>, var: Option<&str>) -> Self {
        match (value.map(str::trim), var.map(str::trim)) {
            (Some(v), _) if !v.is_empty() => Field::Literal(v.to_string()),
            (_, Some(n)) if !n.is_empty() => Field::Var(n.to_string()),
            _ => Field::Unset,
        }
    }

    /// The value the connector would use, or `var:<NAME>` — unconfirmed —
    /// when the secret does not resolve here.
    async fn resolve(self, secrets: &SecretsManager) -> (String, bool) {
        match self {
            Field::Literal(v) => (v, true),
            Field::Var(name) => match secrets.resolve_secret(&name).await {
                Ok(Some(value)) if !value.trim().is_empty() => (value.trim().to_string(), true),
                _ => (format!("var:{name}"), false),
            },
            Field::Unset => (String::new(), true),
        }
    }
}

/// Why staging may not write `staging` in place of `production`, or `None`.
///
/// Refused: a mapping onto the same database name; one onto the workspace's
/// own Airhouse (`airhouse_managed` — production's tenant, whatever the
/// production database is); one whose host and user resolve to production's
/// (a heuristic, not proof — two names can reach one database through a CNAME
/// or a second role, and the credential behind the staging entry is its
/// owner's to keep separate); and, with [`Unresolved::Refuse`], one whose
/// host or user could not be resolved.
pub async fn mapping_refusal(
    production: &Database,
    staging: &Database,
    secrets: &SecretsManager,
    unresolved: Unresolved,
) -> Option<String> {
    let (from, to) = (&production.name, &staging.name);
    if from == to {
        return Some(format!(
            "nonProduction.destinations maps `{from}` onto itself, which would write production"
        ));
    }
    if matches!(
        (&production.database_type, &staging.database_type),
        (DatabaseType::DuckDB(_), DatabaseType::DuckDB(_))
    ) {
        return Some(format!(
            "nonProduction.destinations maps `{from}` to `{to}`, and both are DuckDB: a DuckDB \
             destination runs in the same process as production's, where a statement can \
             ATTACH production's file. Map `{from}` to a warehouse with a credential of its own"
        ));
    }
    if matches!(staging.database_type, DatabaseType::AirhouseManaged(_)) {
        return Some(format!(
            "nonProduction.destinations maps `{from}` to `{to}`, the workspace's own Airhouse \
             (airhouse_managed) — production's tenant, so a staging write there is a production \
             write. Write staging facts with ctx.airhouse, whose staging writes land in the \
             app's sibling schema"
        ));
    }
    let (p, s) = (
        identity(production, secrets).await,
        identity(staging, secrets).await,
    );
    if p == s {
        return Some(format!(
            "nonProduction.destinations maps `{from}` to `{to}`, and `{to}` resolves to the same \
             host and user as `{from}` — staging writes would land on production's credential. \
             Point `{to}` at a staging database with its own credential. (This check compares \
             host and user; it is a heuristic, not proof that the two are separate.)"
        ));
    }
    if p.host == s.host && same_database(production, staging, secrets).await {
        return Some(format!(
            "nonProduction.destinations maps `{from}` to `{to}`, and `{to}` is the same database \
             on the same host as `{from}`, under another user — a staging write would land in \
             production's database. Point `{to}` at a database of its own"
        ));
    }
    if unresolved == Unresolved::Refuse && !(p.confirmed && s.confirmed) {
        return Some(format!(
            "the host or user of `{from}` or of `{to}` (nonProduction.destinations) did not \
             resolve, so staging cannot confirm `{to}` is not production's database; nothing \
             was written. Set the missing secret"
        ));
    }
    None
}

/// Whether both configs name one database (without case), by value or by the
/// variable each reads it from. Two configs that name none are not compared.
async fn same_database(a: &Database, b: &Database, secrets: &SecretsManager) -> bool {
    let (a, _) = database_field(a).resolve(secrets).await;
    let (b, _) = database_field(b).resolve(secrets).await;
    !a.trim().is_empty() && a.trim().eq_ignore_ascii_case(b.trim())
}

/// `db`'s identity. Host is lowercased with any URL scheme and trailing `/`
/// dropped, so `https://ch.example.com/` and `ch.example.com` match.
pub async fn identity(db: &Database, secrets: &SecretsManager) -> Identity {
    let (host, user) = fields(&db.database_type);
    let (host, host_confirmed) = host.resolve(secrets).await;
    let (user, user_confirmed) = user.resolve(secrets).await;
    Identity {
        host: normalize_host(&host),
        user,
        confirmed: host_confirmed && user_confirmed,
    }
}

/// The host and user fields of each kind. A kind with no host names what it
/// reaches instead (an account, a file, a platform-managed store), so two
/// entries of it compare by that.
fn fields(database_type: &DatabaseType) -> (Field, Field) {
    let lit = |s: &str| Field::Literal(s.to_string());
    match database_type {
        DatabaseType::Postgres(c) => (
            Field::of(c.host.as_deref(), c.host_var.as_deref()),
            Field::of(c.user.as_deref(), c.user_var.as_deref()),
        ),
        DatabaseType::Redshift(c) => (
            Field::of(c.host.as_deref(), c.host_var.as_deref()),
            Field::of(c.user.as_deref(), c.user_var.as_deref()),
        ),
        DatabaseType::Mysql(c) => (
            Field::of(c.host.as_deref(), c.host_var.as_deref()),
            Field::of(c.user.as_deref(), c.user_var.as_deref()),
        ),
        DatabaseType::Airhouse(c) => (
            Field::of(c.host.as_deref(), c.host_var.as_deref()),
            Field::of(c.user.as_deref(), c.user_var.as_deref()),
        ),
        DatabaseType::ClickHouse(c) => (
            Field::of(c.host.as_deref(), c.host_var.as_deref()),
            // An unset ClickHouse user is the server's `default`.
            match Field::of(c.user.as_deref(), c.user_var.as_deref()) {
                Field::Unset => lit("default"),
                user => user,
            },
        ),
        // Snowflake folds unquoted account and user names to one case.
        DatabaseType::Snowflake(c) => (
            lit(&c.account.to_ascii_lowercase()),
            lit(&c.username.to_ascii_lowercase()),
        ),
        DatabaseType::Bigquery(c) => (
            lit("bigquery"),
            Field::of(
                c.key_path.as_ref().and_then(|p| p.to_str()),
                c.key_path_var.as_deref(),
            ),
        ),
        DatabaseType::DuckDB(c) => (lit("duckdb"), lit(&duckdb_location(&c.options))),
        // One MotherDuck token reaches every database its account holds, so
        // the database is no part of who the credential is: the token is.
        DatabaseType::MotherDuck(c) => (lit("motherduck"), Field::Var(c.token_var.clone())),
        DatabaseType::DOMO(c) => (lit(&c.instance), Field::Var(c.developer_token_var.clone())),
        // Platform-managed: every entry of the kind is the same credential.
        DatabaseType::AirhouseManaged(_) => (lit("airhouse_managed"), lit("workspace")),
        DatabaseType::PostgresManaged(_) => (lit("postgres_managed"), lit("analyst")),
    }
}

/// The database `db`'s config points at, as configured — what a statement on
/// its connection names when it qualifies a table
/// (`env_policy::destination_sql`). `Ok(None)` when the config names none
/// (DuckDB, BigQuery, the managed kinds); `Err` when it names one through a
/// secret that does not resolve, which the host treats as a refusal.
pub async fn configured_database(
    db: &Database,
    secrets: &SecretsManager,
) -> Result<Option<String>, String> {
    match database_field(db).resolve(secrets).await {
        (value, true) if value.trim().is_empty() => Ok(None),
        (value, true) => Ok(Some(value.trim().to_string())),
        (value, false) => Err(format!(
            "`{}`'s database is read from {value}, which did not resolve",
            db.name
        )),
    }
}

/// What a write on `db`'s connection may qualify a table by: its configured
/// database — for BigQuery, its configured datasets (`dataset`, and the keys
/// of `datasets`), since a dataset is what `dataset.table` names there.
pub async fn configured_names(
    db: &Database,
    secrets: &SecretsManager,
) -> Result<Vec<String>, String> {
    if let DatabaseType::Bigquery(c) = &db.database_type {
        return Ok(c.dataset.iter().chain(c.datasets.keys()).cloned().collect());
    }
    Ok(configured_database(db, secrets)
        .await?
        .into_iter()
        .collect())
}

/// Where a kind's configured database is read from; `Unset` for a kind that
/// names none.
fn database_field(db: &Database) -> Field {
    match &db.database_type {
        DatabaseType::Postgres(c) => Field::of(c.database.as_deref(), c.database_var.as_deref()),
        DatabaseType::Redshift(c) => Field::of(c.database.as_deref(), c.database_var.as_deref()),
        DatabaseType::Mysql(c) => Field::of(c.database.as_deref(), c.database_var.as_deref()),
        DatabaseType::Airhouse(c) => Field::of(c.database.as_deref(), c.database_var.as_deref()),
        DatabaseType::ClickHouse(c) => {
            match Field::of(c.database.as_deref(), c.database_var.as_deref()) {
                // An unset ClickHouse database is the server's `default`.
                Field::Unset => Field::Literal("default".to_string()),
                database => database,
            }
        }
        DatabaseType::Snowflake(c) => Field::Literal(c.database.clone()),
        DatabaseType::MotherDuck(c) => Field::of(c.database.as_deref(), None),
        _ => Field::Unset,
    }
}

fn duckdb_location(options: &DuckDBOptions) -> String {
    match options {
        DuckDBOptions::Local { file_search_path } => file_search_path.clone(),
        DuckDBOptions::File { path } => path.clone(),
        DuckDBOptions::DuckLake(lake) => serde_json::to_string(lake).unwrap_or_default(),
    }
}

fn normalize_host(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    let host = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))
        .unwrap_or(&host);
    host.trim_end_matches('/').to_string()
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
