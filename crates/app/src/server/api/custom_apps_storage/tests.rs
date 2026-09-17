//! Tests for the custom-app asset store.
//!
//! Two layers: the pure key/pathname logic (which is the tenant-isolation
//! boundary, so it gets the most attention), and a full lifecycle against the
//! filesystem backend — put/get/head/list/copy/delete — which exercises the same
//! code paths an app hits, minus presigning (S3-only by nature).

use super::*;

fn app() -> Uuid {
    Uuid::from_u128(7)
}
fn other() -> Uuid {
    Uuid::from_u128(8)
}

/// The default for every pre-existing test: no retention policy, so nothing is
/// tagged and nothing expires. Retention-specific behaviour is asserted in the
/// `retention_tagging` block below and in `retention.rs`'s own unit tests.
fn no_retention() -> RetentionPolicy {
    RetentionPolicy::default()
}

// ── Pathname normalization ────────────────────────────────────────────────────

#[test]
fn normalize_places_pathname_inside_the_app_silo() {
    let key = normalize_pathname(app(), "generated/report.pdf", false).unwrap();
    assert_eq!(key, format!("{}generated/report.pdf", app_prefix(app())));
    // Nesting is preserved — apps organize their own assets.
    let nested = normalize_pathname(app(), "generated/2026/q1/summary.csv", false).unwrap();
    assert!(
        nested.ends_with("generated/2026/q1/summary.csv"),
        "{nested}"
    );
}

