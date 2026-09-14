//! Where a document's bytes live.
//!
//! The same object store and the same signing code the custom-app asset store
//! uses, under a different prefix: `org-documents/{org_id}/{document_id}/{n}`.
//! Reused rather than copied — two presigners drift, and the one that drifts is
//! always the one without the tests.
//!
//! # Why not a per-app silo
//!
//! `customer-app-storage/{app_id}/` is a hard boundary, and correctly so: an
//! app's uploads are its own. A document is not. The Knowledge base and
//! Compliance are plausibly two apps over one library, the analytics agent has
//! to be able to index it, and an org that removes an app must not lose its
//! health permits with it. Filing org assets inside one app's silo assigns the
//! wrong owner and makes every other reader impossible.
//!
//! # Why no caller string reaches the key
//!
//! Every segment is a uuid or an integer this server chose. There is no
//! filename, no folder path, and therefore no traversal to validate against and
//! no collision to suffix around — the properties `normalize_pathname` has to
//! work for on the app path come free here because the shape is narrower.

use std::time::Duration;

use uuid::Uuid;

use crate::server::api::custom_apps_storage::{StorageError, bucket, s3};

/// Long enough for a browser to finish an upload it has already started, short
/// enough that a link captured from the network tab is stale by the time it is
/// used somewhere else.
const UPLOAD_TTL: Duration = Duration::from_secs(900);
/// Downloads are a redirect the reader follows immediately, so this is shorter
/// than the app store's hour — that hour exists for links pasted into email,
/// which is not what a redirect from the document reader is.
const DOWNLOAD_TTL: Duration = Duration::from_secs(300);

/// Ceiling on one uploaded document, bound into the signature so the object
/// store itself rejects a larger body rather than trusting the browser.
pub const MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024;

/// The object key for one version. Total function: nothing here can fail and
/// nothing here is caller-supplied.
pub fn object_key(org_id: Uuid, document_id: Uuid, version_no: i32) -> String {
    format!("org-documents/{org_id}/{document_id}/{version_no}")
}

fn require_bucket() -> Result<String, StorageError> {
    bucket().ok_or_else(|| {
        StorageError::NotConfigured(
            "OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET is unset, so document uploads cannot be signed"
                .to_string(),
        )
    })
}

/// Mint the PUT the browser uploads to.
///
/// Content type and length are part of the signature, so a client that sends a
/// different type or a bigger body is refused by the object store — the ceiling
/// is enforced there, not by trusting what the browser said it would send.
pub async fn upload_url(
    org_id: Uuid,
    document_id: Uuid,
    version_no: i32,
    content_type: &str,
    content_length: u64,
) -> Result<(String, String), StorageError> {
    if content_length == 0 {
        return Err(StorageError::Invalid(
            "contentLength must be greater than 0".to_string(),
        ));
    }
    if content_length > MAX_UPLOAD_BYTES {
        return Err(StorageError::TooLarge(format!(
            "upload of {content_length} bytes exceeds the {MAX_UPLOAD_BYTES}-byte ceiling"
        )));
    }
    let bucket = require_bucket()?;
    let key = object_key(org_id, document_id, version_no);
    // No retention tag, unlike an app asset. A document is kept until somebody
    // deletes it; a sweeper that expired SOPs on a TTL would be a data-loss bug
    // wearing a lifecycle rule's clothes.
    let url = s3::presign_put(
        &bucket,
        &key,
        content_type,
        content_length,
        UPLOAD_TTL,
        None,
    )
    .await?;
    Ok((url, key))
}

/// Mint the GET the reader is redirected to.
///
/// `filename` is the document's title, so the file that lands in Downloads is
/// named after what the reader clicked rather than after a uuid.
pub async fn download_url(key: &str, filename: &str) -> Result<String, StorageError> {
    let bucket = require_bucket()?;
    s3::presign_get(
        &bucket,
        key,
        DOWNLOAD_TTL,
        Some(sanitize_filename(filename)),
    )
    .await
}

