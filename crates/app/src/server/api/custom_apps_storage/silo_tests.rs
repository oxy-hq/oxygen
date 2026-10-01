//! An environment's storage silo on the filesystem backend: writes land in the
//! sibling `customer-app-storage/<id>~<env>/`, reads fall back to production's
//! same key read-only, `delete` and `copy` never touch production's silo, and
//! metering and app deletion see every silo (environments design §4.2, §4.3).
//!
//! Each test points `OXY_STATE_DIR` at its own scratch directory; nextest runs
//! every test in its own process.

use std::path::PathBuf;

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::*;

fn use_temp_state_dir() -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("oxy-silo-{}", Uuid::new_v4()));
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::remove_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET");
        std::env::set_var("OXY_STATE_DIR", &tmp);
    }
    tmp
}

fn silos() -> (Silo, Silo) {
    let app = Uuid::new_v4();
    (
        Silo::production(app),
        Silo::for_environment(app, &AppEnvironment::Staging),
    )
}

async fn write(silo: &Silo, pathname: &str, body: &str) -> String {
    put(
        silo,
        pathname,
        body.as_bytes().to_vec(),
        PutOptions::default(),
        &RetentionPolicy::default(),
    )
    .await
    .expect("put")
    .key
}

async fn body(silo: &Silo, key: &str) -> Option<String> {
    get(silo, key)
        .await
        .expect("get")
        .map(|(bytes, _)| String::from_utf8(bytes).expect("utf8"))
}

async fn keys(silo: &Silo) -> Vec<String> {
    list(silo, None, None, None)
        .await
        .expect("list")
        .objects
        .into_iter()
        .map(|o| o.key)
        .collect()
}

#[tokio::test]
async fn staging_writes_its_own_silo_and_reads_production_through_it() {
    let tmp = use_temp_state_dir();
    let (production, staging) = silos();
    let production_key = write(&production, "generated/report.csv", "prod").await;

    // Staging reads production's object by its production key, or by the
    // same pathname in its own silo — no copy step.
    assert_eq!(
        body(&staging, &production_key).await.as_deref(),
        Some("prod")
    );
    let staged_path = staging.key("generated/report.csv");
    assert_eq!(body(&staging, &staged_path).await.as_deref(), Some("prod"));
    let meta = head(&staging, &production_key).await.unwrap().unwrap();
    assert_eq!(meta.key, production_key, "head says which silo answered");

    // Without allowOverwrite the key already exists, as a read says it does.
    let refused = put(
        &staging,
        &production_key,
        b"stg".to_vec(),
        PutOptions::default(),
        &RetentionPolicy::default(),
    )
    .await
    .expect_err("production's key reads as existing");
    assert!(
        matches!(refused, StorageError::AlreadyExists(_)),
        "{refused}"
    );
    assert!(
        copy(&staging, &production_key, &production_key, false)
            .await
            .is_err(),
        "a copy onto it is refused alike"
    );

    // With allowOverwrite, a staging write of the same pathname — even named
    // by production's key — lands in staging's silo and shadows it there.
    let staging_key = put(
        &staging,
        &production_key,
        b"stg".to_vec(),
        PutOptions {
            allow_overwrite: true,
            ..Default::default()
        },
        &RetentionPolicy::default(),
    )
    .await
    .expect("put")
    .key;
    assert_eq!(staging_key, staged_path);
    assert_eq!(
        body(&staging, &production_key).await.as_deref(),
        Some("stg")
    );
    assert_eq!(
        body(&production, &production_key).await.as_deref(),
        Some("prod"),
        "production's object is untouched"
    );
    assert!(
        get(&production, &staging_key).await.is_err(),
        "production can never name a staging key"
    );
    let _ = std::fs::remove_dir_all(tmp);
}

#[tokio::test]
async fn a_staging_delete_or_copy_never_reaches_productions_silo() {
    let tmp = use_temp_state_dir();
    let (production, staging) = silos();
    let production_key = write(&production, "uploads/scan.pdf", "prod").await;

    // Deleting production's key from staging deletes staging's copy of that
    // pathname (there is none) — never production's object.
    let deleted = delete(&staging, std::slice::from_ref(&production_key))
        .await
        .expect("delete");
    assert_eq!(deleted, 1, "accepted, like any idempotent delete");
    assert!(head(&production, &production_key).await.unwrap().is_some());

    // A copy reads production's object and writes staging's silo, whatever
    // silo the destination names.
    let copied = copy(
        &staging,
        &production_key,
        &production_key.replace("scan", "copy"),
        false,
    )
    .await
    .expect("copy");
    assert!(copied.key.starts_with(&staging.prefix()), "{}", copied.key);
    assert_eq!(keys(&production).await, vec![production_key.clone()]);
    assert_eq!(keys(&staging).await, vec![copied.key.clone()]);

    // And staging deleting its own copy leaves production's alone.
    delete(&staging, &[copied.key.clone(), production_key.clone()])
        .await
        .unwrap();
    assert!(keys(&staging).await.is_empty());
    assert_eq!(keys(&production).await, vec![production_key]);
    let _ = std::fs::remove_dir_all(tmp);
}

