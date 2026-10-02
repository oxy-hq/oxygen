//! Dropping one non-production environment's sibling Airhouse schema, and the
//! ledger rows that said its files were applied — the Airhouse step of a
//! sandbox's teardown (`custom_apps_sandboxes::teardown`,
//! `internal-docs/custom-app-sandboxes.md` → Lifecycle).
//!
//! **The schema is always computed, never taken from input**: it is the name
//! [`AirhouseHome::for_environment`] gives the app's slug and the sandbox in
//! the teardown's payload, and nothing else is ever dropped. Three guards
//! stand between that name and a `DROP`, each on its own:
//!
//! - the environment must be a **sandbox** — production has no sibling, and
//!   staging's is never dropped;
//! - the name must read back as a sibling (`airhouse::app_schema::
//!   is_environment_schema`);
//! - the name must not be **another app's own schema**. A legacy slug holding
//!   `--` derives a production schema with a sibling's shape (`a--dev-b` and
//!   app `a`'s sandbox `dev-b` are both `app_a__dev_b`), and the shape cannot
//!   tell them apart — the workspace's apps can ([`schema_owner`]).
//!
//! **Nothing connects unless there is something to drop.** A sandbox whose app
//! declares no `airhouseMigrations` never had a sibling: the only thing that
//! creates one is an apply (`airhouse::apply_airhouse_over_until`), and an
//! apply that ran a file recorded it. So no ledger row for `schema:<sibling>`
//! means no mint and no connection — most teardowns.
//!
//! **With ledger rows, the drop finishes or fails — it is never skipped.** A
//! worker with no Airhouse configured cannot drop a sibling the ledger says
//! exists, and says so: the teardown fails, the sandbox stays `deleting`, and
//! a worker that can reach Airhouse finishes it. "Nothing to drop" is said
//! only when nothing was ever applied.
//!
//! **Idempotent.** Each step is `IF EXISTS`, and the ledger rows are cleared
//! last: a teardown that died after dropping the tables finds the rows still
//! there, connects again, and finishes.
//!
//! **A refused `DROP SCHEMA` is not a failure** once every relation is gone:
//! whether a scoped Writer may drop its own schema is Airhouse's to decide, and
//! an empty schema that every listing already hides is not worth a teardown
//! that can never complete. It is reported as `schema_dropped: false`.

use entity::custom_app_migrations;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use tokio_postgres::SimpleQueryMessage;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use super::airhouse::{airhouse_lock_key, connect_as_app};
use super::airhouse_home::{AirhouseHome, schema_owner};
use super::apply::pg_detail;
use super::types::{MigrationError, STORE_AIRHOUSE};

/// Whose sibling a drop removes.
#[derive(Clone, Copy, Debug)]
pub struct AirhouseDrop<'a> {
    pub app_id: Uuid,
    pub app_slug: &'a str,
    /// The app's workspace, whose Airhouse the sibling lives in.
    pub workspace_id: Uuid,
}

/// What a drop did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DropOutcome {
    /// The sibling that was dropped; `None` when there was nothing to drop and
    /// nothing connected.
    pub schema: Option<String>,
    pub relations_dropped: usize,
    /// `false` when Airhouse refused `DROP SCHEMA` after every relation was
    /// gone: an empty, hidden schema remains.
    pub schema_dropped: bool,
    pub ledger_rows_cleared: u64,
}

/// Drop `environment`'s sibling of the app's Airhouse schema and clear its
/// ledger rows. See the module docs for when nothing happens.
#[instrument(skip(db), fields(app_id = %run.app_id, app_slug = %run.app_slug, %environment))]
pub(crate) async fn drop_environment_schema(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
) -> Result<DropOutcome, MigrationError> {
    let Some(home) = sandbox_sibling(db, run, environment).await? else {
        return Ok(DropOutcome::default());
    };
    // No ledger row: no file was ever applied there, so there is no sibling
    // to drop — on this worker or any other.
    let applied = ledger_rows(db, run.app_id, &home).await?;
    if applied == 0 {
        return Ok(DropOutcome::default());
    }
    // The ledger says a sibling exists and this worker cannot reach it. That
    // is a failure, not "nothing to drop": the teardown would remove the row,
    // and the next sandbox of the name would inherit the tables and a ledger
    // that says its files are applied.
    if airhouse::wire_endpoint().is_none() {
        return Err(MigrationError::Infra {
            filename: String::new(),
            message: format!(
                "this worker has no Airhouse configured, and the ledger records {applied} \
                 file(s) applied to {}: a worker that can reach it must drop it",
                home.schema()
            ),
        });
    }

    // The apply's own lock for this target, so a drop never interleaves with
    // a queued apply of the same sandbox's migrations.
    let pool = db.get_postgres_connection_pool();
    let mut lock = pool
        .begin()
        .await
        .map_err(|e| MigrationError::Db(format!("open the drop lock: {e}")))?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(airhouse_lock_key(run.app_id, home.target()))
        .fetch_one(&mut *lock)
        .await
        .map_err(|e| MigrationError::Db(format!("acquire the drop lock: {e}")))?;
    if !got {
        return Err(MigrationError::Busy);
    }
    let outcome = match connect_as_app(run.workspace_id, run.app_slug, home.schema()).await {
        Ok(client) => drop_environment_schema_over(db, run, environment, &client).await,
        Err(e) => Err(e),
    };
    // Ending the transaction releases the lock; a failed rollback is moot.
    let _ = lock.rollback().await;
    outcome
}

