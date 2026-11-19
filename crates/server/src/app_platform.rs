//! The platform work behind a custom app's approved actions
//! (`internal-docs/proposals/app-platform-access.md`), installed at boot in
//! every process that runs functions.
//!
//! The function runtime decides each call before it gets here: the manifest
//! asked for it, an org admin approved it, and `oxy-authz` allows it for the
//! app's service account. This only forwards to each context's own code (the
//! functions the HTTP routes call), because it is the one crate that may see
//! both `oxy-api-frontline` and `oxy-api-airway`
//! (`internal-docs/domain-boundaries.md`, S3).

use std::future::Future;
use std::pin::Pin;

use oxy_api_frontline::frontline_devices::app_ops as kiosks;
use oxy_app_core::app_platform_seam::{
    AppPlatformActions, FrontlineDevicesOp, PlatformActionError, PlatformFuture, ResolvedPipeline,
    SourceReport,
};
use sea_orm::DatabaseConnection;
use sea_orm::prelude::Uuid;

/// `oxy-server`'s implementation: the kiosk and upload code, unchanged.
pub(crate) struct ServerAppPlatform;

impl AppPlatformActions for ServerAppPlatform {
    fn frontline_devices<'a>(
        &'a self,
        db: &'a DatabaseConnection,
        org_id: Uuid,
        op: FrontlineDevicesOp,
        actor_user_id: Uuid,
    ) -> PlatformFuture<'a> {
        Box::pin(async move {
            match op {
                FrontlineDevicesOp::List => kiosks::list(db, org_id).await,
                FrontlineDevicesOp::Create {
                    name,
                    location_id,
                    return_to,
                    idle_timeout_seconds,
                } => {
                    kiosks::create(
                        db,
                        org_id,
                        &name,
                        location_id,
                        return_to.as_deref(),
                        idle_timeout_seconds,
                        actor_user_id,
                    )
                    .await
                }
                FrontlineDevicesOp::ReissueLink { id } => kiosks::reissue(db, org_id, id).await,
                FrontlineDevicesOp::Revoke { id } => kiosks::revoke(db, org_id, id).await,
            }
        })
    }

    fn resolve_pipeline<'a>(
        &'a self,
        db: &'a DatabaseConnection,
        org_id: Uuid,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedPipeline, PlatformActionError>> + Send + 'a>>
    {
        Box::pin(oxy_api_airway::app_ops::resolve(db, org_id, name))
    }

    fn source_upload_report<'a>(
        &'a self,
        pipeline: ResolvedPipeline,
        report: SourceReport,
    ) -> PlatformFuture<'a> {
        Box::pin(oxy_api_airway::app_ops::report(pipeline, report))
    }
}
