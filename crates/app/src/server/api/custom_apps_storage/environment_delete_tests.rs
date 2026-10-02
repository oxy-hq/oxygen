//! Removing one environment's storage silo — a sandbox's teardown
//! (`custom_apps_sandboxes::teardown`) — on the filesystem backend: exactly
//! that silo goes, production's is refused, and a second call is a no-op.
//!
//! Each test points `OXY_STATE_DIR` at its own scratch directory; nextest runs
//! every test in its own process.

use std::path::PathBuf;

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::*;

fn use_temp_state_dir() -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("oxy-env-delete-{}", Uuid::new_v4()));
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::remove_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET");
        std::env::set_var("OXY_STATE_DIR", &tmp);
    }
    tmp
}

fn sandbox(handle: &str) -> AppEnvironment {
    AppEnvironment::Dev {
        handle: handle.into(),
    }
}

async fn write(silo: &Silo, pathname: &str) -> String {
    put(
        silo,
        pathname,
        b"x".to_vec(),
        PutOptions::default(),
        &RetentionPolicy::default(),
    )
    .await
    .expect("put")
    .key
}

async fn count(silo: &Silo) -> usize {
    list(silo, None, None, None)
        .await
        .expect("list")
        .objects
        .len()
}

/// One sandbox's silo goes; production's, staging's, a sandbox whose handle
/// this one's is a prefix of, and another app's same-named sandbox all stay.
#[tokio::test]
async fn deleting_an_environments_assets_removes_that_silo_alone() {
    let tmp = use_temp_state_dir();
    let app = Uuid::new_v4();
    let mine = Silo::for_environment(app, &sandbox("a1"));
    let kept = [
        Silo::production(app),
        Silo::for_environment(app, &AppEnvironment::Staging),
        Silo::for_environment(app, &sandbox("a1-b")),
        Silo::for_environment(Uuid::new_v4(), &sandbox("a1")),
    ];
    write(&mine, "uploads/a.bin").await;
    write(&mine, "generated/deep/b.bin").await;
    // A pathname each: an environment's write refuses to shadow a key
    // production already holds.
    for (i, silo) in kept.iter().enumerate() {
        write(silo, &format!("uploads/kept-{i}.bin")).await;
    }
    assert_eq!(count(&mine).await, 2);

    delete_environment_assets(app, &sandbox("a1"))
        .await
        .expect("delete the sandbox's silo");

    assert_eq!(count(&mine).await, 0);
    for silo in &kept {
        assert_eq!(count(silo).await, 1, "{} is untouched", silo.prefix());
    }
    // Idempotent: the teardown task may run twice.
    delete_environment_assets(app, &sandbox("a1"))
        .await
        .expect("a second delete is a no-op");
    // …and one that never wrote anything has nothing to remove.
    delete_environment_assets(app, &sandbox("never"))
        .await
        .expect("no silo on disk");
    let _ = std::fs::remove_dir_all(tmp);
}

/// Production's silo is the app's, removed only with the app
/// (`delete_app_assets`): naming it here is refused and deletes nothing.
#[tokio::test]
async fn deleting_productions_assets_as_an_environment_is_refused() {
    let tmp = use_temp_state_dir();
    let app = Uuid::new_v4();
    let production = Silo::production(app);
    write(&production, "uploads/a.bin").await;

    let err = delete_environment_assets(app, &AppEnvironment::Production)
        .await
        .expect_err("production is not an environment silo");
    assert!(matches!(err, StorageError::Denied(_)), "{err}");
    assert_eq!(count(&production).await, 1, "nothing was deleted");
    let _ = std::fs::remove_dir_all(tmp);
}

/// Staging's silo is not a sandbox's: nothing tears staging down, so naming
/// it here is refused and deletes nothing — whatever the caller checked.
#[tokio::test]
async fn deleting_stagings_assets_as_an_environment_is_refused() {
    let tmp = use_temp_state_dir();
    let app = Uuid::new_v4();
    let staging = Silo::for_environment(app, &AppEnvironment::Staging);
    write(&staging, "uploads/a.bin").await;

    let err = delete_environment_assets(app, &AppEnvironment::Staging)
        .await
        .expect_err("staging is not a sandbox");
    assert!(matches!(err, StorageError::Denied(_)), "{err}");
    assert_eq!(count(&staging).await, 1, "nothing was deleted");
    let _ = std::fs::remove_dir_all(tmp);
}
