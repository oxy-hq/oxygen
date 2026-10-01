//! The asset operations, each in one [`Silo`] — production's, or an
//! environment's sibling with production's same key as a read-only fallback
//! (`silo`).

use uuid::Uuid;

use super::{
    DEFAULT_DOWNLOAD_TTL_SECS, DEFAULT_LIST_LIMIT, DEFAULT_UPLOAD_TTL_SECS, DownloadUrl,
    INLINE_BLOB_MAX_BYTES, ListPage, MAX_LIST_LIMIT, PutOptions, PutResult, RetentionPolicy, Silo,
    StorageError, StoredObject, UploadUrl, bucket, expires_at, guess_content_type, local,
    max_upload_bytes, normalize_in, presign_ttl, require_bucket, retention_tag_in, s3, validate_in,
};

// ── Presigned upload / download (require object storage) ──────────────────────

/// Mint a presigned PUT the browser uses to upload one file directly to S3.
/// Content-Type and Content-Length are bound into the signature, so S3 rejects a
/// mismatched or oversized body without oxy ever seeing the bytes.
///
/// `pathname` defaults to `uploads/<filename>`; a random suffix is added by
/// default so two people uploading `report.pdf` don't collide. The key is in
/// `silo`, and so is the retention tag's reading of it: an environment's upload
/// is signed exactly as production's same pathname would be.
pub async fn get_upload_url(
    silo: &Silo,
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
    let key = normalize_in(silo, pathname, true)?;
    let content_type = if content_type.trim().is_empty() {
        guess_content_type(&key)
    } else {
        content_type
    };
    let ttl = presign_ttl(ttl_secs, DEFAULT_UPLOAD_TTL_SECS);
    let tagging = retention_tag_in(silo, &key, retention);
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

/// Mint a presigned GET for an object of `silo` — outside production, the
/// environment's copy when it has one, else production's same key. `download`
/// forces a save-as via `Content-Disposition: attachment`, which is what an
/// emailed report link wants.
pub async fn get_download_url(
    silo: &Silo,
    key: &str,
    ttl_secs: Option<u64>,
    download: bool,
) -> Result<DownloadUrl, StorageError> {
    let candidates = read_candidates(silo, key)?;
    let bucket = require_bucket("presigned downloads")?;
    let key = first_existing(candidates).await?;
    let ttl = presign_ttl(ttl_secs, DEFAULT_DOWNLOAD_TTL_SECS);
    let filename = key.rsplit('/').next().unwrap_or("download").to_string();
    let url = s3::presign_get(&bucket, &key, ttl, download.then_some(filename)).await?;
    Ok(DownloadUrl {
        url,
        expires_at: expires_at(ttl),
    })
}

// ── Where a read is answered ──────────────────────────────────────────────────

/// The keys a read of `key` in `silo` tries, in order: the silo's own, then —
/// outside production — production's same relative key, read-only.
fn read_candidates(silo: &Silo, key: &str) -> Result<Vec<String>, StorageError> {
    let own = validate_in(silo, key)?;
    let mut keys = vec![own.clone()];
    if let Some(production) = silo.fallback() {
        let relative = own.strip_prefix(silo.prefix().as_str()).unwrap_or(&own);
        keys.push(production.key(relative));
    }
    Ok(keys)
}

/// The one key a read resolves to: the first of `candidates` that exists, else
/// the silo's own (so a miss reports the key in the caller's silo). A single
/// candidate — production — costs no lookup.
async fn first_existing(candidates: Vec<String>) -> Result<String, StorageError> {
    if candidates.len() > 1 {
        for candidate in &candidates {
            if head_key(candidate).await?.is_some() {
                return Ok(candidate.clone());
            }
        }
    }
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| StorageError::Invalid("no key to read".to_string()))
}

async fn get_key(key: &str) -> Result<Option<(Vec<u8>, Option<String>)>, StorageError> {
    match bucket() {
        Some(bucket) => s3::get(&bucket, key).await,
        None => local::get(key).await,
    }
}

async fn head_key(key: &str) -> Result<Option<StoredObject>, StorageError> {
    match bucket() {
        Some(bucket) => s3::head(&bucket, key).await,
        None => local::head(key).await,
    }
}

/// Outside production, a write without `allowOverwrite` treats production's
/// same key as already there: a read of the key answers production's object
/// (the fallback), so a "fresh" environment copy would silently shadow it — an
/// overwrite in all but name. With `allowOverwrite` the environment writes its
/// own copy; production's object is never touched either way.
async fn refuse_shadowing(
    silo: &Silo,
    key: &str,
    allow_overwrite: bool,
) -> Result<(), StorageError> {
    let Some(production) = silo.fallback().filter(|_| !allow_overwrite) else {
        return Ok(());
    };
    let relative = key.strip_prefix(silo.prefix().as_str()).unwrap_or(key);
    if head_key(&production.key(relative)).await?.is_some() {
        return Err(StorageError::AlreadyExists(format!(
            "'{relative}' already exists in production's storage, which this environment \
             reads through; pass allowOverwrite to write this environment's own copy"
        )));
    }
    Ok(())
}

// ── Server-side asset operations (S3 or local filesystem) ─────────────────────

