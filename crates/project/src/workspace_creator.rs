use include_dir::{Dir, include_dir};
use std::fs;
use tracing::error;

/// A Data App (`.app.yml`). Data Apps are retired, so a new workspace is never
/// given one, even though `demo_project/` still carries three as fixtures for
/// the tests that view and run existing apps.
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

#[cfg(target_os = "windows")]
static DEMO_DIST: Dir = include_dir!("D:\\a\\oxy\\oxy\\crates\\core\\demo_project");
#[cfg(not(target_os = "windows"))]
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
