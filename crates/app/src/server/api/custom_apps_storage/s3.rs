//! S3 backend for the custom-app asset store.
//!
//! Presigning is the reason this module exists: it is what lets the browser talk
//! to object storage directly instead of streaming megabytes through oxy and the
//! V8 isolate. Everything else here is the ordinary object CRUD the store needs.

use std::time::Duration;

use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::{ByteStream, DateTimeFormat};

use super::{ListPage, PutOptions, StorageError, StoredObject};

/// Build an S3 client, honoring `AWS_ENDPOINT_URL` (LocalStack/MinIO) with
/// path-style addressing — identical to the build store's client.
///
/// `pub(crate)` so `retention` (bucket-lifecycle calls) and
/// `server::audit_anchor` (the Object Lock anchors) share the same endpoint
/// configuration; a second client builder would be one more place for the
/// MinIO path-style flag to be forgotten.
pub(crate) async fn client() -> S3Client {
    let shared = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let mut builder = aws_sdk_s3::config::Builder::from(&shared);
    if std::env::var("AWS_ENDPOINT_URL")
        .ok()
        .is_some_and(|v| !v.trim().is_empty())
    {
        builder = builder.force_path_style(true);
    }
    S3Client::from_conf(builder.build())
}

fn presigning(ttl: Duration) -> Result<PresigningConfig, StorageError> {
    PresigningConfig::expires_in(ttl)
        .map_err(|e| StorageError::S3(format!("invalid presign config: {e}")))
}

/// Presigned PUT. Content-Type and Content-Length are part of the signature, so a
/// client that sends a different type or a larger body is rejected by S3 itself —
/// the size ceiling is enforced by the object store, not by trust in the browser.
///
/// `tagging` (`oxy-ttl=30d`) is bound into the signature the same way, so the
/// retention class cannot be stripped or forged by the uploading browser. The
/// caller **must** send a matching `x-amz-tagging` header or S3 rejects the
/// upload with a signature mismatch — `@oxy-hq/sdk`'s uploader does this, and
/// `get_upload_url` returns the value so a hand-rolled uploader can too.
pub(crate) async fn presign_put(
    bucket: &str,
    key: &str,
    content_type: &str,
    content_length: u64,
    ttl: Duration,
    tagging: Option<&str>,
) -> Result<String, StorageError> {
    let mut req = client()
        .await
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(content_type)
        .content_length(content_length as i64);
    if let Some(tagging) = tagging {
        req = req.tagging(tagging);
    }
    let signed = req
        .presigned(presigning(ttl)?)
        .await
        .map_err(|e| StorageError::S3(format!("presign put {key}: {e}")))?;
    Ok(signed.uri().to_string())
}

/// Presigned GET. `download_filename` forces a save-as through
/// `Content-Disposition`, which is what an emailed report link wants.
pub(crate) async fn presign_get(
    bucket: &str,
    key: &str,
    ttl: Duration,
    download_filename: Option<String>,
) -> Result<String, StorageError> {
    let mut req = client().await.get_object().bucket(bucket).key(key);
    if let Some(name) = download_filename {
        // Quotes escaped so a crafted filename can't break out of the header.
        let safe = name.replace('"', "");
        req = req.response_content_disposition(format!("attachment; filename=\"{safe}\""));
    }
    let signed = req
        .presigned(presigning(ttl)?)
        .await
        .map_err(|e| StorageError::S3(format!("presign get {key}: {e}")))?;
    Ok(signed.uri().to_string())
}

