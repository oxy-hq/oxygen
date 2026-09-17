//! `ctx.storage` — the custom-app **asset store**.
//!
//! One store for both kinds of asset an app produces:
//!
//! * **Uploaded** — a file a human picks in the browser. The function mints a
//!   presigned PUT and the browser uploads **straight to S3**, so the bytes never
//!   cross the isolate's JSON op boundary (every op arg is a JSON string) and never
//!   proxy through oxy (the 32 MiB body limit in `custom_apps_proxy` exists
//!   precisely because large bodies "should hit S3 directly").
//! * **Generated** — a file the function itself produces (a rendered PDF, a CSV
//!   export, a chart PNG). Written server-side with [`put`], which takes raw bytes,
//!   so **binary** generated assets are first-class rather than text-only.
//!
//! Both live in the same per-app silo and share one addressing scheme, so "list the
//! reports" doesn't care which way a file arrived. Convention is a leading path
//! segment (`uploads/…`, `generated/…`); [`get_upload_url`] defaults to `uploads/`.
//!
//! ## Shape, and where it comes from
//!
//! The surface follows the shape the market converged on (Vercel Blob's
//! `put`/`head`/`list`/`del`/`copy`, Cloudflare R2 + S3 presigning, Supabase's
//! small-upload/large-upload split), with the defaults that turned out to matter:
//!
//! * **`allow_overwrite` defaults to false** — silently clobbering an asset is
//!   worse than an error. Enforced *atomically* via the S3 conditional write
//!   (`If-None-Match: *`), not a racy head-then-put.
//! * **`add_random_suffix`** to make a caller-chosen name collision-proof.
//! * **Cursor pagination on [`list`]** — an app with 100k assets must not be able
//!   to make one call walk every page into memory.
//! * **Presign TTL up to 7 days** (the SigV4 maximum). A 15-minute link is right
//!   for an upload and wrong for a download link emailed to a human, which is
//!   exactly what `ctx.email.send` + this module are for together.
//!
//! Where Oxy is deliberately *simpler* than Vercel: their client-upload flow needs
//! a token dance (`handleUpload` / `onBeforeGenerateToken`) because the browser
//! posts to an unauthenticated route that must authorize the upload itself. An Oxy
//! Function is **already** the authenticated, authorized server context — identity
//! is resolved before the isolate starts and `storage.{read,write}` is a
//! fail-closed manifest capability — so there is no token to mint or verify.
//!
//! Every object is private; reads are always presigned and time-boxed. There is no
//! `access: 'public'` equivalent, on purpose.
//!
//! ## Tenant isolation
//!
//! Everything lives under `customer-app-storage/<app_id>/` and **every** operation
//! re-validates that the resolved key sits under the invoking app's prefix, so a
//! function cannot reach another app's silo even by forging a key.
//!
//! ## Local dev
//!
//! `put`/`get`/`head`/`list`/`delete`/`copy` fall back to the filesystem (like the
//! build store), so server-side asset storage works with no S3. *Presigning*
//! genuinely needs object storage — it mints a URL the browser talks to directly —
//! so without a bucket those two calls return a clear error; point
//! `AWS_ENDPOINT_URL` at a local MinIO to exercise them.

mod local;
pub mod metering;
pub mod quota;
pub mod retention;
pub(crate) mod s3;
pub mod sweeper;
#[cfg(test)]
mod tests;
pub mod usage;

use std::time::Duration;

use serde::Serialize;
use uuid::Uuid;

pub use retention::{RetentionPolicy, RetentionRule};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("s3 error: {0}")]
    S3(String),
    #[error("filesystem storage error: {0}")]
    Io(String),
    /// A key/pathname that escapes the invoking app's silo, or an unsafe name.
    #[error("storage access denied: {0}")]
    Denied(String),
    /// Over a configured size ceiling.
    #[error("storage payload too large: {0}")]
    TooLarge(String),
    /// The target already exists and `allow_overwrite` was not set.
    #[error("storage conflict: {0}")]
    AlreadyExists(String),
    /// A copy names a source that was never written. `get` and `head` answer
    /// that with `Ok(None)`; a copy has nothing to answer with, so it is an
    /// error — one both stores raise in this shape, which the host-call
    /// classifier reads as `not_found` (control flow) rather than a failure.
    #[error("storage object not found: {0}")]
    NotFound(String),
    /// A presign was requested but no object-store bucket is configured.
    #[error("object storage not configured: {0}")]
    NotConfigured(String),
    #[error("invalid storage request: {0}")]
    Invalid(String),
}

