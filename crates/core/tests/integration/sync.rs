//! The `.databases/` render behind `POST /api/{workspace_id}/databases/sync`
//! (`oxy::semantic::render`), against the CI fixture's real databases: the
//! BigQuery `primary_database` (key from `BIGQUERY_SAMPLE_KEY`) and the DuckDB
//! `local`.
//!
//! These were the `oxy sync` binary tests. The command is a stub now (`oxyc
//! sync` writes what the route returns), so the file assertions they made run
//! against the rendered files instead. The write rules they also pinned (skip
//! an existing file, `--overwrite` prunes a stale `models/` file and never the
//! database directory) moved with the writing, to
//! `sdk/cli/src/commands/sync.test.ts`. Nothing here touches the fixture's disk,
//! so these no longer race each other on `.databases/`.

use oxy::adapters::secrets::SecretsManager;
use oxy::config::{ConfigBuilder, ConfigManager, OnMissing, Origin, WorkingCopy};
use oxy::semantic::render::{RenderedFile, render_databases, select_databases};
use oxy::service::sync::SyncFilter;

async fn fixture_config() -> ConfigManager<WorkingCopy> {
    // The `oxy` binary installs this at startup (`crates/server/src/main.rs`
    // does it for the server); an in-process render has to, or the BigQuery
    // client panics choosing one. Errors only when already installed.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    ConfigBuilder::new()
        .with_workspace_path(oxy_test_utils::oxy_example_fixture_dir())
        .expect("fixture workspace")
        .build_with_working_copy(Origin::Disk, OnMissing::Fail)
        .await
        .expect("fixture config")
}

async fn render(database: &str, datasets: &[&str]) -> Vec<RenderedFile> {
    let config = fixture_config().await;
    let secrets = SecretsManager::from_environment().expect("secrets");
    let filter = SyncFilter {
        database: Some(database.to_string()),
        datasets: datasets.iter().map(|d| d.to_string()).collect(),
        tables: vec![],
    };
    render_databases(&config, &secrets, filter)
        .await
        .expect("render")
}

fn content<'a>(files: &'a [RenderedFile], path: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} was not rendered"))
        .content
}

/// Was `test_sync_database_creates_expected_files`: the same DDL and model
/// assertions, made on the rendered files.
#[tokio::test]
async fn bigquery_dataset_renders_its_ddl_and_one_model_per_table() {
    let files = render("primary_database", &["dbt_prod_metrics"]).await;

    let sql = content(&files, ".databases/primary_database/dbt_prod_metrics.sql");
    for table in [
        "monthly_active_organizations",
        "monthly_active_users",
        "monthly_active_organizations_with_warehouse",
        "monthly_active_users_with_warehouse",
    ] {
        assert!(
            sql.contains(&format!(
                "CREATE TABLE `df-warehouse-prod.dbt_prod_metrics.{table}"
            )),
            "SQL file missing {table} table"
        );
        let model = content(
            &files,
            &format!(".databases/primary_database/dbt_prod_metrics/models/{table}.schema.yml"),
        );
        assert!(
            model.contains(&format!("table: dbt_prod_metrics.{table}")),
            "{table}.schema.yml does not contain expected table definition"
        );
    }
    // Every path sits under the database's directory, which is what `oxyc sync`
    // refuses anything else on.
    assert!(
        files
            .iter()
            .all(|f| f.path.starts_with(".databases/primary_database/"))
    );
}

/// Was `test_sync_database_fails_with_invalid_database`: the same message,
/// which the route answers as 404 before any warehouse is asked.
#[tokio::test]
async fn an_unknown_database_is_refused_before_any_warehouse_is_asked() {
    let config = fixture_config().await;
    let filter = SyncFilter {
        database: Some("non_existent_database".to_string()),
        ..Default::default()
    };
    let err = select_databases(&config, filter).unwrap_err();
    assert!(
        err.to_string()
            .contains("Database 'non_existent_database' not found in config"),
        "{err}"
    );
}

/// Was `test_sync_entire_database_with_overwrite_flag`'s first half: syncing
/// the whole DuckDB `local` produces its files, all under `.databases/local/`.
#[tokio::test]
async fn duckdb_database_renders_under_its_own_directory() {
    let files = render("local", &[]).await;
    assert!(!files.is_empty(), "the DuckDB fixture rendered nothing");
    assert!(
        files
            .iter()
            .all(|f| f.path.starts_with(".databases/local/")),
        "{:?}",
        files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert!(
        files
            .iter()
            .any(|f| f.path.contains("/models/") && f.path.ends_with(".schema.yml"))
    );
}

/// Rendering writes nothing: the fixture's `.databases/` is not created by a
/// render, which is what lets a fleet node with no working copy serve one.
#[tokio::test]
async fn rendering_writes_nothing_to_the_workspace() {
    let dir = oxy_test_utils::oxy_example_fixture_dir().join(".databases/local");
    let before = dir.exists();
    let _ = render("local", &[]).await;
    assert_eq!(dir.exists(), before);
}
