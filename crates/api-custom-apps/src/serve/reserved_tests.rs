//! The platform-reserved `__oxy/` namespace: what it recognises, how each
//! name is classified by method, and the manifest's short name and icon.

use super::*;

/// The monogram must be classified, not fall through to the bundle — the
/// whole point of the reserved namespace.
#[test]
fn monogram_is_a_classified_reserved_path() {
    assert!(matches!(
        classify_reserved("icon.svg", &axum::http::Method::GET),
        Reserved::MonogramIcon
    ));
    assert!(matches!(
        classify_reserved("icon.svg", &axum::http::Method::POST),
        Reserved::Unknown
    ));
}

/// The fallback exists to fit a home-screen label. Falling back to the slug
/// did not: `"Store Operations"` and `"store-operations"` are both 16.
#[test]
fn short_name_actually_shortens() {
    assert_eq!(short_name_for("Store Ops"), "Store Ops");
    assert_eq!(short_name_for("Store Operations"), "Store");
    assert_eq!(short_name_for("Poke House Store Ops"), "Poke House");
    // One long word has no boundary to cut on, so cut anyway rather than
    // returning something that overflows.
    assert_eq!(short_name_for("Supercalifragilistic").chars().count(), 12);
    for name in [
        "Store Operations",
        "Poke House Store Ops",
        "Supercalifragilistic",
    ] {
        assert!(
            short_name_for(name).chars().count() <= 12,
            "{name} produced a label that is still too long"
        );
    }
}

#[test]
fn recognises_the_reserved_namespace_and_nothing_adjacent() {
    assert_eq!(reserved_platform_path("__oxy/sw.js"), Some("sw.js"));
    assert_eq!(reserved_platform_path("/__oxy/beacon"), Some("beacon"));
    // Case-insensitive prefix, case-preserving tail.
    assert_eq!(reserved_platform_path("__OXY/sw.js"), Some("sw.js"));
    assert_eq!(reserved_platform_path("__oxy/"), Some(""));

    // An app's own files must never be captured by the prefix.
    assert_eq!(reserved_platform_path("assets/__oxy/x.js"), None);
    assert_eq!(reserved_platform_path("__oxygen/x.js"), None);
    assert_eq!(reserved_platform_path("index.html"), None);
    assert_eq!(reserved_platform_path(""), None);
    // Shorter than the prefix — the split must not panic.
    assert_eq!(reserved_platform_path("__ox"), None);
}

use axum::http::Method;

#[test]
fn classifies_the_platform_endpoints_by_name_and_method() {
    assert_eq!(
        classify_reserved("sw.js", &Method::GET),
        Reserved::ServiceWorker
    );
    assert_eq!(classify_reserved("beacon", &Method::POST), Reserved::Beacon);
    assert_eq!(
        classify_reserved("asset-manifest.json", &Method::GET),
        Reserved::BuildObject
    );
}

/// The live-build probe is a GET-only platform endpoint. A POST is an
/// unknown resource like any other wrong verb, never a fall-through.
#[test]
fn the_live_build_probe_is_get_only() {
    assert_eq!(
        classify_reserved("build", &Method::GET),
        Reserved::LiveBuild
    );
    assert_eq!(classify_reserved("build", &Method::POST), Reserved::Unknown);
    // Not a prefix match: an app path that merely starts with the name is
    // not the probe.
    assert_eq!(classify_reserved("builds", &Method::GET), Reserved::Unknown);
}

/// The manifest is written into the build prefix at publish, so it must
/// reach the ordinary dispatch rather than being answered here. Getting this
/// wrong 404s the document the service worker's whole precache depends on —
/// and does it silently, because a worker with no manifest simply installs
/// and caches nothing.
#[test]
fn the_asset_manifest_falls_through_to_the_build() {
    assert_eq!(
        classify_reserved("asset-manifest.json", &Method::GET),
        Reserved::BuildObject,
        "the manifest is an object in the build, not a synthesised response"
    );
}

/// A wrong method is an unknown resource, not a 405: answering differently
/// would advertise which verbs each platform path takes.
#[test]
fn a_wrong_method_or_unknown_name_is_simply_not_found() {
    assert_eq!(classify_reserved("sw.js", &Method::POST), Reserved::Unknown);
    assert_eq!(classify_reserved("beacon", &Method::GET), Reserved::Unknown);
    assert_eq!(classify_reserved("", &Method::GET), Reserved::Unknown);
    assert_eq!(
        classify_reserved("not-a-thing", &Method::GET),
        Reserved::Unknown
    );
    // An app's own file under the prefix cannot be reached either — publish
    // strips them, and this is the serve-side half of the same rule.
    assert_eq!(
        classify_reserved("secrets.json", &Method::GET),
        Reserved::Unknown
    );
}
