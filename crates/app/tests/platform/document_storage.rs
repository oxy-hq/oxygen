//! The document upload path, against a real object store.
//!
//! Everything else about documents is proved against Postgres. This is the half
//! that is not Postgres: a presigned PUT the browser talks to directly, and a
//! presigned GET it is redirected to. Until this existed the only thing asserted
//! about that path was that a missing bucket answers `503` — which is a test of
//! the refusal, not of the feature.
//!
//! Two of the cases here exist to prove claims the code makes in prose. The
//! module doc says content type and length are bound into the signature, so the
//! object store rejects a client that sends something else; that is either true
//! or it is a comment. `a_larger_body_than_was_signed_for_is_refused` and
//! `a_different_content_type_than_was_signed_for_is_refused` settle it.
//!
//! # Opt-in, and why it skips rather than fails
//!
//! Needs an S3-compatible endpoint. Without one the test would fail on every
//! laptop and in CI, which trains people to ignore it. It skips loudly instead.
//!
//! ```bash
//! docker run -d --name oxy-doc-minio -p 9000:9000 \
//!   -e MINIO_ROOT_USER=oxydocs -e MINIO_ROOT_PASSWORD=oxydocs123 \
//!   minio/minio:latest server /data
//! docker run --rm --network host --entrypoint sh minio/mc:latest -c \
//!   "mc alias set l http://127.0.0.1:9000 oxydocs oxydocs123 && mc mb --ignore-existing l/oxy-documents"
//!
//! AWS_ENDPOINT_URL=http://127.0.0.1:9000 AWS_REGION=us-east-1 \
//! AWS_ACCESS_KEY_ID=oxydocs AWS_SECRET_ACCESS_KEY=oxydocs123 \
//! OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET=oxy-documents \
//!   cargo nextest run -p oxy-app --test platform -E 'test(document_storage)'
//! ```

use uuid::Uuid;

use oxy_app::server::api::documents::storage;

/// Is an object store configured? Returns the reason it is not, for the skip
/// message — "skipped" with no cause is how a permanently-skipped test hides.
fn unconfigured() -> Option<&'static str> {
    if std::env::var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .is_none()
    {
        return Some("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET is unset");
    }
    if std::env::var("AWS_ACCESS_KEY_ID").is_err() {
        return Some("AWS_ACCESS_KEY_ID is unset");
    }
    None
}

macro_rules! require_store {
    () => {
        if let Some(why) = unconfigured() {
            // On a laptop this is a loud skip, which is the point of the module
            // doc above. In CI it is a FAILURE, because a returning test counts
            // as a passing one: these five were the branch's only proof that the
            // upload path works, and they were green-by-default in every
            // automated run — five presigned round trips in 0.59s is the tell.
            //
            // An explicit flag, not an inferred `CI`, and named after the one
            // `crates/oltp` already uses for the identical problem
            // (`OXY_OLTP_REQUIRE_DB`). CI sets it beside the `minio` service
            // that makes these runnable; a laptop does not, and gets the skip.
            if std::env::var("OXY_REQUIRE_OBJECT_STORE").is_ok() {
                panic!(
                    "no object store configured ({why}) — these tests are the only proof the \
                     upload path works, so CI must run them rather than skip them. Start MinIO \
                     and export OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET / AWS_* (see the module doc), \
                     or drop this suite deliberately rather than by omission."
                );
            }
            eprintln!("skipping: no object store configured ({why})");
            return;
        }
    };
}

/// A fresh document id per test, so two runs against the same bucket cannot
/// read each other's objects and pass for the wrong reason.
fn ids() -> (Uuid, Uuid) {
    (Uuid::new_v4(), Uuid::new_v4())
}