/// `tagging` carries the retention class (`oxy-ttl=30d`) the caller resolved from
/// the app's manifest. Kept out of [`PutOptions`] on purpose: that struct is what
/// a function author's call maps onto, and retention is not theirs to set
/// per-write — it is app policy.
pub(super) async fn put(
    bucket: &str,
    key: &str,
    body: Vec<u8>,
    content_type: &str,
    opts: &PutOptions,
    tagging: Option<&str>,
) -> Result<(), StorageError> {
    let mut req = client()
        .await
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(content_type)
        .body(ByteStream::from(body));
    if let Some(max_age) = opts.cache_control_max_age {
        req = req.cache_control(format!("max-age={max_age}"));
    }
    // Retention class, resolved from the app's manifest by the caller. Absent →
    // no tag → no lifecycle rule matches → the object never expires.
    if let Some(tagging) = tagging {
        req = req.tagging(tagging);
    }
    if !opts.allow_overwrite {
        // Atomic "create only" — S3 conditional write. A head-then-put check would
        // be racy, and losing a generated report to a concurrent write is exactly
        // the failure this default exists to prevent.
        req = req.if_none_match("*");
    }
    req.send().await.map_err(|e| {
        let detail = format!("{e}");
        if !opts.allow_overwrite && is_precondition_failed(&detail) {
            StorageError::AlreadyExists(format!(
                "'{key}' already exists; pass allowOverwrite to replace it or \
                 addRandomSuffix to store alongside it"
            ))
        } else {
            StorageError::S3(format!("put_object {key}: {e}"))
        }
    })?;
    Ok(())
}

/// S3 signals a failed `If-None-Match` as 412 PreconditionFailed. The SDK models
/// it as an unmodeled error here, so match on the wire signal.
fn is_precondition_failed(detail: &str) -> bool {
    detail.contains("PreconditionFailed") || detail.contains("status: 412")
}

pub(super) async fn get(
    bucket: &str,
    key: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>, StorageError> {
    match client()
        .await
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
    {
        Ok(resp) => {
            let content_type = resp.content_type().map(str::to_string);
            let data = resp
                .body
                .collect()
                .await
                .map_err(|e| StorageError::S3(format!("collect {key}: {e}")))?;
            Ok(Some((data.into_bytes().to_vec(), content_type)))
        }
        Err(err) => {
            if err
                .as_service_error()
                .map(|e| e.is_no_such_key())
                .unwrap_or(false)
            {
                Ok(None)
            } else {
                Err(StorageError::S3(format!("get_object {key}: {err}")))
            }
        }
    }
}

pub(crate) async fn head(bucket: &str, key: &str) -> Result<Option<StoredObject>, StorageError> {
    match client()
        .await
        .head_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
    {
        Ok(resp) => Ok(Some(StoredObject {
            key: key.to_string(),
            size: resp.content_length().unwrap_or(0),
            content_type: resp.content_type().map(str::to_string),
            last_modified: resp
                .last_modified()
                .and_then(|t| t.fmt(DateTimeFormat::DateTime).ok()),
        })),
        Err(err) => {
            // HeadObject returns a bare 404 with no modeled NoSuchKey body.
            let detail = format!("{err}");
            if err
                .as_service_error()
                .map(|e| e.is_not_found())
                .unwrap_or(false)
                || detail.contains("status: 404")
            {
                Ok(None)
            } else {
                Err(StorageError::S3(format!("head_object {key}: {err}")))
            }
        }
    }
}

/// One page only — the caller drives pagination with the returned cursor. The
/// unbounded "loop every continuation token" shape is a memory hazard on a large
/// silo and is deliberately not offered.
pub(super) async fn list(
    bucket: &str,
    prefix: &str,
    limit: usize,
    cursor: Option<String>,
) -> Result<ListPage, StorageError> {
    let mut req = client()
        .await
        .list_objects_v2()
        .bucket(bucket)
        .prefix(prefix)
        .max_keys(limit as i32);
    if let Some(token) = cursor.filter(|c| !c.is_empty()) {
        req = req.continuation_token(token);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| StorageError::S3(format!("list_objects_v2 {prefix}: {e}")))?;
    let objects = resp
        .contents()
        .iter()
        .filter_map(|o| {
            o.key().map(|k| StoredObject {
                key: k.to_string(),
                size: o.size().unwrap_or(0),
                content_type: None,
                last_modified: o
                    .last_modified()
                    .and_then(|t| t.fmt(DateTimeFormat::DateTime).ok()),
            })
        })
        .collect();
    let has_more = resp.is_truncated().unwrap_or(false);
    Ok(ListPage {
        objects,
        cursor: has_more
            .then(|| resp.next_continuation_token().map(str::to_string))
            .flatten(),
        has_more,
    })
}

pub(super) async fn delete(bucket: &str, keys: &[String]) -> Result<usize, StorageError> {
    let ids: Vec<aws_sdk_s3::types::ObjectIdentifier> = keys
        .iter()
        .filter_map(|k| {
            aws_sdk_s3::types::ObjectIdentifier::builder()
                .key(k)
                .build()
                .ok()
        })
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }
    // Count = keys ACCEPTED for deletion. S3 DeleteObjects is idempotent — a key
    // that didn't exist is reported as deleted too — and `quiet(true)` suppresses
    // the per-key response, so there's no cheap way to count only keys that
    // actually existed. The local backend matches this (absent counts as deleted).
    let count = ids.len();
    let payload = aws_sdk_s3::types::Delete::builder()
        .set_objects(Some(ids))
        .quiet(true)
        .build()
        .map_err(|e| StorageError::S3(format!("build Delete payload: {e}")))?;
    client()
        .await
        .delete_objects()
        .bucket(bucket)
        .delete(payload)
        .send()
        .await
        .map_err(|e| StorageError::S3(format!("delete_objects: {e}")))?;
    Ok(count)
}