/// Write a **generated** asset into `silo`. Takes raw bytes, so binary output
/// (PDF, PNG, Parquet) is first-class rather than text-only.
pub async fn put(
    silo: &Silo,
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
    let key = normalize_in(silo, pathname, opts.add_random_suffix)?;
    refuse_shadowing(silo, &key, opts.allow_overwrite).await?;
    let content_type = opts
        .content_type
        .clone()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| guess_content_type(&key).to_string());
    let size = body.len() as u64;
    match bucket() {
        Some(bucket) => {
            let tagging = retention_tag_in(silo, &key, retention);
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

/// Read a small asset back: the silo's copy, else (outside production)
/// production's same key. `Ok(None)` when neither exists.
pub async fn get(
    silo: &Silo,
    key: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>, StorageError> {
    for candidate in read_candidates(silo, key)? {
        if let Some(found) = get_key(&candidate).await? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Metadata without the body, looked up like [`get`]. The returned `key` says
/// which silo answered. `Ok(None)` when absent.
pub async fn head(silo: &Silo, key: &str) -> Result<Option<StoredObject>, StorageError> {
    for candidate in read_candidates(silo, key)? {
        if let Some(found) = head_key(&candidate).await? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// One page of `silo`'s assets — that silo alone, never merged with
/// production's. Bounded and cursor-paginated: a silo with 100k objects must
/// not turn one call into an unbounded walk.
pub async fn list(
    silo: &Silo,
    sub_prefix: Option<&str>,
    limit: Option<usize>,
    cursor: Option<String>,
) -> Result<ListPage, StorageError> {
    let prefix = super::list_prefix_in(silo, sub_prefix)?;
    list_raw(&prefix, limit.unwrap_or(DEFAULT_LIST_LIMIT), cursor).await
}

/// One page under any `prefix` — the metering walk's reader, which spans every
/// silo of an app. Not for a caller-chosen prefix.
pub(super) async fn list_raw(
    prefix: &str,
    limit: usize,
    cursor: Option<String>,
) -> Result<ListPage, StorageError> {
    let limit = limit.clamp(1, MAX_LIST_LIMIT);
    match bucket() {
        Some(bucket) => s3::list(&bucket, prefix, limit, cursor).await,
        None => local::list(prefix, limit, cursor),
    }
}

/// Delete one or more assets of `silo`. A production key named outside
/// production is re-rooted, so this never deletes a production object from
/// another environment. Idempotent: deleting an absent key is not an error.
/// Returns the number of keys **accepted** for deletion, not a count of keys
/// that existed — deletion is a no-op success for an absent key, and S3 can't
/// cheaply report prior existence, so both backends count an absent key as
/// deleted.
pub async fn delete(silo: &Silo, keys: &[String]) -> Result<usize, StorageError> {
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
        .map(|k| validate_in(silo, k))
        .collect::<Result<_, _>>()?;
    match bucket() {
        Some(bucket) => s3::delete(&bucket, &validated).await,
        None => local::delete(&validated).await,
    }
}

/// Delete keys already resolved to one silo — the sweeper's expiry, which
/// selected them from a listing. Chunked to the per-call limit.
pub(super) async fn delete_raw(keys: &[String]) -> Result<usize, StorageError> {
    let mut deleted = 0;
    for chunk in keys.chunks(MAX_LIST_LIMIT) {
        deleted += match bucket() {
            Some(bucket) => s3::delete(&bucket, chunk).await?,
            None => local::delete(chunk).await?,
        };
    }
    Ok(deleted)
}

/// Server-side copy into `silo` — no bytes through the isolate. The source is
/// read like [`get`] (outside production, production's object when the
/// environment has no copy); the destination is always in `silo`.
pub async fn copy(
    silo: &Silo,
    from_key: &str,
    to_pathname: &str,
    allow_overwrite: bool,
) -> Result<PutResult, StorageError> {
    let from = first_existing(read_candidates(silo, from_key)?).await?;
    let to = normalize_in(silo, to_pathname, false)?;
    refuse_shadowing(silo, &to, allow_overwrite).await?;
    if from == to {
        return Err(StorageError::Invalid(
            "copy source and destination are the same key".to_string(),
        ));
    }
    match bucket() {
        Some(bucket) => s3::copy(&bucket, &from, &to, allow_overwrite).await?,
        None => local::copy(&from, &to, allow_overwrite).await?,
    }
    let meta = head_key(&to).await?;
    Ok(PutResult {
        key: to.clone(),
        size: meta.as_ref().map(|m| m.size.max(0) as u64).unwrap_or(0),
        content_type: meta
            .and_then(|m| m.content_type)
            .unwrap_or_else(|| guess_content_type(&to).to_string()),
    })
}

/// Delete every asset belonging to an app — production's silo and every
/// environment's — used when the app itself is deleted so its bytes don't
/// outlive it.
pub async fn delete_app_assets(app_id: Uuid) -> Result<(), StorageError> {
    for root in super::environments::silo_roots(app_id).await? {
        match bucket() {
            Some(bucket) => s3::delete_prefix(&bucket, &root).await?,
            None => local::delete_prefix(&root).await?,
        }
    }
    Ok(())
}
