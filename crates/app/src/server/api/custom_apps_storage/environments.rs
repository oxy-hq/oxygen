//! Every silo an app has — production's and each environment's — for the two
//! walks that must see all of them: usage metering and app deletion
//! (environments design §4.3, "Retention"). A silo left out of either would be
//! bytes nobody is billed for, or bytes that outlive the app.

use uuid::Uuid;

use super::{Silo, StorageError, bucket, local, silo};

/// The prefixes that together hold every object of `app_id`.
///
/// Production's silo, then the environments'. On the object store the latter
/// is one raw prefix, `customer-app-storage/<app_id>~`, which S3 lists and
/// deletes by directly. The filesystem store has no raw prefixes — a prefix is
/// a directory — so there each environment's silo that exists on disk is its
/// own entry.
pub(super) async fn silo_roots(app_id: Uuid) -> Result<Vec<String>, StorageError> {
    let mut roots = vec![Silo::production(app_id).prefix()];
    match bucket() {
        Some(_) => roots.push(silo::environment_silos_prefix(app_id)),
        None => roots.extend(local_environment_silos(app_id).await?),
    }
    Ok(roots)
}

/// Each `customer-app-storage/<app_id>~<env>/` directory on disk.
async fn local_environment_silos(app_id: Uuid) -> Result<Vec<String>, StorageError> {
    let raw = silo::environment_silos_prefix(app_id);
    let Some((root, stem)) = raw.rsplit_once('/') else {
        return Ok(Vec::new());
    };
    let mut silos: Vec<String> = local::child_dirs(root)
        .await?
        .into_iter()
        .filter(|name| name.starts_with(stem) && name.len() > stem.len())
        .map(|name| format!("{root}/{name}/"))
        .collect();
    silos.sort();
    Ok(silos)
}
