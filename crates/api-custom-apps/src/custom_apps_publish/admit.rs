//! The admit stage: resolve the org, authorize, and refuse what can be
//! refused before the expensive work — a malformed or reused build id, a bad
//! slug, a publish that names another workspace, a publisher past its hourly
//! budget, a new app past the org's cap. Nothing is written.

use entity::{apps, organizations};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{
    PublishError, PublishInput, PublishTarget, authorize_publish, build_id_taken,
    ensure_same_workspace, find_app, limits, org_for_project, resolve_org, sandbox_publish, store,
    validate_project,
};

/// What admit decided: the org, the app row as it was, and — for a sandbox
/// publish — the app the sandbox belongs to.
pub(super) struct Admitted {
    pub org: organizations::Model,
    pub existing_app: Option<apps::Model>,
    pub sandbox_app: Option<Uuid>,
}

pub(super) async fn admit(
    db: &DatabaseConnection,
    input: &PublishInput,
    target: &PublishTarget,
) -> Result<Admitted, PublishError> {
    let org = resolve_publish_org(db, input).await?;
    authorize_publish(db, &org, input).await?;
    limits::refuse_past_publish_rate(org.id, input)?;
    // A semantic pin is staging-only and must name a ready revision of the
    // workspace this publish targets (which `ensure_same_workspace` below
    // holds to the app's own). Checked before the bundle is inflated.
    if let Some(revision_id) = input.semantic_revision_id {
        crate::custom_apps_staging_pin::validate_pin_for_publish(
            db,
            input.project_id,
            revision_id,
            input.promote,
        )
        .await
        .map_err(PublishError::SemanticPin)?;
    }
    refuse_malformed(input)?;
    let existing_app = find_app(db, org.id, &input.app_slug).await?;
    let sandbox_app =
        sandbox_publish::admit_target(db, input, existing_app.as_ref(), target).await?;
    if existing_app.is_none() {
        limits::refuse_new_app_past_cap(db, org.id).await?;
    }
    if let Some(app) = &existing_app {
        // The same fast-path reasoning for a publish that names another
        // workspace or reuses a build id: refused before the bundle is
        // inflated. `upsert_app` and the store stage re-check, and are the
        // authoritative ones.
        ensure_same_workspace(db, app, input).await?;
        if build_id_taken(db, app.id, &input.build_id).await? {
            return Err(PublishError::DuplicateBuild {
                app_slug: input.app_slug.clone(),
                build_id: input.build_id.clone(),
            });
        }
    }
    Ok(Admitted {
        org,
        existing_app,
        sandbox_app,
    })
}

/// When the publisher pinned only a workspace (`--project` without `--org`),
/// infer the org from it — a workspace belongs to exactly one org. When an
/// org IS given, keep the cross-org guard: it must match the workspace's org.
async fn resolve_publish_org(
    db: &DatabaseConnection,
    input: &PublishInput,
) -> Result<organizations::Model, PublishError> {
    match &input.org_ref {
        Some(org_ref) => {
            let org = resolve_org(db, org_ref).await?;
            validate_project(db, input.project_id, org.id, &org.slug).await?;
            Ok(org)
        }
        None => org_for_project(db, input.project_id).await,
    }
}

/// A malformed build id or slug, before the pipeline pays for the inflate,
/// the validation and the compression only to refuse. The slug is a raw
/// multipart field: it feeds the OLTP writer derivation (`app_writer_name`),
/// a `repo_path` segment and the served base path, so a bad one never reaches
/// the row, the path or the store.
fn refuse_malformed(input: &PublishInput) -> Result<(), PublishError> {
    if !store::is_valid_build_id(&input.build_id) {
        return Err(PublishError::InvalidBuildId(input.build_id.clone()));
    }
    if !crate::admin::apps::is_valid_slug(&input.app_slug) {
        return Err(PublishError::InvalidSlug(input.app_slug.clone()));
    }
    Ok(())
}
