//! The steps that give a sandbox its own OLTP schema, and take it away:
//! whether a publish should queue one ([`wants_schema`]), the two halves of
//! the queued task ([`ensure`], [`migrate`]) and the teardown's step
//! ([`drop_schema`]). The SQL is `custom_apps_migrations::sandbox_schema`'s
//! and `oxy_oltp::sandbox_schema`'s; this is where the sandbox's **row** is
//! read and written around it (`oltp_state`).
//!
//! Every function here is called under the sandbox's lock ([`super::lock`]),
//! except [`wants_schema`], which a publish asks before it queues anything.

use std::time::Duration;

use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::OltpBranch;
use oxy_oltp::branches::BranchCut;
use oxy_oltp::entity::branches::BranchStatus;
use oxy_oltp::sandbox_schema::seed::{SeedCaps, SeedReport};
use oxy_oltp::sandbox_schema::{SandboxSchema, exists_on_branch};
use oxy_oltp::schema::WriterRef;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::oltp_state::{self, OltpSchemaState};
use crate::server::api::custom_apps_migrations::{
    DeclaredMigration, MigrationError, SandboxOltp, SandboxOltpDrop, apply_to_sandbox_schema,
    create_and_seed_sandbox_schema, drop_sandbox_schema,
};
use crate::server::api::custom_apps_nonproduction::staging_task_executor::{
    BUSY_RETRY_DELAYS, HUNG_APPLY_BACKSTOP, bounded, retry_busy,
};

/// How long a seed, or one apply, may take before it is dropped — which
/// closes its connection and rolls its transaction back.
const STEP_DEADLINE: Duration = Duration::from_secs(300);

/// One sandbox of one app, in the org whose staging branch its schema is in.
#[derive(Clone, Copy, Debug)]
pub struct Sandbox<'a> {
    pub app_id: Uuid,
    pub app_slug: &'a str,
    pub org_id: Uuid,
    pub environment: &'a AppEnvironment,
}

impl Sandbox<'_> {
    /// The app's writer and the sandbox's schema, both derived from the slug
    /// and the sandbox's name. `Err` says why the two name no schema.
    fn names(&self) -> Result<(WriterRef, SandboxSchema), String> {
        let writer = oxy_oltp::schema::app_writer_name(self.app_slug)
            .and_then(|name| WriterRef::app(name).ok())
            .ok_or_else(|| format!("the app's slug '{}' backs no OLTP schema", self.app_slug))?;
        let label = match self.environment {
            AppEnvironment::Dev { .. } => self.environment.schema_label(),
            _ => None,
        }
        .ok_or_else(|| format!("{} is not a sandbox", self.environment))?;
        let schema = SandboxSchema::for_writer(&writer, &label).map_err(|e| e.to_string())?;
        Ok((writer, schema))
    }
}

/// Whether a publish to a sandbox of `app_slug` should queue its OLTP schema:
/// the org has an active staging branch, and the app an OLTP writer. Neither
/// connects to the tenant.
pub async fn wants_schema(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_slug: &str,
) -> Result<bool, String> {
    let Some(writer) =
        oxy_oltp::schema::app_writer_name(app_slug).and_then(|name| WriterRef::app(name).ok())
    else {
        return Ok(false);
    };
    let active = oxy_oltp::resolver::branch_is_active(db, org_id, OltpBranch::Staging)
        .await
        .map_err(|e| e.to_string())?;
    if !active {
        return Ok(false);
    }
    oxy_oltp::resolver::writer_is_provisioned(db, org_id, &writer)
        .await
        .map_err(|e| e.to_string())
}

