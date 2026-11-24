//! What a function may do: the fail-closed capability gates its manifest
//! entry and its build grant, the same for a route call and a queued run.
//! Split out of `custom_apps_functions/mod.rs` so that file stops growing.

use super::*;

/// The fail-closed capability gates `entry` grants, the same for a route call
/// and a queued run.
#[cfg(feature = "custom-app-functions")]
pub(super) fn manifest_capabilities(
    entry: &FunctionManifestEntry,
    app: &entity::apps::Model,
    build: &entity::app_builds::Model,
) -> host::FunctionCapabilities {
    // A kind is critical when the function says so, or when the build's
    // notification catalogue gives it the critical level.
    let catalogue =
        oxy_app_core::notifications::catalog::parse::catalogue(build.manifest_json.as_ref());
    let notify_kinds = entry.notify_kinds().map(|mut kinds| {
        for (kind, critical) in kinds.iter_mut() {
            *critical |= catalogue.kinds.get(kind).and_then(|k| k.level)
                == Some(oxy_app_core::notifications::catalog::model::Level::Critical);
        }
        kinds
    });
    let publishes = entry.publishes();
    // Declared by the build (top-level `calls`), for every function of the app;
    // checked when the build was published, so one that does not parse here
    // declares nothing.
    let calls = oxy_custom_apps::calls::DeclaredCalls::from_manifest(build.manifest_json.as_ref())
        .unwrap_or_default();
    host::FunctionCapabilities {
        secrets_write: entry.secrets_write(),
        email_send: entry.email_send(),
        notify_kinds,
        org_read: entry.org_read(),
        platform_facts: entry.platform_facts(),
        frontline_devices: entry
            .platform_actions()
            .contains(&oxy_authz::Action::FrontlineDeviceManage),
        source_uploads: entry
            .platform_actions()
            .contains(&oxy_authz::Action::SourceUploadReport),
        source_upload_pipelines: entry
            .oxygen
            .as_ref()
            .map(oxy_custom_apps::platform_access::PlatformActionSpecs::pipelines)
            .unwrap_or_default(),
        storage_read: entry.storage_read(),
        storage_write: entry.storage_write(),
        // DERIVED from the invoking app's slug, gated by the manifest — never
        // named by the manifest, so one app cannot reach another's schema.
        // A slug that can't back a schema is a distinct fail-closed reason so
        // the host diagnoses it as such, not as "capability missing".
        oltp: host::WriterCapability::resolve(entry.oltp_enabled(), &app.slug),
        // Declared by the build (top-level `oltpChanges`), for every function
        // of the app: a production `ctx.oltp` transaction is then signed,
        // flag or not (`oltp_changes::signs` says why).
        oltp_changes: crate::oltp_changes::signs(build.manifest_json.as_ref()),
        airhouse: host::WriterCapability::resolve(entry.airhouse_enabled(), &app.slug),
        events: (!publishes.is_empty()).then(|| publishes.iter().cloned().collect()),
        // A send is written as the app's events writer too.
        events_writer: host::WriterCapability::resolve(
            !publishes.is_empty() || !calls.is_empty(),
            &app.slug,
        ),
        calls,
        event_catalog: event_catalog_for(publishes.is_empty(), build),
        customer_warehouse_writes: entry.customer_warehouse_writes(),
        fetch_max_bytes: resolve_fetch_max_bytes(entry),
        storage_retention: crate::custom_apps_manifest::retention_policy_from_build_manifest(
            build.manifest_json.as_ref(),
            app.id,
        ),
    }
}

/// The events the build declares, read only for a function that publishes.
/// Checked when the build was published (`app_events::declared_events`), so a
/// catalogue that does not parse here declares nothing.
#[cfg(feature = "custom-app-functions")]
fn event_catalog_for(
    publishes_nothing: bool,
    build: &entity::app_builds::Model,
) -> oxy_custom_apps::events::catalog::EventCatalog {
    if publishes_nothing {
        return Default::default();
    }
    oxy_custom_apps::events::catalog::EventCatalog::from_manifest(build.manifest_json.as_ref())
        .unwrap_or_default()
}
