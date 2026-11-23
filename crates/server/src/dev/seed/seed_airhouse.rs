//! Give each seeded workspace its Airhouse tenant, as creating a workspace
//! does, so the demo's data has the place it has in a deployment, then load
//! the demo workspace's data files into its tenant.
//!
//! Uses the Airhouse `AIRHOUSE_*` names or, with none set, the one `oxy start`
//! runs, found where [`airhouse::config::autodetect_local_airhouse`] looks.
//! With neither it says so and the rest of the seed goes on, and a failure
//! warns rather than failing the seed, like the LLM keys. Only on a database
//! that is local by its URL: a tenant is a real database in that Airhouse, and
//! `OXY_SEED_ALLOW_REMOTE` consents to demo rows, not to that.

use std::path::{Path, PathBuf};

use airhouse::import::{
    FileFormat, IfExists, ImportError, ImportFile, TenantImporter, WorkspaceImporter,
};
use airhouse::provisioner::ProvisionerError;
use airhouse::{EnsureTenantError, EnsuredTenant, ensure_tenant};
use oxy::theme::StyledText;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::seed_compile::SeededWorkspace;

/// The directories of the demo workspace whose `.csv` / `.parquet` files are
/// loaded into its tenant. `examples/` read them as file DuckDB databases
/// (`local` from `.db/`, `training` from `.training_data/`) before both became
/// `airhouse_managed`.
const DEMO_DATA_DIRS: [&str; 2] = [".db", ".training_data"];

/// Look for `oxy start`'s Airhouse before anything reads the Airhouse config,
/// which is read once per process.
pub(crate) async fn find_local_airhouse() {
    airhouse::config::autodetect_local_airhouse().await;
}

/// Give every target its tenant. `false` when there is no Airhouse to use
/// here, so nothing was attempted.
pub(crate) async fn provision_tenants(
    conn: &DatabaseConnection,
    targets: &[SeededWorkspace],
) -> bool {
    if super::seed_partners::remote_seeding_allowed()
        || !super::seed_partners::database_url_looks_local()
    {
        println!(
            "{} skipping Airhouse tenants — not an unambiguously local database",
            "⏭️".info()
        );
        return false;
    }
    if airhouse::AirhouseConfig::cached().as_runtime().is_none() {
        println!(
            "{} skipping Airhouse tenants — no Airhouse (`oxy start` runs one)",
            "⏭️".info()
        );
        return false;
    }
    let (mut provisioned, mut ready) = (0, 0);
    for target in targets {
        match ensure_tenant(conn, target.id).await {
            Ok(EnsuredTenant::Provisioned) => provisioned += 1,
            Ok(EnsuredTenant::AlreadyActive) => ready += 1,
            // The tenant exists in Airhouse but not in this database: a
            // `--clean` that wiped oxy-postgres but not an Airhouse it adopted.
            Err(EnsureTenantError::Provision(ProvisionerError::TenantNameTaken(name))) => {
                println!(
                    "{} {}: Airhouse already has tenant {name} from before this database \
                     was reset; reset that Airhouse with `{}` and seed again",
                    "⚠️".warning(),
                    target.label,
                    oxy::database::docker::airhouse::RESET_EXTERNAL_HINT
                )
            }
            Err(e) => println!(
                "{} {}: no Airhouse tenant: {e}",
                "⚠️".warning(),
                target.label
            ),
        }
    }
    println!(
        "{} Airhouse tenants: {provisioned} provisioned, {ready} already there",
        "✅".success()
    );
    true
}

/// Load every `.csv` / `.parquet` file in the workspace's [`DEMO_DATA_DIRS`]
/// into its tenant, under the names a file DuckDB gives the file
/// ([`ImportFile::named_like_a_file_database`]), so the examples' SQL resolves
/// unchanged. A table that already exists is left as it is, so a second seed
/// loads only what is missing; drop a table to reload it.
pub(crate) async fn load_demo_data(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    workspace_path: &Path,
) {
    let importer = TenantImporter::new(conn.clone());
    let loaded = load_data_files(&importer, workspace_id, workspace_path).await;
    if loaded.files() > 0 {
        println!("{} Airhouse demo data: {loaded}", "✅".success());
    }
}

/// What [`load_data_files`] did, file by file.
#[derive(Debug, Default, PartialEq, Eq)]
struct DataLoad {
    loaded: Vec<String>,
    already_there: Vec<String>,
    failed: Vec<String>,
}

impl DataLoad {
    fn files(&self) -> usize {
        self.loaded.len() + self.already_there.len() + self.failed.len()
    }
}