#[tokio::test]
async fn a_staging_listing_is_its_own_silo_alone() {
    let tmp = use_temp_state_dir();
    let (production, staging) = silos();
    write(&production, "generated/a.csv", "prod").await;
    assert!(
        keys(&staging).await.is_empty(),
        "never merged with production"
    );
    let staged = write(&staging, "generated/b.csv", "stg").await;
    assert_eq!(keys(&staging).await, vec![staged.clone()]);
    let sub = list(&staging, Some("generated"), None, None).await.unwrap();
    assert_eq!(sub.objects.len(), 1);
    assert!(!keys(&production).await.contains(&staged));
    let _ = std::fs::remove_dir_all(tmp);
}

#[tokio::test]
async fn metering_and_app_deletion_cover_every_silo() {
    let tmp = use_temp_state_dir();
    let (production, staging) = silos();
    let other = Silo::production(Uuid::new_v4());
    write(&production, "uploads/a.bin", "1234").await;
    write(&staging, "uploads/b.bin", "12").await;
    write(&other, "uploads/c.bin", "x").await;

    let m = usage::measure_app(production.app_id(), &RetentionPolicy::default()).await;
    assert!(m.is_exact(), "{:?}", m.detail);
    assert_eq!(
        (m.bytes, m.object_count),
        (6, 2),
        "staging's bytes are the app's"
    );
    assert_eq!(m.prefix_breakdown["uploads/"].bytes, 4);
    assert_eq!(m.prefix_breakdown["~staging/uploads/"].bytes, 2);

    delete_app_assets(production.app_id())
        .await
        .expect("delete app");
    assert!(keys(&production).await.is_empty());
    assert!(
        keys(&staging).await.is_empty(),
        "the staging silo goes with the app"
    );
    assert_eq!(keys(&other).await.len(), 1, "another app is untouched");
    let _ = std::fs::remove_dir_all(tmp);
}

/// An app's first `storage.retention` rule changes how its uploads are signed
/// (`x-amz-tagging` is bound into the signature). A staging key is tagged —
/// and so signed — exactly as production's same pathname.
#[test]
fn retention_tags_an_environment_key_by_its_relative_path() {
    let (production, staging) = silos();
    let rules = [RetentionRule {
        prefix: "uploads/".to_string(),
        expire_after: Some("30d".to_string()),
    }];
    let (policy, _) = RetentionPolicy::from_rules(&rules);
    let prod_key = normalize_in(&production, "uploads/a.pdf", false).unwrap();
    let staging_key = normalize_in(&staging, "uploads/a.pdf", false).unwrap();
    let tag = retention_tag_in(&production, &prod_key, &policy);
    assert_eq!(tag.as_deref(), Some("oxy-ttl=30d"));
    assert_eq!(retention_tag_in(&staging, &staging_key, &policy), tag);
    assert_eq!(
        retention_tag_in(&production, &staging_key, &policy),
        None,
        "a key of another silo is never stamped with this one's class"
    );
}

/// The presigned upload itself, signed offline against a fake endpoint: the
/// staging URL names the staging key and carries production's tagging.
#[tokio::test]
async fn a_staging_upload_url_signs_its_own_key_with_the_same_tagging() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET", "silo-test");
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("AWS_REGION", "us-east-1");
        std::env::set_var("AWS_ENDPOINT_URL", "http://127.0.0.1:9");
        std::env::set_var("AWS_CONFIG_FILE", "/nonexistent/oxy-silo-test");
        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", "/nonexistent/oxy-silo-test");
    }
    let (production, staging) = silos();
    let rules = [RetentionRule {
        prefix: "uploads/".to_string(),
        expire_after: Some("30d".to_string()),
    }];
    let (policy, _) = RetentionPolicy::from_rules(&rules);
    let sign = |silo: Silo| {
        let policy = policy.clone();
        async move {
            get_upload_url(&silo, "uploads/a.pdf", "application/pdf", 10, None, &policy)
                .await
                .expect("presign")
        }
    };
    let prod = sign(production).await;
    let stg = sign(staging.clone()).await;
    assert!(stg.key.starts_with(&staging.prefix()), "{}", stg.key);
    assert!(stg.url.contains(&stg.key), "{}", stg.url);
    assert_eq!(stg.tagging, prod.tagging);
    assert_eq!(stg.tagging.as_deref(), Some("oxy-ttl=30d"));
}