/// The one schema a drop for `run` and `environment` may touch: the sandbox's
/// sibling. `Err` for anything but a sandbox. `Ok(None)` when the sandbox has
/// no sibling of its own — [`sibling_home`] names none, or the name is another
/// app's own schema ([`schema_owner`]): a sandbox that could never have been
/// given a sibling has none to drop.
async fn sandbox_sibling(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
) -> Result<Option<AirhouseHome>, MigrationError> {
    let Some(home) = sibling_home(run.app_slug, environment)? else {
        return Ok(None);
    };
    let owner = schema_owner(db, run.workspace_id, run.app_id, home.schema())
        .await
        .map_err(|e| MigrationError::Db(e.to_string()))?;
    if let Some(owner) = owner {
        warn!(schema = home.schema(), %owner,
            "the sandbox's sibling name is another app's own schema; nothing is dropped");
        return Ok(None);
    }
    Ok(Some(home))
}

/// The sibling the slug and the sandbox name. `Err` for a fixed environment —
/// production's schema and staging's sibling are never dropped, whatever the
/// caller checked. `Ok(None)` for a slug too long for the label, or one that
/// names no schema at all.
fn sibling_home(
    app_slug: &str,
    environment: &AppEnvironment,
) -> Result<Option<AirhouseHome>, MigrationError> {
    if !matches!(environment, AppEnvironment::Dev { .. }) {
        return Err(MigrationError::BadManifest(format!(
            "refusing to drop {environment}'s Airhouse schema: only a sandbox's sibling is dropped"
        )));
    }
    Ok(AirhouseHome::for_environment(app_slug, environment)
        .ok()
        .flatten())
}

async fn ledger_rows(
    db: &DatabaseConnection,
    app_id: Uuid,
    home: &AirhouseHome,
) -> Result<u64, MigrationError> {
    custom_app_migrations::Entity::find()
        .filter(custom_app_migrations::Column::AppId.eq(app_id))
        .filter(custom_app_migrations::Column::Store.eq(STORE_AIRHOUSE))
        .filter(custom_app_migrations::Column::Target.eq(home.target().as_key()))
        .count(db)
        .await
        .map_err(|e| MigrationError::Db(e.to_string()))
}

/// Everything that runs over `client`: drop each relation of `home`'s schema,
/// then the schema, then clear the ledger rows of `home`'s target.
///
/// Public so a test can drive the drop over a connection of its own; the
/// teardown reaches it only through [`drop_environment_schema`], which holds
/// the app's lock and connects on the app's scoped credential.
///
/// It takes the app and the sandbox, never a schema: the target is derived
/// here, again, and is refused unless it is exactly that sandbox's sibling —
/// see the module docs for the three guards.
pub async fn drop_environment_schema_over(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
    client: &tokio_postgres::Client,
) -> Result<DropOutcome, MigrationError> {
    let app_id = run.app_id;
    let Some(home) = sandbox_sibling(db, run, environment).await? else {
        return Err(MigrationError::BadManifest(format!(
            "refusing to drop a sibling for {environment} of {}: it has none of its own",
            run.app_slug
        )));
    };
    let home = &home;
    let schema = home.schema();
    if !airhouse::app_schema::is_environment_schema(schema) {
        return Err(MigrationError::BadManifest(format!(
            "refusing to drop {schema}: it is not a non-production environment's schema"
        )));
    }
    let relations = relations_of(client, schema).await?;
    for relation in &relations {
        let statement = relation.drop_statement(schema);
        client
            .simple_query(&statement)
            .await
            .map_err(|e| MigrationError::Infra {
                filename: String::new(),
                message: format!("{statement}: {}", pg_detail(&e)),
            })?;
    }
    let drop_schema = format!("DROP SCHEMA IF EXISTS {}", quote_ident(schema));
    let schema_dropped = match client.simple_query(&drop_schema).await {
        Ok(_) => true,
        Err(e) => {
            warn!(%schema, error = %pg_detail(&e), "Airhouse refused to drop the emptied sibling schema");
            false
        }
    };
    let cleared = custom_app_migrations::Entity::delete_many()
        .filter(custom_app_migrations::Column::AppId.eq(app_id))
        .filter(custom_app_migrations::Column::Store.eq(STORE_AIRHOUSE))
        .filter(custom_app_migrations::Column::Target.eq(home.target().as_key()))
        .exec(db)
        .await
        .map_err(|e| MigrationError::Db(e.to_string()))?
        .rows_affected;
    info!(%schema, relations = relations.len(), schema_dropped, cleared, "sibling Airhouse schema dropped");
    Ok(DropOutcome {
        schema: Some(schema.to_string()),
        relations_dropped: relations.len(),
        schema_dropped,
        ledger_rows_cleared: cleared,
    })
}