impl std::fmt::Display for DataLoad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} loaded, {} already there",
            self.loaded.len(),
            self.already_there.len()
        )?;
        if !self.failed.is_empty() {
            write!(f, ", {} failed", self.failed.len())?;
        }
        Ok(())
    }
}

/// The data files of `workspace_path`, as `<dir>/<file name>` and their
/// paths, in name order.
fn data_files(workspace_path: &Path) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    for dir in DEMO_DATA_DIRS {
        let Ok(entries) = std::fs::read_dir(workspace_path.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.is_file() && FileFormat::from_file_name(name).is_some() {
                files.push((format!("{dir}/{name}"), path.clone()));
            }
        }
    }
    files.sort();
    files
}

async fn load_data_files(
    importer: &dyn WorkspaceImporter,
    workspace_id: Uuid,
    workspace_path: &Path,
) -> DataLoad {
    let mut load = DataLoad::default();
    for (shown, path) in data_files(workspace_path) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let result = match ImportFile::named_like_a_file_database(&path, name, IfExists::Fail) {
            Ok(file) => importer.import(workspace_id, &file).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(outcome) => {
                println!(
                    "  {} {shown} → {} ({} rows)",
                    "📥".info(),
                    outcome.table,
                    outcome.rows
                );
                load.loaded.push(shown);
            }
            Err(ImportError::AlreadyExists(_)) => load.already_there.push(shown),
            Err(e) => {
                println!("{} {shown} not loaded into Airhouse: {e}", "⚠️".warning());
                load.failed.push(shown);
            }
        }
    }
    load
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use airhouse::import::{ImportOutcome, TableRef};
    use async_trait::async_trait;

    use super::*;

    /// Takes every file once: a second import of the same table is refused
    /// as a real tenant refuses it under `IfExists::Fail`.
    #[derive(Default)]
    struct OnceImporter {
        tables: Mutex<Vec<TableRef>>,
    }

    #[async_trait]
    impl WorkspaceImporter for OnceImporter {
        async fn ready(&self, _: Uuid) -> Result<(), ImportError> {
            Ok(())
        }

        async fn import(&self, _: Uuid, file: &ImportFile) -> Result<ImportOutcome, ImportError> {
            assert_eq!(file.if_exists, IfExists::Fail);
            let mut tables = self.tables.lock().unwrap();
            if tables.contains(&file.target) {
                return Err(ImportError::AlreadyExists(file.target.clone()));
            }
            tables.push(file.target.clone());
            Ok(ImportOutcome {
                table: file.target.clone(),
                aliases: file.aliases.clone(),
                columns: Vec::new(),
                rows: 1,
            })
        }
    }

    fn workspace(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for file in files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "id\n1\n").unwrap();
        }
        dir
    }

    #[tokio::test]
    async fn a_second_seed_loads_only_what_is_missing() {
        let dir = workspace(&[
            ".db/orders.csv",
            ".db/stores.parquet",
            ".db/notes.txt",
            ".db/nested/inner.csv",
            ".training_data/macro.csv",
            "data/elsewhere.csv",
        ]);
        let importer = OnceImporter::default();
        let id = Uuid::nil();

        let first = load_data_files(&importer, id, dir.path()).await;
        assert_eq!(
            first.loaded,
            [
                ".db/orders.csv",
                ".db/stores.parquet",
                ".training_data/macro.csv"
            ]
        );
        assert!(first.already_there.is_empty() && first.failed.is_empty());

        std::fs::write(dir.path().join(".db/customers.csv"), "id\n1\n").unwrap();
        let second = load_data_files(&importer, id, dir.path()).await;
        assert_eq!(second.loaded, [".db/customers.csv"]);
        assert_eq!(second.already_there.len(), 3);
        assert_eq!(second.to_string(), "1 loaded, 3 already there");
    }

    #[tokio::test]
    async fn a_file_whose_name_is_refused_is_reported_and_skipped() {
        let dir = workspace(&[".db/app_x.csv", ".db/fine.csv"]);
        let load = load_data_files(&OnceImporter::default(), Uuid::nil(), dir.path()).await;
        assert_eq!(load.loaded, [".db/fine.csv"]);
        assert_eq!(load.failed, [".db/app_x.csv"]);
    }

    #[tokio::test]
    async fn a_workspace_without_data_directories_loads_nothing() {
        let dir = workspace(&["config.yml"]);
        let load = load_data_files(&OnceImporter::default(), Uuid::nil(), dir.path()).await;
        assert_eq!(load, DataLoad::default());
    }
}
