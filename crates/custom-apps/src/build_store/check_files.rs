//! The files a build's promote checks keep: each screen's picture on staging
//! and on production, the diff between them, a changed screen's visible text
//! and the build's CSS digest (`oxy-api-custom-apps`
//! `custom_apps_build_checks::files`).
//!
//! **Beside the build, never inside it.** The key is
//! `customer-apps/<app_id>/checks/<build row id>/<environment>/<attempt>/<name>`:
//! the same store and the same app prefix as the build's bundle
//! (`customer-apps/<app_id>/builds/<build_id>/`), on a sibling branch. A
//! build's own file set is fixed once it is published, and the bundle cache
//! remembers absences on that assumption; a check's files arrive later, so
//! they never go under a build's prefix and are never read through that cache.
//!
//! **Bytes only, in the object store.** Postgres holds the names (in a check
//! row's `detail`), never the bytes: a picture or a page's text can show an
//! org's data.
//!
//! **Deleted with what they are about.** [`delete_build`] removes one build's
//! files when its row is pruned (`gc_builds`); the whole app's go with
//! [`super::delete_app`], which removes `customer-apps/<app_id>/` on both
//! backends.
//!
//! Two backends, picked as the build store picks them: S3 where
//! `OXY_CUSTOMER_APPS_S3_BUCKET` is set, else the filesystem under
//! `OXY_STATE_DIR` (single-node only, as a publish is).

use std::path::PathBuf;

use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;
use uuid::Uuid;

use super::{BuildStoreError, bucket, s3_client, state_root};

/// The longest file name kept.
pub const MAX_NAME_CHARS: usize = 120;

/// The two environments an attempt runs in. Any other name is not a path
/// segment this module writes.
const ENVIRONMENTS: [&str; 2] = ["staging", "production"];

/// One attempt of one build's checks: where its files live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attempt<'a> {
    pub app_id: Uuid,
    /// The build's row id (`app_builds.id`).
    pub build_pk: Uuid,
    /// `staging` or `production`.
    pub environment: &'a str,
    pub attempt: i32,
}

/// Whether `name` may be a kept file's name: `[a-z0-9._-]`, 1 to
/// [`MAX_NAME_CHARS`] characters, and not starting with a dot (so never `.`,
/// `..` or a hidden file). It becomes the last segment of an object key and of
/// a path on disk.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_CHARS
        && !name.starts_with('.')
        && name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// The prefix every file of one build's checks is under (it ends in `/`).
pub fn build_prefix(app_id: Uuid, build_pk: Uuid) -> String {
    format!("customer-apps/{app_id}/checks/{build_pk}/")
}

/// The prefix of one attempt's files (it ends in `/`). `Err` for an
/// environment that is not staging or production, or an attempt below 1.
pub fn attempt_prefix(at: &Attempt<'_>) -> Result<String, BuildStoreError> {
    if !ENVIRONMENTS.contains(&at.environment) || at.attempt < 1 {
        return Err(BuildStoreError::Io(format!(
            "no check files are kept for environment {:?}, attempt {}",
            at.environment, at.attempt
        )));
    }
    Ok(format!(
        "{}{}/{}/",
        build_prefix(at.app_id, at.build_pk),
        at.environment,
        at.attempt
    ))
}

fn key_of(at: &Attempt<'_>, name: &str) -> Result<String, BuildStoreError> {
    if !is_valid_name(name) {
        return Err(BuildStoreError::Io(format!(
            "unsafe check file name: {name:?}"
        )));
    }
    Ok(format!("{}{name}", attempt_prefix(at)?))
}

fn fs_path(key: &str) -> PathBuf {
    state_root().join(key)
}

/// Keep `bytes` as `name` of `at`'s files, replacing a file of that name.
pub async fn put(at: &Attempt<'_>, name: &str, bytes: Vec<u8>) -> Result<(), BuildStoreError> {
    let key = key_of(at, name)?;
    // As a publish: the filesystem is this node's disk, so on a deployment of
    // several nodes a file kept here would be missing on the next request.
    if bucket().is_none()
        && oxy_app_core::process_role::current_process_role()
            != oxy_app_core::process_role::Role::All
    {
        return Err(BuildStoreError::Io(
            "check files need a shared object store on a multi-instance deployment: set \
             OXY_CUSTOMER_APPS_S3_BUCKET (the filesystem backend is single-node only)"
                .to_string(),
        ));
    }
    match bucket() {
        Some(bucket) => {
            s3_client()
                .await
                .put_object()
                .bucket(&bucket)
                .key(&key)
                .body(ByteStream::from(bytes))
                .send()
                .await
                .map_err(|e| BuildStoreError::S3(format!("put_object {key}: {e}")))?;
        }
        None => {
            let dest = fs_path(&key);
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| BuildStoreError::Io(format!("mkdir {}: {e}", parent.display())))?;
            }
            tokio::fs::write(&dest, bytes)
                .await
                .map_err(|e| BuildStoreError::Io(format!("write {}: {e}", dest.display())))?;
        }
    }
    Ok(())
}