// ── Config ──────────────────────────────────────────────────────────────────

/// Dedicated asset bucket, separate from the build-store and compile-blob buckets
/// so asset retention/lifecycle is governed independently.
///
/// `pub(crate)` because the document model signs into the SAME bucket under its
/// own `org-documents/` prefix. That is deliberate reuse: a second bucket would
/// mean a second ops task before documents worked in any environment, and the
/// prefix already separates the two owners — everything under
/// `customer-app-storage/{app_id}/` is one app's silo, everything under
/// `org-documents/{org_id}/` belongs to the org. See
/// `server::api::documents::storage`.
pub(crate) fn bucket() -> Option<String> {
    std::env::var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET")
        .ok()
        .filter(|b| !b.trim().is_empty())
}

/// Upload links are short-lived: the browser uses one immediately.
const DEFAULT_UPLOAD_TTL_SECS: u64 = 900;
/// Download links are longer-lived because they get emailed to humans, who do not
/// read their mail within fifteen minutes.
const DEFAULT_DOWNLOAD_TTL_SECS: u64 = 3600;
/// SigV4's own ceiling (7 days). Anything longer cannot be signed at all.
const MAX_PRESIGN_TTL_SECS: u64 = 604_800;

// A download link must outlive an upload link — it gets emailed to a human, who
// does not read their mail in the fifteen minutes an upload window allows.
const _: () = assert!(DEFAULT_DOWNLOAD_TTL_SECS > DEFAULT_UPLOAD_TTL_SECS);

/// Ceiling on a single presigned upload, bound into the signature so S3 itself
/// rejects a larger body.
const DEFAULT_MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024;

/// Cap on the server-side `put`/`get` path — a function generating a report, not a
/// file transfer. 6 MiB matches the point other platforms draw the same line
/// (Supabase's standard-upload limit) and keeps the base64 round-trip well inside
/// the isolate's 64 MiB JSON budget. Larger generated assets should be streamed to
/// a presigned PUT.
const INLINE_BLOB_MAX_BYTES: usize = 6 * 1024 * 1024;

/// Default page size for [`list`], and the hard ceiling on a caller's `limit`.
const DEFAULT_LIST_LIMIT: usize = 100;
const MAX_LIST_LIMIT: usize = 1000;

fn presign_ttl(requested_secs: Option<u64>, default_secs: u64) -> Duration {
    let secs = requested_secs
        .filter(|&s| s > 0)
        .unwrap_or(default_secs)
        .min(MAX_PRESIGN_TTL_SECS);
    Duration::from_secs(secs)
}

fn max_upload_bytes() -> u64 {
    std::env::var("OXY_CUSTOMER_APPS_STORAGE_MAX_UPLOAD_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_MAX_UPLOAD_BYTES)
}

// ── Key scheme & tenant isolation ─────────────────────────────────────────────

/// The app's silo. `app_id`-first so every app is a hard boundary; no leading
/// slash, trailing slash (mirrors the build store's `build_prefix`).
pub(crate) fn app_prefix(app_id: Uuid) -> String {
    format!("customer-app-storage/{app_id}/")
}

/// Resolve the `x-amz-tagging` value for a **full** silo key by matching its
/// app-relative part against the app's retention policy.
///
/// The policy declares prefixes the way an author writes them (`uploads/`) while
/// keys carry the silo prefix (`customer-app-storage/<id>/uploads/…`), so the
/// silo part is stripped before matching. Skipping that strip would mean no rule
/// ever matched and every object silently lived forever — a failure that looks
/// exactly like success until the bill arrives.
///
/// `None` at any step (no policy, key outside the silo, no matching prefix) means
/// no tag, which means no lifecycle rule applies. Fail open, deliberately.
fn retention_tag(app_id: Uuid, key: &str, policy: &RetentionPolicy) -> Option<String> {
    if policy.is_empty() {
        return None;
    }
    let relative = key.strip_prefix(app_prefix(app_id).as_str())?;
    policy
        .resolve(relative)
        .map(retention::TtlClass::tagging_header)
}

/// Sanitize one path segment: keep `[A-Za-z0-9._-]`, collapse anything else to
/// `_`, strip leading dots (no `..` or hidden-file surprises), bound the length.
fn sanitize_segment(segment: &str) -> String {
    let mut out: String = segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    while out.starts_with('.') {
        out.remove(0);
    }
    out.truncate(120);
    out
}

/// Normalize a caller-chosen pathname into a full, validated key inside the app's
/// silo. Each segment is sanitized independently so `/` still nests (an app can
/// organize `generated/2026/q1.pdf`) while `..` can never escape.
///
/// `add_random_suffix` inserts a short random component before the extension,
/// which is how a caller keeps a human-readable name without risking collisions.
pub(crate) fn normalize_pathname(
    app_id: Uuid,
    pathname: &str,
    add_random_suffix: bool,
) -> Result<String, StorageError> {
    let trimmed = pathname.trim().trim_start_matches('/');
    // Accept an already-prefixed key so a round-tripped key from list()/put() can
    // be handed straight back in.
    let prefix = app_prefix(app_id);
    let relative = trimmed.strip_prefix(prefix.as_str()).unwrap_or(trimmed);
    if relative.is_empty() {
        return Err(StorageError::Invalid(
            "pathname must not be empty".to_string(),
        ));
    }
    let mut segments: Vec<String> = relative
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(sanitize_segment)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Err(StorageError::Invalid(format!(
            "pathname '{pathname}' has no usable segments"
        )));
    }
    if add_random_suffix {
        let last = segments.pop().expect("non-empty checked above");
        let suffix = Uuid::new_v4().simple().to_string()[..8].to_string();
        segments.push(match last.rsplit_once('.') {
            // Keep the extension last so content-type sniffing still works.
            Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{suffix}.{ext}"),
            _ => format!("{last}-{suffix}"),
        });
    }
    Ok(format!("{prefix}{}", segments.join("/")))
}

