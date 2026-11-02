use include_dir::{Dir, include_dir};
use std::fs;
use tracing::error;

/// A Data App (`.app.yml`). Data Apps are removed, so a new workspace is never given
/// one; this keeps a file that reappears in the embedded demo directory out of the copy.
fn is_data_app_file(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    name.ends_with(".app.yml") || name.ends_with(".app.yaml")
}

/// Outcome of a skip-if-exists demo copy. Paths are relative to the destination
/// root (e.g. `"config.yml"`, `"agents/duckdb.agent.yml"`).
#[derive(Debug, Default)]
pub struct DemoCopyResult {
    pub files_written: Vec<String>,
    pub files_skipped: Vec<String>,
    pub files_failed: Vec<(String, String)>,
}

/// Copy the embedded demo project into `target`, skipping any file that
/// already exists. Returns per-file outcomes; never returns `Err` for
/// individual file failures (those land in `files_failed`). Returns `Err`
/// only when the top-level directory cannot be created.
pub async fn copy_demo_files_to_with_skip(
    target: &std::path::Path,
) -> std::io::Result<DemoCopyResult> {
    tokio::fs::create_dir_all(target).await?;
    let mut result = DemoCopyResult::default();
    copy_embedded_dir_recursive_skip(&DEMO_DIST, target, std::path::Path::new(""), &mut result);
    Ok(result)
}