pub(super) async fn copy(
    bucket: &str,
    from: &str,
    to: &str,
    allow_overwrite: bool,
) -> Result<(), StorageError> {
    if !allow_overwrite && head(bucket, to).await?.is_some() {
        return Err(StorageError::AlreadyExists(format!(
            "'{to}' already exists; pass allowOverwrite to replace it"
        )));
    }
    // CopySource is `<bucket>/<key>`; the key must be URL-encoded per the S3 API.
    // Encode per path segment so the `/` separators survive. A no-op for today's
    // sanitized keys, but correct if the key scheme ever loosens.
    let source_key = from
        .split('/')
        .map(urlencoding::encode)
        .collect::<Vec<_>>()
        .join("/");
    client()
        .await
        .copy_object()
        .bucket(bucket)
        .key(to)
        .copy_source(format!("{bucket}/{source_key}"))
        .send()
        .await
        .map_err(|err| {
            let code = err.as_service_error().and_then(ProvideErrorMetadata::code);
            if copy_source_missing(code) {
                StorageError::NotFound(format!("copy source '{from}' does not exist"))
            } else {
                StorageError::S3(format!("copy_object {from} -> {to}: {err}"))
            }
        })?;
    Ok(())
}

/// Whether a failed `CopyObject` means its source is missing. S3 answers that
/// with 404 `NoSuchKey`, but `CopyObjectError` models no such variant (only
/// `ObjectNotInActiveTierError`), so the error code is what there is to read —
/// and `SdkError`'s `Display` says only "service error", which nothing
/// downstream can act on. The code rather than the status: `NoSuchBucket` is a
/// 404 too, and that one is the deployment's to fix, not the caller's.
fn copy_source_missing(code: Option<&str>) -> bool {
    code == Some("NoSuchKey")
}

/// Delete everything under a prefix, page by page. Unlike [`list`], walking every
/// page IS the job here, and each page is batched into one `DeleteObjects` call
/// (≤1000 keys, which matches `ListObjectsV2`'s page size).
pub(super) async fn delete_prefix(bucket: &str, prefix: &str) -> Result<(), StorageError> {
    let client = client().await;
    let mut continuation: Option<String> = None;
    loop {
        let mut req = client.list_objects_v2().bucket(bucket).prefix(prefix);
        if let Some(token) = &continuation {
            req = req.continuation_token(token.clone());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| StorageError::S3(format!("list_objects_v2 {prefix}: {e}")))?;
        let keys: Vec<String> = resp
            .contents()
            .iter()
            .filter_map(|o| o.key().map(str::to_string))
            .collect();
        if !keys.is_empty() {
            delete(bucket, &keys).await?;
        }
        if resp.is_truncated().unwrap_or(false) {
            continuation = resp.next_continuation_token().map(str::to_string);
            if continuation.is_none() {
                break;
            }
        } else {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::copy_source_missing;

    /// The code decides, not the status: a missing bucket is a 404 as well,
    /// and that one must stay an error.
    #[test]
    fn only_a_no_such_key_copy_error_means_the_source_is_missing() {
        assert!(copy_source_missing(Some("NoSuchKey")));
        assert!(!copy_source_missing(Some("NoSuchBucket")));
        assert!(!copy_source_missing(Some("AccessDenied")));
        assert!(!copy_source_missing(None));
    }
}