#[test]
fn normalize_is_idempotent_for_an_already_prefixed_key() {
    // A key round-tripped out of list()/put() must be usable as an input.
    let once = normalize_pathname(app(), "generated/a.pdf", false).unwrap();
    let twice = normalize_pathname(app(), &once, false).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn normalize_strips_traversal_and_unsafe_characters() {
    // `..` segments are sanitized away, never resolved — the key cannot escape.
    let key = normalize_pathname(app(), "../../etc/passwd", false).unwrap();
    assert!(key.starts_with(&app_prefix(app())), "{key}");
    assert!(!key.contains(".."), "{key}");
    // Another app's prefix, injected as a literal, still lands in MY silo. Only
    // the *caller's own* prefix is stripped for idempotency, so a foreign prefix
    // survives as ordinary path segments — harmless, because what matters is
    // where the key resolves, not what it spells.
    let injected = normalize_pathname(
        app(),
        &format!("../{}secret.txt", app_prefix(other())),
        false,
    )
    .unwrap();
    assert!(injected.starts_with(&app_prefix(app())), "{injected}");
    // The property that actually matters: the victim app cannot reach it.
    assert!(
        matches!(
            validate_key(other(), &injected),
            Err(StorageError::Denied(_))
        ),
        "a key spelling another app's prefix must still be unreachable BY that app: {injected}"
    );
    // ...and it is reachable by its real owner.
    assert!(validate_key(app(), &injected).is_ok());
    // Spaces/glob/control characters collapse to underscores.
    let messy = normalize_pathname(app(), "my report*?.csv", false).unwrap();
    assert!(messy.ends_with("my_report__.csv"), "{messy}");
}

#[test]
fn normalize_rejects_empty_and_segment_less_pathnames() {
    assert!(matches!(
        normalize_pathname(app(), "", false),
        Err(StorageError::Invalid(_))
    ));
    assert!(matches!(
        normalize_pathname(app(), "///", false),
        Err(StorageError::Invalid(_))
    ));
    // A pathname of only dots sanitizes to nothing rather than to a hidden file.
    assert!(matches!(
        normalize_pathname(app(), "...", false),
        Err(StorageError::Invalid(_))
    ));
}

#[test]
fn random_suffix_keeps_the_extension_last() {
    // Content-type sniffing keys off the extension, so the suffix goes before it.
    let key = normalize_pathname(app(), "uploads/report.pdf", true).unwrap();
    assert!(key.ends_with(".pdf"), "{key}");
    assert!(key.contains("report-"), "{key}");
    // Two calls never collide.
    let a = normalize_pathname(app(), "uploads/report.pdf", true).unwrap();
    let b = normalize_pathname(app(), "uploads/report.pdf", true).unwrap();
    assert_ne!(a, b);
    // Extension-less names still get a suffix.
    let bare = normalize_pathname(app(), "uploads/README", true).unwrap();
    assert!(bare.contains("README-"), "{bare}");
}

// ── Tenant isolation ──────────────────────────────────────────────────────────

#[test]
fn validate_key_blocks_cross_tenant_access() {
    let mine = format!("{}uploads/x/report.pdf", app_prefix(app()));
    assert!(validate_key(app(), &mine).is_ok());

    // Another app's silo — the case that matters most.
    let theirs = format!("{}uploads/x/report.pdf", app_prefix(other()));
    assert!(matches!(
        validate_key(app(), &theirs),
        Err(StorageError::Denied(_))
    ));

    // Traversal out of my own prefix.
    let sneaky = format!("{}../{}x", app_prefix(app()), app_prefix(other()));
    assert!(matches!(
        validate_key(app(), &sneaky),
        Err(StorageError::Denied(_))
    ));

    // An unprefixed key, and a bare prefix naming no object.
    assert!(matches!(
        validate_key(app(), "report.pdf"),
        Err(StorageError::Denied(_))
    ));
    assert!(matches!(
        validate_key(app(), &app_prefix(app())),
        Err(StorageError::Invalid(_))
    ));
}

#[test]
fn list_prefix_is_confined_to_the_silo() {
    assert_eq!(resolve_list_prefix(app(), None).unwrap(), app_prefix(app()));
    assert_eq!(
        resolve_list_prefix(app(), Some("generated")).unwrap(),
        format!("{}generated", app_prefix(app()))
    );
    // Already-prefixed input isn't double-prefixed.
    let full = format!("{}uploads", app_prefix(app()));
    assert_eq!(resolve_list_prefix(app(), Some(&full)).unwrap(), full);
    assert!(matches!(
        resolve_list_prefix(app(), Some("../secrets")),
        Err(StorageError::Denied(_))
    ));
}

// ── Content types & TTL policy ────────────────────────────────────────────────

#[test]
fn content_type_is_inferred_for_generated_assets() {
    assert_eq!(guess_content_type("a/b/report.pdf"), "application/pdf");
    assert_eq!(guess_content_type("export.csv"), "text/csv");
    assert_eq!(guess_content_type("chart.PNG"), "image/png");
    assert_eq!(
        guess_content_type("data.parquet"),
        "application/vnd.apache.parquet"
    );
    assert_eq!(guess_content_type("noext"), "application/octet-stream");
}

#[test]
fn presign_ttl_defaults_and_clamps() {
    assert_eq!(
        presign_ttl(None, DEFAULT_UPLOAD_TTL_SECS).as_secs(),
        DEFAULT_UPLOAD_TTL_SECS
    );
    assert_eq!(presign_ttl(Some(60), DEFAULT_UPLOAD_TTL_SECS).as_secs(), 60);
    // Zero is meaningless — fall back to the default rather than sign a dead URL.
    assert_eq!(
        presign_ttl(Some(0), DEFAULT_DOWNLOAD_TTL_SECS).as_secs(),
        DEFAULT_DOWNLOAD_TTL_SECS
    );
    // Clamped to SigV4's own 7-day maximum, above which signing simply fails.
    assert_eq!(
        presign_ttl(Some(999_999_999), DEFAULT_DOWNLOAD_TTL_SECS).as_secs(),
        MAX_PRESIGN_TTL_SECS
    );
    // (That a download link outlives an upload link is asserted at compile time
    // next to the constants in mod.rs — a download link gets emailed to a human.)
}

// ── Size ceilings ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn put_rejects_an_oversized_inline_blob() {
    let big = vec![0u8; INLINE_BLOB_MAX_BYTES + 1];
    let err = put(
        app(),
        "generated/big.bin",
        big,
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StorageError::TooLarge(_)));
    // The message must point at the escape hatch, not just refuse.
    assert!(format!("{err}").contains("presigned"), "{err}");
}

#[tokio::test]
async fn upload_url_validates_content_length() {
    // Zero-length is a client bug, not a zero-byte upload.
    assert!(matches!(
        get_upload_url(
            app(),
            "uploads/x.pdf",
            "application/pdf",
            0,
            None,
            &no_retention()
        )
        .await,
        Err(StorageError::Invalid(_))
    ));
    // Over the ceiling is refused before any signing happens.
    let over = max_upload_bytes() + 1;
    assert!(matches!(
        get_upload_url(
            app(),
            "uploads/x.pdf",
            "application/pdf",
            over,
            None,
            &no_retention()
        )
        .await,
        Err(StorageError::TooLarge(_))
    ));
}

#[tokio::test]
async fn presigning_without_a_bucket_is_a_clear_error() {
    // SAFETY: single-threaded test.
    unsafe {
        std::env::remove_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET");
    }
    let err = get_upload_url(
        app(),
        "uploads/x.pdf",
        "application/pdf",
        10,
        None,
        &no_retention(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StorageError::NotConfigured(_)));
    let err = get_download_url(app(), &format!("{}a.pdf", app_prefix(app())), None, false)
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::NotConfigured(_)));
}