/// The cross-tenant defense: a key handed back to us MUST sit inside the invoking
/// app's prefix and carry no traversal. Returns the normalized key.
pub(crate) fn validate_key(app_id: Uuid, key: &str) -> Result<String, StorageError> {
    let key = key.trim().trim_start_matches('/');
    let prefix = app_prefix(app_id);
    if !key.starts_with(prefix.as_str()) {
        return Err(StorageError::Denied(format!(
            "key '{key}' is outside this app's storage"
        )));
    }
    if key.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(StorageError::Denied(format!(
            "key '{key}' contains a path traversal"
        )));
    }
    if key.len() == prefix.len() {
        return Err(StorageError::Invalid("key must name an object".to_string()));
    }
    Ok(key.to_string())
}

/// Resolve an optional caller sub-prefix for [`list`], confined to the app silo.
fn resolve_list_prefix(app_id: Uuid, sub: Option<&str>) -> Result<String, StorageError> {
    let prefix = app_prefix(app_id);
    match sub.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(prefix),
        Some(s) => {
            let s = s.trim_start_matches('/');
            if s.split('/').any(|seg| seg == "..") {
                return Err(StorageError::Denied(format!(
                    "list prefix '{s}' contains a path traversal"
                )));
            }
            if s.starts_with(prefix.as_str()) {
                Ok(s.to_string())
            } else {
                Ok(format!("{prefix}{s}"))
            }
        }
    }
}

/// Best-effort content type from the extension, so a *generated* asset gets a
/// usable type without every call site restating it.
pub(crate) fn guess_content_type(key: &str) -> &'static str {
    let ext = key.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "json" => "application/json",
        "txt" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "md" => "text/markdown",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "zip" => "application/zip",
        "parquet" => "application/vnd.apache.parquet",
        _ => "application/octet-stream",
    }
}

