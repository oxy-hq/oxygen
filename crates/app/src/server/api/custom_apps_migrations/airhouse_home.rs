//! Where an Airhouse apply runs an app's files, and which ledger target
//! records them.
//!
//! Production runs them in the app's own schema, `app_<writer>`, under target
//! `production`. A non-production environment runs the **same files** in its
//! sibling, `app_<writer>__<label>` (`airhouse::app_schema`; the label is
//! `AppEnvironment::schema_label`, so staging's is `…__staging` and a sandbox
//! `dev-a1`'s is `…__dev_a1`), under target `schema:<sibling>` — so a file
//! applied to one environment is never read as applied to another, and promote
//! still plans production's DDL.
//!
//! The files name `app_<writer>`, as the author wrote them. For the sibling
//! each file is checked against the app's schema first (the rules production
//! applies, so staging refuses what production would), moved to the sibling,
//! and checked again against the sibling (`airhouse::sql_retarget`). The
//! second check is the fence: a statement the move missed still names the app
//! schema, and is refused there.

use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use airhouse::sql_retarget::check_retargeted;
use airhouse::sql_rules::{self, Access};

use super::types::{DeclaredMigration, MigrationError, MigrationTarget};

/// One schema an apply writes, and the ledger target it records under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirhouseHome {
    /// The app's own schema — the one every file names.
    app_schema: String,
    /// Where the files run: `app_schema`, or its sibling.
    schema: String,
    target: MigrationTarget,
}

impl AirhouseHome {
    /// The app's own schema, recorded as `production`.
    pub fn production(app_slug: &str) -> Result<Self, MigrationError> {
        let app_schema = airhouse_schema_for(app_slug)?;
        Ok(Self {
            schema: app_schema.clone(),
            app_schema,
            target: MigrationTarget::Production,
        })
    }

    /// `environment`'s sibling of the app's schema, recorded as
    /// `schema:<sibling>`. `None` for production, and for a slug or an
    /// environment that names no sibling — nothing then runs.
    pub fn for_environment(
        app_slug: &str,
        environment: &AppEnvironment,
    ) -> Result<Option<Self>, MigrationError> {
        // Production has no label: it writes the app's own schema.
        let Some(label) = environment.schema_label() else {
            return Ok(None);
        };
        let app_schema = airhouse_schema_for(app_slug)?;
        let Some(sibling) = airhouse::app_schema::environment_schema(&app_schema, &label) else {
            return Ok(None);
        };
        Ok(Some(Self {
            app_schema,
            target: MigrationTarget::Schema(sibling.clone()),
            schema: sibling,
        }))
    }

    /// The schema the files run in.
    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// The ledger target the files are recorded under.
    pub fn target(&self) -> &MigrationTarget {
        &self.target
    }

    /// `m`'s statements as they run here: checked against the app's schema,
    /// and — for a sibling — moved and checked against it too.
    pub(super) fn statements(&self, m: &DeclaredMigration) -> Result<Vec<String>, MigrationError> {
        if self.schema == self.app_schema {
            return check_file(m, &self.app_schema);
        }
        check_retargeted(&m.sql, &self.app_schema, &self.schema, Access::Ddl).map_err(|e| {
            MigrationError::AirhouseRule {
                filename: m.filename.clone(),
                message: e.to_string(),
            }
        })
    }
}

/// The slug of **another** app of the workspace whose own schema is `schema`,
/// when there is one.
///
/// A sibling's name is read back from its shape (`app_<writer>__<label>`,
/// `airhouse::app_schema::is_environment_schema`), and the shape alone cannot
/// tell app `a`'s sandbox `dev-b` from the production schema of an app whose
/// slug is `a--dev-b`: both are `app_a__dev_b`. No slug `is_valid_slug` admits
/// today holds `--`, but rows written before that rule may. So before a
/// sandbox is given a sibling — created, migrated or dropped — the workspace
/// is asked whether that name is already an app's own. Only a slug holding
/// `--` can derive one, so only those rows are read.
pub(crate) async fn schema_owner<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    app_id: Uuid,
    schema: &str,
) -> Result<Option<String>, DbErr> {
    let slugs: Vec<String> = apps::Entity::find()
        .select_only()
        .column(apps::Column::Slug)
        .filter(apps::Column::ProjectId.eq(workspace_id))
        .filter(apps::Column::Id.ne(app_id))
        .filter(apps::Column::Slug.contains("--"))
        .into_tuple()
        .all(db)
        .await?;
    Ok(slugs
        .into_iter()
        .find(|slug| airhouse_schema_for(slug).is_ok_and(|own| own == schema)))
}

/// `app_<writer>` for this slug — the same derivation `ctx.airhouse` uses.
pub(super) fn airhouse_schema_for(app_slug: &str) -> Result<String, MigrationError> {
    let writer = oxy_oltp::schema::app_writer_name(app_slug).ok_or_else(|| {
        MigrationError::BadManifest(format!(
            "oxy-app.json declares airhouseMigrations, but the app's slug '{app_slug}' cannot name \
             a schema: a slug must start with a letter, be at most {max} characters, and use only \
             lowercase letters, digits and hyphens",
            max = oxy_oltp::schema::MAX_NAME_LEN,
        ))
    })?;
    oxy_oltp::schema::WriterRef::app(&writer)
        .map(|w| w.schema_name())
        .map_err(|e| MigrationError::BadManifest(e.to_string()))
}