/// One table or view of the sibling.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Relation {
    name: String,
    is_view: bool,
}

impl Relation {
    fn drop_statement(&self, schema: &str) -> String {
        let kind = if self.is_view { "VIEW" } else { "TABLE" };
        format!(
            "DROP {kind} IF EXISTS {}.{}",
            quote_ident(schema),
            quote_ident(&self.name)
        )
    }
}

/// The sibling's relations, views first: a view reads a table, and dropping
/// the table from under it is refused where dependencies are tracked.
///
/// Read over the simple query protocol, by column name — the only shape
/// Airhouse's wire answers reliably (`airhouse::connector`). `schema` is a
/// validated sibling name, so it is safe as a literal.
async fn relations_of(
    client: &tokio_postgres::Client,
    schema: &str,
) -> Result<Vec<Relation>, MigrationError> {
    let sql = format!(
        "SELECT table_name, table_type FROM information_schema.tables \
         WHERE table_schema = '{schema}' ORDER BY table_name"
    );
    let messages = client
        .simple_query(&sql)
        .await
        .map_err(|e| MigrationError::Infra {
            filename: String::new(),
            message: format!("listing {schema}'s relations: {}", pg_detail(&e)),
        })?;
    let mut relations: Vec<Relation> = messages
        .iter()
        .filter_map(|message| match message {
            SimpleQueryMessage::Row(row) => Some(Relation {
                name: row.get("table_name")?.to_string(),
                is_view: row
                    .get("table_type")
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("VIEW")),
            }),
            _ => None,
        })
        .collect();
    relations.sort_by_key(|relation| !relation.is_view);
    Ok(relations)
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(handle: &str) -> AppEnvironment {
        AppEnvironment::Dev {
            handle: handle.into(),
        }
    }

    /// Only a sandbox has a sibling to drop: a fixed environment is refused
    /// outright. A slug too long for the label, or one that names no schema,
    /// has none.
    #[test]
    fn only_a_sandbox_with_a_sibling_has_something_to_drop() {
        for fixed in [AppEnvironment::Production, AppEnvironment::Staging] {
            let refused = sibling_home("store-ops", &fixed).expect_err("a fixed environment");
            assert!(
                refused.to_string().contains("refusing to drop"),
                "{refused}"
            );
        }
        let long = format!("a{}", "b".repeat(41));
        let none =
            |slug: &str, handle: &str| sibling_home(slug, &sandbox(handle)).expect("a sandbox");
        assert_eq!(none(&long, "abcdefghijkl"), None);
        assert_eq!(none("store_ops", "a1"), None);
        assert_eq!(none("store-ops--dev-a1", "x"), None, "a legacy `--` slug");
        let home = none("store-ops", "a1-b2").expect("a sibling");
        assert_eq!(home.schema(), "app_store_ops__dev_a1_b2");
        assert_eq!(home.target().as_key(), "schema:app_store_ops__dev_a1_b2");
    }

    /// A fixed environment is refused, and a sandbox with no sibling has
    /// nothing to drop, before either touches the database: the connection
    /// here is disconnected, so any statement panics.
    #[tokio::test]
    async fn a_refusal_and_a_sandbox_with_no_sibling_ask_the_database_nothing() {
        let db = DatabaseConnection::default();
        let run = |app_slug| AirhouseDrop {
            app_id: Uuid::nil(),
            app_slug,
            workspace_id: Uuid::nil(),
        };
        for fixed in [AppEnvironment::Production, AppEnvironment::Staging] {
            let refused = drop_environment_schema(&db, run("store-ops"), &fixed)
                .await
                .expect_err("a fixed environment's schema is never dropped");
            assert!(
                refused.to_string().contains("refusing to drop"),
                "{refused}"
            );
        }
        let outcome = drop_environment_schema(&db, run("store_ops"), &sandbox("a1"))
            .await
            .expect("nothing to drop");
        assert_eq!(outcome, DropOutcome::default());
        assert_eq!(outcome.schema, None);
    }

    #[test]
    fn a_relation_is_dropped_by_its_kind_with_quoted_names() {
        let table = Relation {
            name: "visits".into(),
            is_view: false,
        };
        assert_eq!(
            table.drop_statement("app_x__dev_a1"),
            r#"DROP TABLE IF EXISTS "app_x__dev_a1"."visits""#
        );
        let view = Relation {
            name: r#"odd"name"#.into(),
            is_view: true,
        };
        assert_eq!(
            view.drop_statement("app_x__dev_a1"),
            r#"DROP VIEW IF EXISTS "app_x__dev_a1"."odd""name""#
        );
    }
}