/// The file `name` of `at`'s files; `Ok(None)` when there is none (an invalid
/// name names none).
pub async fn get(at: &Attempt<'_>, name: &str) -> Result<Option<Bytes>, BuildStoreError> {
    let Ok(key) = key_of(at, name) else {
        return Ok(None);
    };
    match bucket() {
        Some(bucket) => {
            let client = s3_client().await;
            match client.get_object().bucket(&bucket).key(&key).send().await {
                Ok(resp) => {
                    let data = resp
                        .body
                        .collect()
                        .await
                        .map_err(|e| BuildStoreError::S3(format!("collect {key}: {e}")))?;
                    Ok(Some(data.into_bytes()))
                }
                Err(err) if err.as_service_error().is_some_and(|e| e.is_no_such_key()) => Ok(None),
                Err(err) => Err(BuildStoreError::S3(format!("get_object {key}: {err}"))),
            }
        }
        None => {
            let dest = fs_path(&key);
            match tokio::fs::read(&dest).await {
                Ok(bytes) => Ok(Some(Bytes::from(bytes))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(BuildStoreError::Io(format!("read {}: {e}", dest.display()))),
            }
        }
    }
}

/// The names of `at`'s files, in no particular order. An attempt that kept
/// none is an empty list.
pub async fn names(at: &Attempt<'_>) -> Result<Vec<String>, BuildStoreError> {
    let prefix = attempt_prefix(at)?;
    match bucket() {
        Some(bucket) => Ok(s3_keys(&bucket, &prefix)
            .await?
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
            .collect()),
        None => {
            let dir = fs_path(&prefix);
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(BuildStoreError::Io(format!("list {}: {e}", dir.display()))),
            };
            let mut names = Vec::new();
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| BuildStoreError::Io(format!("list {}: {e}", dir.display())))?
            {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            Ok(names)
        }
    }
}

/// Every key under `prefix` in the bucket.
async fn s3_keys(bucket: &str, prefix: &str) -> Result<Vec<String>, BuildStoreError> {
    let client = s3_client().await;
    let mut keys = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let mut req = client.list_objects_v2().bucket(bucket).prefix(prefix);
        if let Some(token) = &continuation {
            req = req.continuation_token(token.clone());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| BuildStoreError::S3(format!("list_objects_v2 {prefix}: {e}")))?;
        keys.extend(
            resp.contents()
                .iter()
                .filter_map(|o| o.key())
                .map(str::to_string),
        );
        continuation = resp.next_continuation_token().map(str::to_string);
        if !resp.is_truncated().unwrap_or(false) || continuation.is_none() {
            return Ok(keys);
        }
    }
}

/// Delete every check file of one build, every environment and attempt: what
/// `gc_builds` calls when it prunes the build's row. Nothing kept is no error.
pub async fn delete_build(app_id: Uuid, build_pk: Uuid) -> Result<(), BuildStoreError> {
    let prefix = build_prefix(app_id, build_pk);
    match bucket() {
        Some(bucket) => {
            let client = s3_client().await;
            for key in s3_keys(&bucket, &prefix).await? {
                client
                    .delete_object()
                    .bucket(&bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|e| BuildStoreError::S3(format!("delete_object {key}: {e}")))?;
            }
        }
        None => {
            let dir = fs_path(&prefix);
            match tokio::fs::remove_dir_all(&dir).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(BuildStoreError::Io(format!("rmdir {}: {e}", dir.display()))),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(environment: &str, attempt: i32) -> Attempt<'_> {
        Attempt {
            app_id: Uuid::from_u128(1),
            build_pk: Uuid::from_u128(2),
            environment,
            attempt,
        }
    }

    #[test]
    fn a_name_is_lower_case_letters_digits_dot_dash_and_underscore_and_never_starts_with_a_dot() {
        for good in [
            "shot-staging-stores-desk.png",
            "diff-reorder--plans-phone.png",
            "text-production-root-desk.json",
            "css-digest.json",
            "a",
            &"a".repeat(MAX_NAME_CHARS),
        ] {
            assert!(is_valid_name(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            ".hidden",
            "a/b",
            "../x.png",
            "Shot.png",
            "shot staging.png",
            "shot%2f.png",
            "x\u{e9}.png",
            &"a".repeat(MAX_NAME_CHARS + 1),
        ] {
            assert!(!is_valid_name(bad), "{bad:?}");
        }
    }

    /// The contract's key, and nowhere under a build's own prefix.
    #[test]
    fn an_attempts_files_live_beside_the_apps_builds_never_under_one() {
        let prefix = attempt_prefix(&at("staging", 2)).unwrap();
        assert_eq!(
            prefix,
            "customer-apps/00000000-0000-0000-0000-000000000001/checks/\
             00000000-0000-0000-0000-000000000002/staging/2/"
        );
        assert_eq!(
            key_of(&at("production", 1), "shot.png").unwrap(),
            "customer-apps/00000000-0000-0000-0000-000000000001/checks/\
             00000000-0000-0000-0000-000000000002/production/1/shot.png"
        );
        let builds = super::super::build_prefix(Uuid::from_u128(1), "b-1");
        assert!(builds.starts_with("customer-apps/00000000-0000-0000-0000-000000000001/builds/"));
        assert!(!prefix.starts_with("customer-apps/00000000-0000-0000-0000-000000000001/builds/"));
        // The app's own prefix holds both, so deleting the app deletes both.
        assert!(prefix.starts_with("customer-apps/00000000-0000-0000-0000-000000000001/"));
    }

    #[test]
    fn a_sandbox_an_attempt_below_one_and_an_unsafe_name_have_no_key() {
        assert!(attempt_prefix(&at("dev-a1", 1)).is_err());
        assert!(attempt_prefix(&at("../staging", 1)).is_err());
        assert!(attempt_prefix(&at("staging", 0)).is_err());
        assert!(key_of(&at("staging", 1), "../../x").is_err());
        assert!(key_of(&at("staging", 1), "..").is_err());
    }
}