/// The whole round trip: sign, upload, confirm it landed, sign a download, read
/// the same bytes back.
#[tokio::test]
async fn a_document_uploads_and_comes_back_byte_for_byte() {
    require_store!();
    let (org, doc) = ids();
    let body = b"%PDF-1.7\nHealth permit, Encinitas\n".to_vec();

    // Nothing is there yet. Asserted first, so "it exists" below cannot be a
    // leftover from an earlier run.
    let key = storage::object_key(org, doc, 1);
    assert!(
        !storage::object_exists(&key).await.expect("head"),
        "the key was already occupied before anything was uploaded"
    );

    let (url, signed_key) = storage::upload_url(org, doc, 1, "application/pdf", body.len() as u64)
        .await
        .expect("presign the upload");
    assert_eq!(signed_key, key, "the signed key must be the one we predict");

    let put = reqwest::Client::new()
        .put(&url)
        .header("content-type", "application/pdf")
        .body(body.clone())
        .send()
        .await
        .expect("PUT to the object store");
    assert!(
        put.status().is_success(),
        "upload refused: {}",
        put.status()
    );

    // The check `confirm` makes before a version becomes current.
    assert!(
        storage::object_exists(&key).await.expect("head"),
        "the object store accepted the upload but HEAD cannot see it"
    );

    let get = storage::download_url(&key, "Health Permit — Encinitas")
        .await
        .expect("presign the download");
    let res = reqwest::get(&get).await.expect("GET the object");
    assert!(res.status().is_success());

    // The filename a reader's browser will save it under. The em dash is not a
    // character the sanitiser keeps, which is the point of asserting on it.
    let disposition = res
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposition.contains("Health Permit"),
        "the download is not named after the document: {disposition:?}"
    );

    assert_eq!(res.bytes().await.expect("body").to_vec(), body);
}

/// The size ceiling is enforced by the object store, not by trusting the
/// browser. Sign for a small body, send a larger one, and the store refuses it.
#[tokio::test]
async fn a_larger_body_than_was_signed_for_is_refused() {
    require_store!();
    let (org, doc) = ids();

    let (url, _) = storage::upload_url(org, doc, 1, "text/plain", 16)
        .await
        .expect("presign");

    let oversized = vec![b'x'; 4096];
    let put = reqwest::Client::new()
        .put(&url)
        .header("content-type", "text/plain")
        .body(oversized)
        .send()
        .await
        .expect("PUT");

    assert!(
        !put.status().is_success(),
        "a body 256 times the signed length was accepted"
    );
    assert!(
        !storage::object_exists(&storage::object_key(org, doc, 1))
            .await
            .expect("head"),
        "the refused upload still left an object behind"
    );
}

/// Content type is bound into the signature too, so a client cannot sign for a
/// PDF and store an HTML document at the same key — which on a same-origin
/// download is the difference between a file and a script.
#[tokio::test]
async fn a_different_content_type_than_was_signed_for_is_refused() {
    require_store!();
    let (org, doc) = ids();

    let (url, _) = storage::upload_url(org, doc, 1, "application/pdf", 9)
        .await
        .expect("presign");

    let put = reqwest::Client::new()
        .put(&url)
        .header("content-type", "text/html")
        .body(b"<b>hello</b>".to_vec())
        .send()
        .await
        .expect("PUT");

    assert!(
        !put.status().is_success(),
        "an upload signed as application/pdf was accepted as text/html"
    );
}

/// A zero-length upload is refused before anything is signed. Nothing is
/// gained by asking an object store to hold an empty file, and a caller that
/// sends `contentLength: 0` has a bug this names.
#[tokio::test]
async fn an_empty_upload_is_refused_before_it_is_signed() {
    require_store!();
    let (org, doc) = ids();
    assert!(
        storage::upload_url(org, doc, 1, "application/pdf", 0)
            .await
            .is_err()
    );
}

/// Past the ceiling, refused locally rather than by signing a URL the store
/// will reject minutes later, after the browser has uploaded 100 MiB.
#[tokio::test]
async fn an_upload_past_the_ceiling_never_gets_a_url() {
    require_store!();
    let (org, doc) = ids();
    assert!(
        storage::upload_url(
            org,
            doc,
            1,
            "application/pdf",
            storage::MAX_UPLOAD_BYTES + 1
        )
        .await
        .is_err()
    );
}