fn copy_embedded_dir_recursive_skip(
    src: &Dir<'static>,
    dst: &std::path::Path,
    rel_prefix: &std::path::Path,
    result: &mut DemoCopyResult,
) {
    if !dst.exists()
        && let Err(e) = fs::create_dir_all(dst)
    {
        error!("Failed to create directory {:?}: {}", dst, e);
        result
            .files_failed
            .push((rel_prefix.to_string_lossy().into_owned(), e.to_string()));
        return;
    }

    for entry in src.entries() {
        let Some(name) = entry.path().file_name() else {
            continue;
        };
        let dst_path = dst.join(name);
        let rel_path = rel_prefix.join(name);
        let rel_str = rel_path.to_string_lossy().into_owned();

        if let Some(file) = entry.as_file() {
            if is_data_app_file(name) {
                continue;
            }
            if dst_path.exists() {
                result.files_skipped.push(rel_str);
                continue;
            }
            match fs::write(&dst_path, file.contents()) {
                Ok(()) => result.files_written.push(rel_str),
                Err(e) => {
                    error!("Failed to write file {:?}: {}", dst_path, e);
                    result.files_failed.push((rel_str, e.to_string()));
                }
            }
        } else if let Some(dir) = entry.as_dir() {
            copy_embedded_dir_recursive_skip(dir, &dst_path, &rel_path, result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn the_airhouse_demo_points_local_at_airhouse_and_ships_no_dataset() {
        let tmp = TempDir::new().expect("tempdir");
        copy_airhouse_demo_files_to(tmp.path()).await.expect("copy");

        assert!(
            !tmp.path().join(DEMO_DATASET_DIR).exists(),
            "the dataset is loaded into Airhouse, not written to the workspace"
        );
        let config_text = std::fs::read_to_string(tmp.path().join("config.yml")).unwrap();
        let config: oxy::config::model::Config =
            serde_yaml::from_str(&config_text).expect("config.yml still parses");
        let local = config
            .databases
            .iter()
            .find(|d| d.name == "local")
            .expect("the database keeps its name");
        assert!(
            matches!(
                local.database_type,
                oxy::config::model::DatabaseType::AirhouseManaged(_)
            ),
            "local is {}",
            local.database_type
        );
        assert!(!config_text.contains("type: duckdb"));

        // Nothing reads the dataset by its file path any more.
        let mut stack = vec![tmp.path().to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(text) = std::fs::read_to_string(&path) {
                    assert!(
                        !text.contains("'oxymart.csv'"),
                        "{} still reads oxymart.csv as a file",
                        path.display()
                    );
                }
            }
        }
        let sql = std::fs::read_to_string(tmp.path().join("example_sql/total_number_of_store.sql"))
            .unwrap();
        assert!(sql.contains("FROM oxymart.csv"), "{sql}");

        // The views are the template's, byte for byte.
        let view =
            std::fs::read_to_string(tmp.path().join("sales_semantics/views/oxymart.view.yml"))
                .unwrap();
        let embedded = DEMO_DIST
            .get_file("sales_semantics/views/oxymart.view.yml")
            .unwrap()
            .contents_utf8()
            .unwrap();
        assert_eq!(view, embedded);
    }

    #[test]
    fn the_demo_dataset_is_embedded() {
        let files = demo_dataset_files();
        let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["oxymart.csv"]);
        assert!(files[0].1.starts_with(b"Store,Date,Weekly_Sales"));
    }

    #[tokio::test]
    async fn write_minimal_config_yml_creates_file_with_expected_contents() {
        let tmp = TempDir::new().expect("tempdir");
        write_minimal_config_yml(tmp.path())
            .await
            .expect("should write config.yml");
        let contents = std::fs::read_to_string(tmp.path().join("config.yml"))
            .expect("config.yml should exist");
        assert!(contents.contains("databases: []"));
        assert!(contents.contains("models: []"));
    }

    #[tokio::test]
    async fn write_minimal_config_yml_overwrites_existing_file() {
        // The helper is unconditional: callers (onboarding, local setup) are
        // responsible for the "only if missing" check. This test locks in
        // the unconditional behavior so a future refactor doesn't silently
        // change it.
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("config.yml"), "stale").expect("write stale");
        write_minimal_config_yml(tmp.path())
            .await
            .expect("should overwrite");
        let contents = std::fs::read_to_string(tmp.path().join("config.yml")).unwrap();
        assert!(contents.contains("databases: []"));
    }

    #[tokio::test]
    async fn copy_demo_files_to_with_skip_writes_all_files_into_empty_dir() {
        let tmp = TempDir::new().expect("tempdir");
        let result = copy_demo_files_to_with_skip(tmp.path())
            .await
            .expect("copy should succeed");
        assert!(
            result.files_written.iter().any(|p| p == "config.yml"),
            "config.yml should be in files_written, got {:?}",
            result.files_written
        );
        assert!(
            result.files_skipped.is_empty(),
            "nothing should be skipped in an empty dir"
        );
        assert!(
            result.files_failed.is_empty(),
            "nothing should fail in an empty dir"
        );
        assert!(tmp.path().join("config.yml").exists());
    }

    fn app_files_under(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if is_data_app_file(path.file_name().unwrap()) {
                    found.push(path);
                }
            }
        }
        found
    }

    #[tokio::test]
    async fn the_demo_workspace_is_not_given_a_data_app() {
        let skip = TempDir::new().expect("tempdir");
        copy_demo_files_to_with_skip(skip.path())
            .await
            .expect("copy");
        assert_eq!(
            app_files_under(skip.path()),
            Vec::<std::path::PathBuf>::new()
        );

        let plain = TempDir::new().expect("tempdir");
        copy_demo_files_to(plain.path()).await.expect("copy");
        assert_eq!(
            app_files_under(plain.path()),
            Vec::<std::path::PathBuf>::new()
        );
        assert!(plain.path().join("config.yml").exists());
    }

    #[tokio::test]
    async fn copy_demo_files_to_with_skip_skips_existing_files() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("config.yml"), "user's own").expect("seed file");

        let result = copy_demo_files_to_with_skip(tmp.path())
            .await
            .expect("copy should succeed");

        assert!(
            result.files_skipped.iter().any(|p| p == "config.yml"),
            "config.yml should be skipped, got skipped={:?}",
            result.files_skipped
        );
        assert!(
            !result.files_written.iter().any(|p| p == "config.yml"),
            "config.yml must not be in files_written when it was skipped"
        );
        let contents = std::fs::read_to_string(tmp.path().join("config.yml")).unwrap();
        assert_eq!(
            contents, "user's own",
            "pre-existing config.yml must be left untouched"
        );
    }
}

static DEMO_DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/demo_project");

/// Minimal contents written by `write_minimal_config_yml`. Shared between
/// the cloud onboarding `setup_new` handler and local-mode setup.
const MINIMAL_CONFIG_YML: &str = "# Oxygen workspace configuration\n# Add your databases and agents here.\n\ndatabases: []\nmodels: []\n";

/// Write a minimal `config.yml` into `dir`. Unconditional — callers check
/// existence if they need skip-if-exists behavior.
pub async fn write_minimal_config_yml(dir: &std::path::Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(dir.join("config.yml"), MINIMAL_CONFIG_YML).await
}

/// Copy the embedded demo project files into `target`, creating it if needed.
pub async fn copy_demo_files_to(target: &std::path::Path) -> Result<(), axum::http::StatusCode> {
    copy_embedded_dir_recursive(&DEMO_DIST, target).await
}

/// The demo's dataset directory, which a file DuckDB reads (`dataset: .db/`).
const DEMO_DATASET_DIR: &str = ".db";

/// The `config.yml` entry for the demo's database as the template ships it: a
/// file DuckDB over [`DEMO_DATASET_DIR`].
const DEMO_FILE_DATABASE: &str = "  - name: local\n    dataset: .db/\n    type: duckdb\n";