/// A title is user input and this one ends up in a `Content-Disposition`
/// header, so it is reduced to something that cannot carry a header break or a
/// path. `presign_get` also strips quotes; this is the layer that keeps a
/// newline out, which quote-stripping alone would not.
fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Truncate the STEM, never the extension. The caller appends one — that is
    // what `handlers::with_extension` is for — so truncating the whole string
    // cut it back off for any title over the ceiling, and the download landed
    // as an extensionless blob: the exact failure the extension exists to
    // prevent, reintroduced by the length cap beneath it.
    let (stem, ext) = match cleaned.rsplit_once('.') {
        // A short, plausible extension. A dot in the middle of prose is not one.
        Some((s, e)) if !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_alphanumeric()) => {
            (s, Some(e))
        }
        _ => (cleaned.as_str(), None),
    };

    let room = MAX_FILENAME - ext.map_or(0, |e| e.len() + 1);
    let mut out: String = stem.chars().take(room).collect();
    out = out.trim().trim_matches('.').to_string();
    if out.is_empty() {
        out = "document".to_string();
    }
    match ext {
        Some(e) => format!("{out}.{e}"),
        None => out,
    }
}

/// Characters a `Content-Disposition` filename is capped at.
const MAX_FILENAME: usize = 120;

#[cfg(test)]
mod filename_tests {
    use super::sanitize_filename;

    #[test]
    fn a_long_title_keeps_its_extension() {
        let long = format!("{}.pdf", "Health Permit Renewal ".repeat(12));
        let out = sanitize_filename(&long);
        assert!(
            out.ends_with(".pdf"),
            "the length cap cut off the extension: {out}"
        );
        assert!(out.chars().count() <= 120);
    }

    #[test]
    fn a_dot_in_prose_is_not_an_extension() {
        // Truncating here must not treat "5 percent" as a suffix to protect.
        let out = sanitize_filename("Cooling curve v1.5 percent tolerance");
        assert_eq!(out, "Cooling curve v1.5 percent tolerance");
    }

    #[test]
    fn the_ordinary_cases_are_unchanged() {
        assert_eq!(
            sanitize_filename("Health Permit 2026.pdf"),
            "Health Permit 2026.pdf"
        );
        assert_eq!(sanitize_filename("  ...  "), "document");
        assert_eq!(sanitize_filename("Board pack / Q3"), "Board pack - Q3");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_built_only_from_ids() {
        let org = Uuid::from_u128(1);
        let doc = Uuid::from_u128(2);
        assert_eq!(
            object_key(org, doc, 3),
            format!("org-documents/{org}/{doc}/3")
        );
    }

    #[test]
    fn a_hostile_title_cannot_shape_the_download_header() {
        // A header break, a path separator, and a quote — the three ways a
        // title could otherwise reach past the filename it is supposed to be.
        // CR and LF become hyphens, so no second header can be injected.
        assert_eq!(
            sanitize_filename("evil\r\nSet-Cookie: x=1"),
            "evil--Set-Cookie- x-1"
        );
        // Slashes go too, so a title cannot describe a path. The leading dots
        // are trimmed, which is what stops a name the object store or a client
        // would read as relative.
        assert_eq!(sanitize_filename("../../etc/passwd"), "-..-etc-passwd");
        assert_eq!(sanitize_filename("\"quoted\""), "-quoted-");
    }

    #[test]
    fn an_unnameable_title_still_produces_a_filename() {
        assert_eq!(sanitize_filename("   "), "document");
        assert_eq!(sanitize_filename("..."), "document");
    }
}

/// Did the bytes actually land?
///
/// Called before a version is made current. Without it "confirm" is a claim the
/// client makes about an upload nobody checked, and the failure it produces is
/// the worst kind: a published document that answers `404` when somebody
/// finally opens it, long after the person who uploaded it stopped watching.
pub async fn object_exists(key: &str) -> Result<bool, StorageError> {
    let bucket = require_bucket()?;
    Ok(s3::head(&bucket, key).await?.is_some())
}

/// One mapping from a storage failure to a status, so the upload path and the
/// download path cannot disagree about what "not configured" means to a client.
///
/// `NotConfigured` is `503` rather than `500`: the deployment is missing a
/// bucket, which is an environment fact an operator can fix, and a `500` sends
/// them reading application logs for a configuration problem.
pub fn status_for(e: &StorageError) -> axum::http::StatusCode {
    use axum::http::StatusCode;
    match e {
        StorageError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        StorageError::Invalid(_) => StatusCode::BAD_REQUEST,
        StorageError::NotConfigured(_) => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
