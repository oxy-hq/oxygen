//! The inflate stage: decompress and check the bundle, and lift out of it
//! everything later stages need — the manifest, the functions, the declared
//! migrations, the entry bytes to warm the cache with — before `files` moves
//! into the store. Nothing is written.

use axum::body::Bytes;
use sea_orm::DatabaseConnection;

use super::admit::Admitted;
use super::{
    PublishError, PublishInput, asset_manifest, check_declared_functions,
    check_notification_catalogue, check_widgets, function_specs, migrations,
    parse_embedded_manifest, precompress, staging, unpack_tar_gz,
};

/// The bundle, inflated and checked.
pub(super) struct Inflated {
    pub files: Vec<(String, Vec<u8>)>,
    pub index_bytes: Option<Bytes>,
    pub manifest_json: Option<serde_json::Value>,
    pub fn_specs: Vec<(String, serde_json::Value)>,
    pub declared_migrations: Vec<migrations::DeclaredMigration>,
    pub declared_airhouse: Vec<migrations::DeclaredMigration>,
    pub asset_manifest: asset_manifest::AssetManifest,
    pub seed_entries: Vec<(String, Bytes)>,
    /// The publish's own warnings, in the order they are answered.
    pub warnings: Vec<String>,
}

type Files = Vec<(String, Vec<u8>)>;

/// See the module docs. `input.tarball` is taken: it is not used afterwards.
pub(super) async fn inflate(
    db: &DatabaseConnection,
    input: &mut PublishInput,
    admitted: &Admitted,
) -> Result<Inflated, PublishError> {
    let files = unpack_off_runtime(std::mem::take(&mut input.tarball)).await?;
    // Fast deploy validation (gate 1): the known blank-screen causes as an
    // actionable 422 BEFORE storing the build.
    crate::custom_apps_validate::validate_bundle(&files, &admitted.org.slug, &input.app_slug)
        .map_err(PublishError::Invalid)?;
    let index_bytes = files
        .iter()
        .find(|(p, _)| p == "index.html")
        .map(|(_, b)| Bytes::from(b.clone()));
    // The bundle's own oxy-app.json is captured into the build row so the
    // manifest resolver reads it from the DB; the multipart `manifest` field
    // is the fallback. A present-but-unparseable oxy-app.json is a hard error,
    // never a silent function-less publish.
    let manifest_json = parse_embedded_manifest(&files)?.or_else(|| input.manifest.clone());
    // Every declared function must be validly named and ship its bundled JS,
    // checked before the row or a single byte is written.
    let fn_specs = function_specs(manifest_json.as_ref());
    check_declared_functions(&files, &fn_specs)?;
    let catalogue_warnings = check_notification_catalogue(manifest_json.as_ref())?;
    check_widgets(manifest_json.as_ref())?;
    // A `shared` env key falls back to production's value outside production,
    // so it may not be one an environment must hold its own of.
    crate::custom_apps_secrets::shared_env::check_publish(
        db,
        admitted.existing_app.as_ref(),
        manifest_json.as_ref(),
        &fn_specs,
        &files,
    )
    .await?;
    // Advisory, never a gate: an oversized card image only slows the home page.
    let art_warning =
        crate::custom_apps_validate::oversized_art_warning(&files, manifest_json.as_ref());
    // Declared on a draft too, so the author learns a wrong directory on the
    // publish that carries it, not on a promote weeks later.
    let declared_migrations = migrations::declare(manifest_json.as_ref(), &files)?;
    let declared_airhouse =
        migrations::declare_airhouse(manifest_json.as_ref(), &files, &input.app_slug)?;
    // Where staging's warehouse writes land: an author error refuses the
    // publish, nothing staging does fails a promoting one.
    let mapping_warning =
        staging::mapping_warning(input.project_id, manifest_json.as_ref(), input.promote)
            .await
            .map_err(PublishError::DestinationMapping)?;
    let (files, asset_manifest, seed_entries) =
        with_platform_assets(files, &input.build_id, manifest_json.as_ref()).await?;
    let warnings = art_warning
        .into_iter()
        .chain(mapping_warning)
        .chain(catalogue_warnings)
        .collect();
    Ok(Inflated {
        files,
        index_bytes,
        manifest_json,
        fn_specs,
        declared_migrations,
        declared_airhouse,
        asset_manifest,
        seed_entries,
        warnings,
    })
}

/// Inflate off the async runtime: even bounded, the unpack is CPU-bound and
/// would park a Tokio worker. The request's Sentry hub rides along so a
/// panic is captured on the request's scope, not the blocking pool's — and
/// that hub is deliberately NOT custom-app-tagged: a panic in Oxy's own
/// unpacker is a platform bug.
async fn unpack_off_runtime(tarball: Vec<u8>) -> Result<Files, PublishError> {
    let hub = sentry::Hub::current();
    tokio::task::spawn_blocking(move || sentry::Hub::run(hub, || unpack_tar_gz(&tarball)))
        .await
        .map_err(|e| PublishError::BadTarball(format!("bundle decompression task failed: {e}")))?
}

type Assets = (Files, asset_manifest::AssetManifest, Vec<(String, Bytes)>);

/// Reserve the bundle's `__oxy/` namespace with the platform's asset
/// manifest, keep the entry bytes aside to warm the cache with, and emit the
/// `.br` siblings (`custom_apps_precompress`) — on the blocking pool, like
/// the unpack. Peak resident bytes is the unpacked bundle plus its
/// compressible subset in brotli form (~1.3x `MAX_DECOMPRESSED_BYTES`): the
/// cap bounds what an uploader can make us hold, and these bytes are ours.
async fn with_platform_assets(
    mut files: Files,
    build_id: &str,
    manifest_json: Option<&serde_json::Value>,
) -> Result<Assets, PublishError> {
    let manifest = asset_manifest::install_into(&mut files, build_id, manifest_json);
    let seed_entries: Vec<(String, Bytes)> = manifest
        .entries
        .iter()
        .filter_map(|entry| {
            files
                .iter()
                .find(|(path, _)| path.trim_start_matches('/') == entry.path)
                .map(|(path, bytes)| (path.clone(), Bytes::from(bytes.clone())))
        })
        .collect();
    let hub = sentry::Hub::current();
    let files = tokio::task::spawn_blocking(move || {
        sentry::Hub::run(hub, || {
            let mut variants = precompress::precompressed_variants(&files);
            files.append(&mut variants);
            files
        })
    })
    .await
    .map_err(|e| PublishError::BadTarball(format!("bundle pre-compression task failed: {e}")))?;
    Ok((files, manifest, seed_entries))
}