/// Check one file against the schema and DuckLake's rules.
pub(super) fn check_file(
    m: &DeclaredMigration,
    schema: &str,
) -> Result<Vec<String>, MigrationError> {
    sql_rules::check(&m.sql, schema, Access::Ddl).map_err(|e| MigrationError::AirhouseRule {
        filename: m.filename.clone(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(sql: &str) -> DeclaredMigration {
        DeclaredMigration {
            filename: "0001_init.sql".into(),
            checksum: "x".into(),
            sql: sql.into(),
        }
    }

    #[test]
    fn the_schema_is_the_one_ctx_airhouse_writes() {
        assert_eq!(airhouse_schema_for("store-ops").unwrap(), "app_store_ops");
        assert!(
            airhouse_schema_for("store_ops").is_err(),
            "an underscore aliases a hyphen"
        );
    }

    #[test]
    fn a_file_with_a_key_is_the_authors_to_fix() {
        let err = check_file(
            &file("CREATE TABLE app_store_ops.visits (visit_id VARCHAR PRIMARY KEY)"),
            "app_store_ops",
        )
        .unwrap_err();
        assert!(err.is_author_fault(), "{err}");
        assert!(err.to_string().contains("0001_init.sql"), "{err}");
    }

    #[test]
    fn a_clean_file_splits_into_its_statements() {
        let statements = check_file(
            &file(
                "CREATE TABLE app_store_ops.visits (visit_id VARCHAR NOT NULL, recorded_at TIMESTAMPTZ);
                 CREATE VIEW app_store_ops.latest AS SELECT * FROM app_store_ops.visits;",
            ),
            "app_store_ops",
        )
        .expect("clean");
        assert_eq!(statements.len(), 2);
    }

    #[test]
    fn staging_runs_the_same_file_in_its_sibling_under_its_own_target() {
        let home = AirhouseHome::for_environment("store-ops", &AppEnvironment::Staging)
            .unwrap()
            .expect("staging has a sibling");
        assert_eq!(home.schema(), "app_store_ops__staging");
        assert_eq!(home.target().as_key(), "schema:app_store_ops__staging");
        let statements = home
            .statements(&file(
                "CREATE SCHEMA IF NOT EXISTS app_store_ops;
                 CREATE TABLE app_store_ops.visits (visit_id VARCHAR NOT NULL);",
            ))
            .expect("moves");
        for statement in &statements {
            assert!(
                !statement
                    .replace("app_store_ops__staging", "")
                    .contains("app_store_ops"),
                "{statement}"
            );
        }
        let production = AirhouseHome::production("store-ops").unwrap();
        assert_eq!(production.schema(), "app_store_ops");
        assert_eq!(production.target(), &MigrationTarget::Production);
        assert_eq!(
            AirhouseHome::for_environment("store-ops", &AppEnvironment::Production).unwrap(),
            None
        );
    }

    /// A sandbox runs the files in a sibling of its own, recorded under that
    /// sibling's target — never staging's, never production's.
    #[test]
    fn a_sandbox_runs_the_file_in_its_own_sibling_under_its_own_target() {
        let sandbox = AppEnvironment::Dev {
            handle: "a1-b2".into(),
        };
        let home = AirhouseHome::for_environment("store-ops", &sandbox)
            .unwrap()
            .expect("a sandbox has a sibling");
        assert_eq!(home.schema(), "app_store_ops__dev_a1_b2");
        assert_eq!(home.target().as_key(), "schema:app_store_ops__dev_a1_b2");
        let statements = home
            .statements(&file(
                "CREATE TABLE app_store_ops.visits (visit_id VARCHAR NOT NULL);",
            ))
            .expect("moves");
        assert!(
            statements[0].contains("app_store_ops__dev_a1_b2"),
            "{statements:?}"
        );
    }

    /// A slug whose writer leaves no room for the sandbox's label names no
    /// sibling: nothing runs, rather than a truncated name meeting another.
    #[test]
    fn a_slug_too_long_for_the_label_has_no_sandbox_sibling() {
        let slug = format!("a{}", "b".repeat(41));
        let sandbox = AppEnvironment::Dev {
            handle: "abcdefghijkl".into(),
        };
        assert_eq!(
            AirhouseHome::for_environment(&slug, &sandbox).unwrap(),
            None
        );
        assert!(
            AirhouseHome::for_environment(&slug, &AppEnvironment::Staging)
                .unwrap()
                .is_some(),
            "control: staging's shorter label still fits"
        );
    }

    /// DuckLake's rules hold in the sibling as in production.
    #[test]
    fn the_sibling_refuses_what_production_refuses() {
        let home = AirhouseHome::for_environment("store-ops", &AppEnvironment::Staging)
            .unwrap()
            .unwrap();
        for sql in [
            "CREATE TABLE app_store_ops.visits (visit_id VARCHAR PRIMARY KEY)",
            "CREATE TABLE app_store_ops.visits (visit_id VARCHAR UNIQUE)",
            "CREATE TABLE other.visits (visit_id VARCHAR)",
        ] {
            let err = home.statements(&file(sql)).unwrap_err();
            assert!(err.is_author_fault(), "{sql}: {err}");
        }
    }
}