/// Make the sandbox's schema usable on the branch as it is now cut: nothing
/// when `row` records it ready there and it is there; otherwise create it,
/// seed it from staging's, and record the outcome on the row.
///
/// `Ok(None)`: the org has no active staging branch. `Ok(Some(what was
/// done))`. `Err(why)`: it is not usable, and the row says `failed`.
pub async fn ensure(
    db: &DatabaseConnection,
    sandbox: Sandbox<'_>,
    row: &app_environments::Model,
) -> Result<Option<String>, String> {
    let Some(cut) = active_cut(db, sandbox.org_id).await? else {
        return Ok(None);
    };
    let (writer, schema) = sandbox.names()?;
    if oltp_state::of_row(row).is_some_and(|state| state.is_ready_on(&schema, &cut)) {
        // The row says it exists on this cut: believe the branch, not the row.
        let there = exists_on_branch(db, sandbox.org_id, OltpBranch::Staging, &schema)
            .await
            .map_err(|e| format!("check the schema {schema}: {e}"))?;
        if there == Some(true) {
            return Ok(Some(format!("{schema} was already seeded")));
        }
    }
    let run = SandboxOltp {
        app_id: sandbox.app_id,
        org_id: sandbox.org_id,
        writer: &writer,
        schema: &schema,
    };
    seed_and_record(db, sandbox, run, &cut).await
}

/// The org's staging branch as it is cut now, when it is active.
async fn active_cut(db: &DatabaseConnection, org_id: Uuid) -> Result<Option<BranchCut>, String> {
    let (_, branch) = oxy_oltp::branches::find(db, org_id, OltpBranch::Staging)
        .await
        .map_err(|e| format!("read the org's staging branch: {e}"))?;
    Ok(branch
        .filter(|row| row.status == BranchStatus::Active)
        .map(|row| BranchCut::of(&row)))
}

/// Mark the row `seeding`, create and seed the schema, and mark the row
/// `ready` or `failed` — `seeding` first, so a schema is never on the branch
/// with no row recording it for the teardown to find.
async fn seed_and_record(
    db: &DatabaseConnection,
    sandbox: Sandbox<'_>,
    run: SandboxOltp<'_>,
    cut: &BranchCut,
) -> Result<Option<String>, String> {
    let record = |state: OltpSchemaState| async move {
        oltp_state::write(db, sandbox.app_id, sandbox.environment, &state)
            .await
            .map_err(|e| format!("record its state: {e}"))
    };
    let seeding = OltpSchemaState::seeding(run.schema, cut);
    record(seeding.clone()).await?;
    let caps = SeedCaps::from_env();
    let attempt = || async {
        bounded(
            STEP_DEADLINE,
            create_and_seed_sandbox_schema(db, run, &caps),
        )
        .await
        .unwrap_or_else(|| Err(past_deadline("the seed")))
    };
    match retry_busy(&BUSY_RETRY_DELAYS, attempt).await {
        Ok(Some((seeded_on, report))) => {
            let ready = OltpSchemaState::seeding(run.schema, &seeded_on).ready(&report);
            record(ready).await?;
            Ok(Some(seed_summary(run.schema, &report)))
        }
        Ok(None) => {
            let why = "the org's OLTP staging branch went away while it was being seeded";
            record(seeding.failed(why.to_string())).await?;
            Ok(None)
        }
        Err(e) => {
            let why = format!("{} was not seeded: {e}", run.schema);
            record(seeding.failed(why.clone())).await?;
            Err(why)
        }
    }
}

fn past_deadline(step: &str) -> MigrationError {
    MigrationError::Infra {
        filename: String::new(),
        message: format!(
            "{step} passed its {}s deadline and was stopped",
            STEP_DEADLINE.as_secs()
        ),
    }
}

fn seed_summary(schema: &SandboxSchema, report: &SeedReport) -> String {
    let empty = match report.structure_only.as_slice() {
        [] => String::new(),
        tables => format!("; left empty, over the size cap: {}", tables.join(", ")),
    };
    let shared = match report.staging_dependencies.as_slice() {
        [] => String::new(),
        uses => format!(
            "; still uses staging's {} (a staging migration cannot drop it while this sandbox \
             exists)",
            uses.join(", ")
        ),
    };
    format!(
        "{schema} seeded from staging's ({} tables, {} rows{empty}{shared})",
        report.tables.len(),
        report.rows_copied
    )
}