// ── Returned shapes (serialized straight to the JS caller) ────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadUrl {
    pub url: String,
    pub key: String,
    pub expires_at: String,
    /// The `x-amz-tagging` header the uploader **must** send verbatim, when the
    /// app's retention policy assigns this key a class. It is bound into the
    /// signature, so omitting it fails the upload with a signature mismatch
    /// rather than silently storing an object that never expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagging: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadUrl {
    pub url: String,
    pub expires_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutResult {
    pub key: String,
    pub size: u64,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredObject {
    pub key: String,
    pub size: i64,
    pub content_type: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPage {
    pub objects: Vec<StoredObject>,
    /// Opaque cursor for the next page; `None` when the listing is complete.
    pub cursor: Option<String>,
    pub has_more: bool,
}

/// Options for [`put`].
#[derive(Debug, Default)]
pub struct PutOptions {
    pub content_type: Option<String>,
    pub add_random_suffix: bool,
    pub allow_overwrite: bool,
    /// `Cache-Control: max-age=<secs>` stored on the object, so a generated asset
    /// served through a presigned GET is cacheable by the browser.
    pub cache_control_max_age: Option<u64>,
}

fn expires_at(ttl: Duration) -> String {
    (chrono::Utc::now() + chrono::Duration::from_std(ttl).unwrap_or_default()).to_rfc3339()
}

fn require_bucket(op: &str) -> Result<String, StorageError> {
    bucket().ok_or_else(|| {
        StorageError::NotConfigured(format!(
            "{op} requires OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET (point AWS_ENDPOINT_URL at a \
             local MinIO to exercise it in dev)"
        ))
    })
}

// ── Presigned upload / download (require object storage) ──────────────────────

/// Mint a presigned PUT the browser uses to upload one file directly to S3.
/// Content-Type and Content-Length are bound into the signature, so S3 rejects a
/// mismatched or oversized body without oxy ever seeing the bytes.
///
/// `pathname` defaults to `uploads/<filename>`; a random suffix is added by
/// default so two people uploading `report.pdf` don't collide.
pub async fn get_upload_url(
    app_id: Uuid,
    pathname: &str,
    content_type: &str,
    content_length: u64,
    ttl_secs: Option<u64>,
    retention: &RetentionPolicy,
) -> Result<UploadUrl, StorageError> {
    if content_length == 0 {
        return Err(StorageError::Invalid(
            "contentLength must be greater than 0".to_string(),
        ));
    }
    let ceiling = max_upload_bytes();
    if content_length > ceiling {
        return Err(StorageError::TooLarge(format!(
            "upload of {content_length} bytes exceeds the {ceiling}-byte ceiling \
             (OXY_CUSTOMER_APPS_STORAGE_MAX_UPLOAD_BYTES)"
        )));
    }
    let bucket = require_bucket("presigned uploads")?;
    // Uploads ALWAYS get a random suffix (unlike `put`, which honors the caller's
    // choice): a browser upload is user-driven and collision-prone — two people
    // picking `report.pdf` must not clobber each other — and the returned `key` is
    // authoritative, so the caller records it and never needs to predict it.
    let key = normalize_pathname(app_id, pathname, true)?;
    let content_type = if content_type.trim().is_empty() {
        guess_content_type(&key)
    } else {
        content_type
    };
    let ttl = presign_ttl(ttl_secs, DEFAULT_UPLOAD_TTL_SECS);
    let tagging = retention_tag(app_id, &key, retention);
    let url = s3::presign_put(
        &bucket,
        &key,
        content_type,
        content_length,
        ttl,
        tagging.as_deref(),
    )
    .await?;
    Ok(UploadUrl {
        url,
        key,
        expires_at: expires_at(ttl),
        tagging,
    })
}

/// Mint a presigned GET for an object in this app's silo. `download` forces a
/// save-as via `Content-Disposition: attachment`, which is what an emailed report
/// link wants.
pub async fn get_download_url(
    app_id: Uuid,
    key: &str,
    ttl_secs: Option<u64>,
    download: bool,
) -> Result<DownloadUrl, StorageError> {
    let key = validate_key(app_id, key)?;
    let bucket = require_bucket("presigned downloads")?;
    let ttl = presign_ttl(ttl_secs, DEFAULT_DOWNLOAD_TTL_SECS);
    let filename = key.rsplit('/').next().unwrap_or("download").to_string();
    let url = s3::presign_get(&bucket, &key, ttl, download.then_some(filename)).await?;
    Ok(DownloadUrl {
        url,
        expires_at: expires_at(ttl),
    })
}

// ── Server-side asset operations (S3 or local filesystem) ─────────────────────

/// Write a **generated** asset. Takes raw bytes, so binary output (PDF, PNG,
/// Parquet) is first-class rather than text-only.
pub async fn put(
    app_id: Uuid,
    pathname: &str,
    body: Vec<u8>,
    opts: PutOptions,
    retention: &RetentionPolicy,
) -> Result<PutResult, StorageError> {
    if body.len() > INLINE_BLOB_MAX_BYTES {
        return Err(StorageError::TooLarge(format!(
            "ctx.storage.put is capped at {INLINE_BLOB_MAX_BYTES} bytes; for larger assets \
             mint a presigned upload URL and stream to it"
        )));
    }
    let key = normalize_pathname(app_id, pathname, opts.add_random_suffix)?;
    let content_type = opts
        .content_type
        .clone()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| guess_content_type(&key).to_string());
    let size = body.len() as u64;
    match bucket() {
        Some(bucket) => {
            let tagging = retention_tag(app_id, &key, retention);
            s3::put(
                &bucket,
                &key,
                body,
                &content_type,
                &opts,
                tagging.as_deref(),
            )
            .await?
        }
        // The filesystem fallback has no tags and no lifecycle engine, so a local
        // asset never expires. Stated here rather than left implicit: it is a real
        // dev/prod divergence, and `spawn_lifecycle_verify` logs the same fact
        // at boot so nobody concludes retention is broken in prod from a local run.
        None => local::put(&key, body, opts.allow_overwrite).await?,
    }
    Ok(PutResult {
        key,
        size,
        content_type,
    })
}

/// Read a small asset back. `Ok(None)` when absent.
pub async fn get(
    app_id: Uuid,
    key: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>, StorageError> {
    let key = validate_key(app_id, key)?;
    match bucket() {
        Some(bucket) => s3::get(&bucket, &key).await,
        None => local::get(&key).await,
    }
}

/// Metadata without the body. `Ok(None)` when absent.
pub async fn head(app_id: Uuid, key: &str) -> Result<Option<StoredObject>, StorageError> {
    let key = validate_key(app_id, key)?;
    match bucket() {
        Some(bucket) => s3::head(&bucket, &key).await,
        None => local::head(&key).await,
    }
}

/// One page of the app's assets. Bounded and cursor-paginated: a silo with 100k
/// objects must not turn one call into an unbounded walk.
pub async fn list(
    app_id: Uuid,
    sub_prefix: Option<&str>,
    limit: Option<usize>,
    cursor: Option<String>,
) -> Result<ListPage, StorageError> {
    let prefix = resolve_list_prefix(app_id, sub_prefix)?;
    let limit = limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT);
    match bucket() {
        Some(bucket) => s3::list(&bucket, &prefix, limit, cursor).await,
        None => local::list(&prefix, limit, cursor),
    }
}

/// Delete one or more assets. Idempotent: deleting an absent key is not an error.
/// Returns the number of keys **accepted** for deletion, not a count of keys that
/// existed — deletion is a no-op success for an absent key, and S3 can't cheaply
/// report prior existence, so both backends count an absent key as deleted.
pub async fn delete(app_id: Uuid, keys: &[String]) -> Result<usize, StorageError> {
    if keys.is_empty() {
        return Ok(0);
    }
    if keys.len() > MAX_LIST_LIMIT {
        return Err(StorageError::Invalid(format!(
            "delete accepts at most {MAX_LIST_LIMIT} keys per call"
        )));
    }
    let validated: Vec<String> = keys
        .iter()
        .map(|k| validate_key(app_id, k))
        .collect::<Result<_, _>>()?;
    match bucket() {
        Some(bucket) => s3::delete(&bucket, &validated).await,
        None => local::delete(&validated).await,
    }
}

/// Server-side copy within the app's silo — no bytes through the isolate.
pub async fn copy(
    app_id: Uuid,
    from_key: &str,
    to_pathname: &str,
    allow_overwrite: bool,
) -> Result<PutResult, StorageError> {
    let from = validate_key(app_id, from_key)?;
    let to = normalize_pathname(app_id, to_pathname, false)?;
    if from == to {
        return Err(StorageError::Invalid(
            "copy source and destination are the same key".to_string(),
        ));
    }
    match bucket() {
        Some(bucket) => s3::copy(&bucket, &from, &to, allow_overwrite).await?,
        None => local::copy(&from, &to, allow_overwrite).await?,
    }
    let meta = head(app_id, &to).await?;
    Ok(PutResult {
        key: to.clone(),
        size: meta.as_ref().map(|m| m.size.max(0) as u64).unwrap_or(0),
        content_type: meta
            .and_then(|m| m.content_type)
            .unwrap_or_else(|| guess_content_type(&to).to_string()),
    })
}

/// Delete every asset belonging to an app — used when the app itself is deleted so
/// its bytes don't outlive it.
pub async fn delete_app_assets(app_id: Uuid) -> Result<(), StorageError> {
    let prefix = app_prefix(app_id);
    match bucket() {
        Some(bucket) => s3::delete_prefix(&bucket, &prefix).await,
        None => local::delete_prefix(&prefix).await,
    }
}
