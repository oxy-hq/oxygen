//! The fixture's release history, made by the real code paths, in the seed
//! process: five channel publishes (`publish_to`, which admits, inflates,
//! validates, stores, records the build and its functions, moves staging and
//! reconciles staging's schedules), each followed by the publish audit row the
//! HTTP handler writes, and four promotes through the plan routes' own act
//! (`custom_apps_promote::act::promote`: plan, blockers, the one production
//! function, production's schedule reconcile, the audit row).
//!
//! The actor is the seeded App Operator — publish resolves an unscoped
//! operator as staff. Sequence: r1, r2, r3 and r4 each published and promoted;
//! r5 published to staging only. Production ends on r4, the bad release.

use entity::{app_builds, organizations};
use oxy_app_core::audit::RequestActor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::bundle::Release;
use super::calendar::LIVE;
use super::{APP_SLUG, FixtureError};
use crate::custom_apps_promote::act;
use crate::custom_apps_promote::resolve::PromoteRequest;
use crate::custom_apps_publish::{OrgRef, PublishInput, PublishTarget, publish_to};
use crate::custom_apps_publish_audit;

/// Publish every release and promote the first [`LIVE`]; the `app_builds.id`
/// of each, in release order.
pub async fn run(
    db: &DatabaseConnection,
    operator: &AuthenticatedUser,
    org: &organizations::Model,
    workspace_id: Uuid,
    app_id: Uuid,
    releases: &[Release],
) -> Result<Vec<Uuid>, FixtureError> {
    let actor = RequestActor::session(operator.clone());
    let mut builds = Vec::with_capacity(releases.len());
    let mut production: Option<Uuid> = None;
    for release in releases {
        let input = input(operator, org, workspace_id, release);
        let published = publish_to(input, PublishTarget::Channels)
            .await
            .map_err(|e| FixtureError::step(format!("publish {}", release.label()), e))?;
        custom_apps_publish_audit::published(db, &actor, None, &published).await;
        let build = build_pk(db, app_id, &release.build_id).await?;
        builds.push(build);
        if release.index > LIVE {
            continue;
        }
        let request = PromoteRequest {
            from: "staging".to_string(),
            build: Some(release.build_id.clone()),
            expect_current: Some(production),
            ..Default::default()
        };
        act::promote(db, &actor, app_id, "production", &request)
            .await
            .map_err(|e| FixtureError::step(format!("promote {}", release.label()), e))?;
        production = Some(build);
    }
    Ok(builds)
}

fn input(
    operator: &AuthenticatedUser,
    org: &organizations::Model,
    workspace_id: Uuid,
    release: &Release,
) -> PublishInput {
    PublishInput {
        org_ref: Some(OrgRef::Id(org.id)),
        app_slug: APP_SLUG.to_string(),
        project_id: workspace_id,
        branch: None,
        build_id: release.build_id.clone(),
        name: None,
        promote: false,
        tarball: release.tarball(),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(operator.id),
        published_by_email: operator.email.clone(),
        publisher: Some(oxy_server_authz::Caller::from_user(operator)),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
        confirm_schedule_removals: Vec::new(),
    }
}

async fn build_pk(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_id: &str,
) -> Result<Uuid, FixtureError> {
    app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::BuildId.eq(build_id))
        .one(db)
        .await
        .map_err(|e| FixtureError::step("read the published build", e))?
        .map(|b| b.id)
        .ok_or_else(|| {
            FixtureError::step("read the published build", format!("no build {build_id}"))
        })
}