/// Apply `declared` to the sandbox's schema under its own ledger target.
/// `Ok(what was applied)`, or `Err(why not)`.
pub async fn migrate(
    db: &DatabaseConnection,
    sandbox: Sandbox<'_>,
    build_pk: Uuid,
    declared: &[DeclaredMigration],
) -> Result<String, String> {
    let (writer, schema) = sandbox.names()?;
    let run = SandboxOltp {
        app_id: sandbox.app_id,
        org_id: sandbox.org_id,
        writer: &writer,
        schema: &schema,
    };
    let attempt = || async {
        bounded(
            STEP_DEADLINE,
            apply_to_sandbox_schema(db, run, build_pk, declared),
        )
        .await
        .unwrap_or_else(|| Err(past_deadline("the apply")))
    };
    match retry_busy(&BUSY_RETRY_DELAYS, attempt).await {
        Ok(Some(applied)) => Ok(match applied.summary() {
            summary if summary.is_empty() => "no OLTP migrations to apply".to_string(),
            summary => summary,
        }),
        Ok(None) => Err("the org's OLTP staging branch went away before its migrations ran".into()),
        Err(e) => Err(format!("its OLTP migrations were not applied: {e}")),
    }
}

/// The teardown's OLTP step: drop the sandbox's schema and its ledger rows.
/// `recorded` is whether the sandbox's row holds a state. A sandbox whose
/// names give no schema never had one.
pub async fn drop_schema(
    db: &DatabaseConnection,
    sandbox: Sandbox<'_>,
    recorded: bool,
) -> Result<SandboxOltpDrop, MigrationError> {
    let Ok((_, schema)) = sandbox.names() else {
        return Ok(SandboxOltpDrop::default());
    };
    bounded(
        HUNG_APPLY_BACKSTOP,
        drop_sandbox_schema(db, sandbox.app_id, sandbox.org_id, &schema, recorded),
    )
    .await
    .unwrap_or_else(|| {
        Err(MigrationError::Infra {
            filename: String::new(),
            message: format!("it passed its {}s deadline", HUNG_APPLY_BACKSTOP.as_secs()),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox<'a>(slug: &'a str, environment: &'a AppEnvironment) -> Sandbox<'a> {
        Sandbox {
            app_id: Uuid::nil(),
            app_slug: slug,
            org_id: Uuid::nil(),
            environment,
        }
    }

    #[test]
    fn a_sandboxs_names_are_derived_from_the_slug_and_the_sandbox() {
        let dev = AppEnvironment::Dev {
            handle: "fix-42".into(),
        };
        let (writer, schema) = sandbox("store-ops", &dev).names().expect("names");
        assert_eq!(writer.schema_name(), "app_store_ops");
        assert_eq!(schema.name(), "app_store_ops__dev_fix_42");
    }

    /// Staging and production have no sandbox schema; a slug or a pair too
    /// long for one says why rather than naming a shorter one.
    #[test]
    fn what_names_no_schema_says_why() {
        let dev = AppEnvironment::Dev {
            handle: "abcdefghijkl".into(),
        };
        assert!(
            sandbox("store", &AppEnvironment::Staging)
                .names()
                .unwrap_err()
                .contains("not a sandbox")
        );
        let long = "a".repeat(42);
        let why = sandbox(&long, &dev).names().unwrap_err();
        assert!(why.contains("never truncated"), "{why}");
        assert!(sandbox("store_ops", &dev).names().is_err());
    }

    #[test]
    fn the_seed_summary_names_the_tables_left_empty() {
        let schema = sandbox(
            "store",
            &AppEnvironment::Dev {
                handle: "a1".into(),
            },
        )
        .names()
        .expect("names")
        .1;
        let report = SeedReport {
            tables: vec!["a".into(), "b".into()],
            structure_only: vec!["b".into()],
            rows_copied: 7,
            ..SeedReport::default()
        };
        assert_eq!(
            seed_summary(&schema, &report),
            "app_store__dev_a1 seeded from staging's (2 tables, 7 rows; left empty, over the size \
             cap: b)"
        );
    }
}