/// The same entry when the data lives in the workspace's Airhouse. Same name,
/// so the views (`datasource: "local"`), the agent and `defaults.database`
/// need no edit.
const DEMO_AIRHOUSE_DATABASE: &str = "  - name: local\n    type: airhouse_managed\n";

/// The demo's dataset files, as embedded in the binary: `(file name, bytes)`.
/// What a deployment loads into the workspace's Airhouse, so the bytes never
/// come from a checkout.
pub fn demo_dataset_files() -> Vec<(&'static str, &'static [u8])> {
    let Some(dir) = DEMO_DIST.get_dir(DEMO_DATASET_DIR) else {
        return Vec::new();
    };
    dir.files()
        .filter_map(|f| Some((f.path().file_name()?.to_str()?, f.contents())))
        .collect()
}

/// Copy the embedded demo project into `target` for a workspace whose data
/// lives in Airhouse, where the caller loads [`demo_dataset_files`].
///
/// Differs from [`copy_demo_files_to`] in three ways: the dataset directory is
/// not written; `config.yml`'s `local` database is `type: airhouse_managed`;
/// and SQL that reads a dataset file by its path (`FROM 'oxymart.csv'`, which
/// DuckDB treats as a file to open) names the table instead
/// (`FROM oxymart.csv`, which resolves to the `"oxymart"."csv"` view the load
/// creates). The semantic views already name it that way and are copied as-is.
pub async fn copy_airhouse_demo_files_to(target: &std::path::Path) -> std::io::Result<()> {
    let config = DEMO_DIST
        .get_file("config.yml")
        .and_then(|f| f.contents_utf8())
        .ok_or_else(|| std::io::Error::other("the embedded demo has no config.yml"))?;
    if !config.contains(DEMO_FILE_DATABASE) {
        return Err(std::io::Error::other(
            "the embedded demo's config.yml no longer declares `local` as a file DuckDB over \
             .db/, so it cannot be pointed at Airhouse",
        ));
    }
    let dataset_names: Vec<&str> = demo_dataset_files().iter().map(|(n, _)| *n).collect();
    copy_airhouse_demo_dir(&DEMO_DIST, target, &dataset_names)
}

fn copy_airhouse_demo_dir(
    src: &Dir<'static>,
    dst: &std::path::Path,
    dataset_names: &[&str],
) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in src.entries() {
        let Some(name) = entry.path().file_name() else {
            continue;
        };
        let dst_path = dst.join(name);
        if let Some(dir) = entry.as_dir() {
            if dir.path() == std::path::Path::new(DEMO_DATASET_DIR) {
                continue;
            }
            copy_airhouse_demo_dir(dir, &dst_path, dataset_names)?;
        } else if let Some(file) = entry.as_file() {
            if is_data_app_file(name) {
                continue;
            }
            let rel = file.path();
            let contents = match file.contents_utf8() {
                Some(text) if rel == std::path::Path::new("config.yml") => {
                    text.replace(DEMO_FILE_DATABASE, DEMO_AIRHOUSE_DATABASE)
                }
                Some(text) if reads_dataset_files_by_path(rel) => {
                    let mut text = text.to_string();
                    for dataset in dataset_names {
                        text = text.replace(&format!("'{dataset}'"), dataset);
                    }
                    text
                }
                _ => {
                    fs::write(&dst_path, file.contents())?;
                    continue;
                }
            };
            fs::write(&dst_path, contents)?;
        }
    }
    Ok(())
}

/// The demo files whose SQL names a dataset file by its path: example SQL and
/// automations. Views name the table, never the path.
fn reads_dataset_files_by_path(rel: &std::path::Path) -> bool {
    let name = rel.to_string_lossy();
    name.ends_with(".sql") || name.ends_with(".automation.yml")
}

async fn copy_embedded_dir_recursive(
    src: &Dir<'static>,
    dst: &std::path::Path,
) -> Result<(), axum::http::StatusCode> {
    if !dst.exists() {
        fs::create_dir_all(dst).map_err(|e| {
            error!("Failed to create directory {:?}: {}", dst, e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    for entry in src.entries() {
        let name = entry.path().file_name().ok_or_else(|| {
            error!("Failed to get file name from path: {:?}", entry.path());
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

        let dst_path = dst.join(name);

        if let Some(file) = entry.as_file() {
            if is_data_app_file(name) {
                continue;
            }
            let content = file.contents();
            fs::write(&dst_path, content).map_err(|e| {
                error!("Failed to write file {:?}: {}", dst_path, e);
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            })?;
        } else if let Some(dir) = entry.as_dir() {
            Box::pin(copy_embedded_dir_recursive(dir, &dst_path)).await?;
        }
    }

    Ok(())
}