// ── Full lifecycle on the filesystem backend ──────────────────────────────────

/// Point the store at a scratch state dir with no bucket configured, so the
/// filesystem backend is selected.
fn use_temp_state_dir() -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("oxy-storage-{}", Uuid::new_v4()));
    // SAFETY: these tests are single-threaded (serialized by the shared env).
    unsafe {
        std::env::remove_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET");
        std::env::set_var("OXY_STATE_DIR", &tmp);
    }
    tmp
}

use std::path::PathBuf;

#[tokio::test]
async fn generated_asset_lifecycle_on_fs() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();

    // A function writes a generated CSV. Content type is inferred.
    let put_res = put(
        a,
        "generated/jan.csv",
        b"a,b\n1,2\n".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .expect("put");
    assert_eq!(put_res.content_type, "text/csv");
    assert_eq!(put_res.size, 8);
    assert!(put_res.key.starts_with(&app_prefix(a)));

    // Read it back.
    let got = get(a, &put_res.key).await.expect("get").expect("present");
    assert_eq!(got.0, b"a,b\n1,2\n");

    // Metadata without the body.
    let meta = head(a, &put_res.key).await.expect("head").expect("present");
    assert_eq!(meta.size, 8);
    assert_eq!(meta.content_type.as_deref(), Some("text/csv"));

    // Listing finds it.
    let page = list(a, None, None, None).await.expect("list");
    assert!(page.objects.iter().any(|o| o.key == put_res.key));
    assert!(!page.has_more);

    // Copy, then confirm both exist.
    let copied = copy(a, &put_res.key, "generated/jan-copy.csv", false)
        .await
        .expect("copy");
    assert!(head(a, &copied.key).await.unwrap().is_some());
    assert!(head(a, &put_res.key).await.unwrap().is_some());

    // Delete both; deleting again is idempotent (an absent key still counts as
    // accepted, matching S3), not an error.
    let n = delete(a, &[put_res.key.clone(), copied.key.clone()])
        .await
        .expect("delete");
    assert_eq!(n, 2);
    assert!(head(a, &put_res.key).await.unwrap().is_none());
    assert_eq!(
        delete(a, std::slice::from_ref(&put_res.key)).await.unwrap(),
        1,
        "absent key is accepted for deletion (idempotent), so it still counts"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn binary_generated_assets_round_trip_byte_for_byte() {
    // The whole point of taking raw bytes: a generated PDF/PNG must survive.
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    // Bytes that are not valid UTF-8 — a text-only store would corrupt these.
    let binary: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x00, 0xFF, 0xFE, 0x80, 0x01];
    let res = put(
        a,
        "generated/chart.png",
        binary.clone(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .expect("put binary");
    assert_eq!(res.content_type, "image/png");
    let got = get(a, &res.key).await.expect("get").expect("present");
    assert_eq!(got.0, binary, "binary asset must round-trip unchanged");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn put_refuses_to_clobber_unless_told_to() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    let first = put(
        a,
        "generated/r.txt",
        b"one".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .expect("first put");

    // Default is create-only: silently losing an asset is worse than an error.
    let err = put(
        a,
        "generated/r.txt",
        b"two".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StorageError::AlreadyExists(_)));
    assert_eq!(
        get(a, &first.key).await.unwrap().unwrap().0,
        b"one",
        "the original must be untouched"
    );

    // Explicit opt-in replaces it.
    put(
        a,
        "generated/r.txt",
        b"two".to_vec(),
        PutOptions {
            allow_overwrite: true,
            ..Default::default()
        },
        &no_retention(),
    )
    .await
    .expect("overwrite");
    assert_eq!(get(a, &first.key).await.unwrap().unwrap().0, b"two");

    // ...and a random suffix stores alongside instead of replacing.
    let side = put(
        a,
        "generated/r.txt",
        b"three".to_vec(),
        PutOptions {
            add_random_suffix: true,
            ..Default::default()
        },
        &no_retention(),
    )
    .await
    .expect("suffixed");
    assert_ne!(side.key, first.key);
    assert_eq!(get(a, &first.key).await.unwrap().unwrap().0, b"two");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn copy_refuses_to_clobber_and_rejects_self() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    let src = put(
        a,
        "generated/a.txt",
        b"src".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .expect("put src");
    put(
        a,
        "generated/b.txt",
        b"dst".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .expect("put dst");

    let err = copy(a, &src.key, "generated/b.txt", false)
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::AlreadyExists(_)));
    // Overwrite is opt-in.
    copy(a, &src.key, "generated/b.txt", true)
        .await
        .expect("copy over");

    let err = copy(a, &src.key, "generated/a.txt", false)
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::Invalid(_)));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn list_paginates_with_a_cursor() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    for i in 0..7 {
        put(
            a,
            &format!("generated/f{i:02}.txt"),
            vec![b'x'; i + 1],
            PutOptions::default(),
            &no_retention(),
        )
        .await
        .expect("put");
    }

    let first = list(a, None, Some(3), None).await.expect("page 1");
    assert_eq!(first.objects.len(), 3);
    assert!(first.has_more);
    let cursor = first.cursor.clone().expect("cursor when more remain");

    let second = list(a, None, Some(3), Some(cursor)).await.expect("page 2");
    assert_eq!(second.objects.len(), 3);
    assert!(second.has_more);

    let third = list(a, None, Some(3), second.cursor.clone())
        .await
        .expect("page 3");
    assert_eq!(third.objects.len(), 1);
    assert!(!third.has_more);
    assert!(third.cursor.is_none());

    // Pages are disjoint and cover everything exactly once.
    let mut seen: Vec<String> = first
        .objects
        .iter()
        .chain(&second.objects)
        .chain(&third.objects)
        .map(|o| o.key.clone())
        .collect();
    let total = seen.len();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 7, "expected 7 distinct keys");
    assert_eq!(total, 7, "pages must not overlap");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn list_is_scoped_by_prefix_and_never_leaks_another_app() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    put(
        a,
        "generated/mine.txt",
        b"a".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .unwrap();
    put(
        a,
        "uploads/mine.txt",
        b"a".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .unwrap();
    put(
        b,
        "generated/theirs.txt",
        b"b".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .unwrap();

    // Sub-prefix narrows within the app.
    let generated = list(a, Some("generated"), None, None).await.unwrap();
    assert_eq!(generated.objects.len(), 1);
    assert!(generated.objects[0].key.ends_with("generated/mine.txt"));

    // The whole silo is exactly this app's two objects — never app b's.
    let all = list(a, None, None, None).await.unwrap();
    assert_eq!(all.objects.len(), 2);
    assert!(
        all.objects
            .iter()
            .all(|o| o.key.starts_with(&app_prefix(a))),
        "listing leaked outside the app silo: {:?}",
        all.objects
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn cross_tenant_reads_and_writes_are_denied_before_touching_disk() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let asset = put(
        a,
        "generated/secret.txt",
        b"top".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .expect("put");

    // App b holds a's key (leaked, guessed, whatever) — every path refuses it.
    assert!(matches!(
        get(b, &asset.key).await,
        Err(StorageError::Denied(_))
    ));
    assert!(matches!(
        head(b, &asset.key).await,
        Err(StorageError::Denied(_))
    ));
    assert!(matches!(
        delete(b, std::slice::from_ref(&asset.key)).await,
        Err(StorageError::Denied(_))
    ));
    assert!(matches!(
        copy(b, &asset.key, "generated/stolen.txt", false).await,
        Err(StorageError::Denied(_))
    ));

    // ...and a's asset is still intact.
    assert_eq!(get(a, &asset.key).await.unwrap().unwrap().0, b"top");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn missing_objects_read_as_none_not_errors() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    let absent = format!("{}generated/nope.txt", app_prefix(a));
    assert!(get(a, &absent).await.unwrap().is_none());
    assert!(head(a, &absent).await.unwrap().is_none());
    // Listing an empty silo is an empty page, not a failure.
    let page = list(a, None, None, None).await.unwrap();
    assert!(page.objects.is_empty());
    assert!(!page.has_more);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn delete_app_assets_removes_the_whole_silo() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mine = put(
        a,
        "generated/x.txt",
        b"x".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .unwrap();
    let theirs = put(
        b,
        "generated/y.txt",
        b"y".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .unwrap();

    delete_app_assets(a).await.expect("delete app assets");
    assert!(head(a, &mine.key).await.unwrap().is_none());
    // A neighbouring app is untouched.
    assert!(head(b, &theirs.key).await.unwrap().is_some());
    // Idempotent.
    delete_app_assets(a)
        .await
        .expect("second delete is a no-op");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn delete_rejects_an_unbounded_batch() {
    let a = Uuid::new_v4();
    let keys: Vec<String> = (0..(MAX_LIST_LIMIT + 1))
        .map(|i| format!("{}generated/f{i}.txt", app_prefix(a)))
        .collect();
    assert!(matches!(
        delete(a, &keys).await,
        Err(StorageError::Invalid(_))
    ));
    assert_eq!(delete(a, &[]).await.unwrap(), 0);
}

// ── Retention tagging ────────────────────────────────────────────────────────
//
// `retention.rs` unit-tests class parsing and prefix matching in isolation.
// What these cover is the seam between the two: turning an app's policy plus a
// *full silo key* into the tag value actually bound into the S3 request. That
// seam is where a silent failure lives — strip the silo prefix wrongly and every
// object quietly stops expiring, with no error anywhere.

fn policy(rules: &[(&str, Option<&str>)]) -> RetentionPolicy {
    let rules: Vec<RetentionRule> = rules
        .iter()
        .map(|(prefix, expire_after)| RetentionRule {
            prefix: (*prefix).to_string(),
            expire_after: expire_after.map(str::to_string),
        })
        .collect();
    let (policy, warnings) = RetentionPolicy::from_rules(&rules);
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    policy
}

#[test]
fn retention_tag_matches_against_the_silo_relative_path() {
    let p = policy(&[("generated/", Some("90d"))]);
    // The key carries the silo prefix; the policy does not. If the prefix isn't
    // stripped before matching, this returns None and nothing ever expires.
    let key = normalize_pathname(app(), "generated/q1.pdf", false).unwrap();
    assert!(key.starts_with(&app_prefix(app())), "precondition: {key}");
    assert_eq!(
        retention_tag(app(), &key, &p).as_deref(),
        Some("oxy-ttl=90d")
    );
}

#[test]
fn retention_tag_is_absent_without_a_policy() {
    let key = normalize_pathname(app(), "generated/q1.pdf", false).unwrap();
    assert_eq!(retention_tag(app(), &key, &no_retention()), None);
}

#[test]
fn retention_tag_is_absent_for_an_unmatched_prefix() {
    let p = policy(&[("tmp/", Some("1d"))]);
    let key = normalize_pathname(app(), "uploads/invoice.pdf", false).unwrap();
    // Fail open: an unmatched key keeps its bytes forever.
    assert_eq!(retention_tag(app(), &key, &p), None);
}

#[test]
fn retention_tag_refuses_a_key_from_another_apps_silo() {
    // Belt-and-braces against the tenant boundary: even if a foreign key reached
    // here, it must not be stamped with THIS app's policy.
    let p = policy(&[("generated/", Some("30d"))]);
    let foreign = normalize_pathname(other(), "generated/q1.pdf", false).unwrap();
    assert_eq!(retention_tag(app(), &foreign, &p), None);
}

#[test]
fn retention_tag_honors_longest_prefix_through_the_full_key() {
    let p = policy(&[("generated/", Some("90d")), ("generated/tmp/", Some("1d"))]);
    let long = normalize_pathname(app(), "generated/tmp/scratch.csv", false).unwrap();
    let short = normalize_pathname(app(), "generated/keep.pdf", false).unwrap();
    assert_eq!(
        retention_tag(app(), &long, &p).as_deref(),
        Some("oxy-ttl=1d")
    );
    assert_eq!(
        retention_tag(app(), &short, &p).as_deref(),
        Some("oxy-ttl=90d")
    );
}

#[test]
fn a_random_suffix_does_not_change_the_matched_class() {
    // Uploads always get a random suffix; the suffix lands on the FILENAME, so
    // the prefix match must be unaffected.
    let p = policy(&[("uploads/", Some("30d"))]);
    let key = normalize_pathname(app(), "uploads/report.pdf", true).unwrap();
    assert_ne!(
        key,
        normalize_pathname(app(), "uploads/report.pdf", true).unwrap()
    );
    assert_eq!(
        retention_tag(app(), &key, &p).as_deref(),
        Some("oxy-ttl=30d")
    );
}

/// A copy whose source was never written is `NotFound`, whose message the
/// host-call classifier reads as `not_found` — control flow, not a failure.
/// The object store answers the same way (`s3::copy_source_missing`).
#[tokio::test]
async fn copy_from_a_never_written_key_is_not_found() {
    let tmp = use_temp_state_dir();
    let a = Uuid::new_v4();
    let missing = format!("customer-app-storage/{a}/reports/never-written.csv");
    let err = copy(a, &missing, "archive/report.csv", false)
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::NotFound(_)), "{err}");
    assert_eq!(
        err.to_string(),
        format!("storage object not found: copy source '{missing}' does not exist")
    );
    let _ = std::fs::remove_dir_all(tmp);
}
