//! `GET /api/_catalog` handler tests — moved from `oxy-app`'s
//! `server::api::catalog`, since they need the generated table.

use axum::Json;
use axum::extract::Query;
use oxy_app::server::api::catalog::{CatalogQuery, get_catalog};
use oxy_app::server::route_catalog::searchable_fields;

use crate::catalog;

/// The endpoint's whole job is to be non-empty and to carry both surfaces.
/// An empty catalog is indistinguishable, on the client, from "this
/// deployment has no such route" — which is why `oxyc` refuses one rather
/// than serving it, and why the server must never produce one.
#[tokio::test]
async fn catalog_carries_both_surfaces_and_is_not_empty() {
    let Json(catalog) = get_catalog(catalog(), Query(CatalogQuery::default())).await;

    assert!(
        catalog.routes.len() > 100,
        "the catalog shrank to {} routes — the build-time walk probably stopped following the router",
        catalog.routes.len()
    );
    assert!(
        catalog.routes.iter().any(|r| r.path.starts_with("/api/")),
        "no /api routes in the catalog"
    );
    assert!(
        catalog
            .routes
            .iter()
            .any(|r| r.path.starts_with("/external/api/")),
        "no /external/api routes in the catalog"
    );
    assert!(!catalog.surfaces.is_empty(), "no surfaces described");
}

/// Every route reports a role, because `oxyc routes` filters on it: the
/// default view hides `ide-only` and `worker-only` mounts, which a caller
/// hitting the load balancer cannot reach directly.
#[tokio::test]
async fn every_route_carries_a_role() {
    let Json(catalog) = get_catalog(catalog(), Query(CatalogQuery::default())).await;
    for route in &catalog.routes {
        assert!(
            matches!(route.role, "fleet-ok" | "ide-only" | "worker-only"),
            "{} {} reported an unknown role {:?}",
            route.method,
            route.path,
            route.role
        );
    }
}

/// The catalog must describe ITSELF. A caller that cannot discover the
/// discovery endpoint has to be told about it out of band, which is the
/// situation this whole surface exists to remove.
#[tokio::test]
async fn the_catalog_lists_itself() {
    let Json(catalog) = get_catalog(catalog(), Query(CatalogQuery::default())).await;
    assert!(
        catalog.routes.iter().any(|r| r.path == "/api/_catalog"),
        "/api/_catalog is missing from its own route table — the build-time \
         walk did not see its mount"
    );
}

/// `?filter=` narrows server-side, so a client after three routes does not
/// download all ~670.
#[tokio::test]
async fn the_filter_narrows_the_table() {
    let Json(all) = get_catalog(catalog(), Query(CatalogQuery::default())).await;
    let Json(filtered) = get_catalog(
        catalog(),
        Query(CatalogQuery {
            filter: Some("threads".into()),
        }),
    )
    .await;

    assert!(
        filtered.routes.len() < all.routes.len(),
        "the filter returned everything — it is not being applied"
    );
    assert!(!filtered.routes.is_empty(), "no route matched `threads`");
    // The haystack is built from EXACTLY the fields `search` reads, via the
    // same function it reads them with. Listing them here by hand made the
    // assertion a superset of the real filter, so it passed whether or not
    // `description` was searched — which is precisely how the endpoint's
    // doc came to claim a field the filter did not match.
    for route in &filtered.routes {
        let matched = catalog()
            .routes
            .iter()
            .find(|r| r.method == route.method && r.path == route.path)
            .map(searchable_fields)
            .expect("a filtered route is in the catalog")
            .iter()
            .any(|field| field.to_lowercase().contains("threads"));
        assert!(
            matched,
            "{} {} matched `threads` but contains it in none of the searchable fields",
            route.method, route.path
        );
    }
}

/// `description` really is searched, not merely documented as searched.
///
/// The generic filter test cannot show this: `threads` appears in the path
/// of every route it returns, so it would pass against a filter that
/// ignored descriptions entirely. This needs a needle that lives ONLY in a
/// description.
#[tokio::test]
async fn the_filter_reaches_descriptions() {
    // A word from some handler's doc comment that is in no path.
    let needle = catalog()
        .routes
        .iter()
        .filter(|r| !r.description.is_empty())
        .find_map(|r| {
            r.description
                .split_whitespace()
                .map(|w| {
                    w.trim_matches(|c: char| !c.is_alphanumeric())
                        .to_lowercase()
                })
                .find(|w| {
                    w.len() > 7
                        && catalog()
                            .routes
                            .iter()
                            .all(|other| !other.path.to_lowercase().contains(w.as_str()))
                })
        })
        .expect("some description carries a word that appears in no path");

    let Json(got) = get_catalog(
        catalog(),
        Query(CatalogQuery {
            filter: Some(needle.clone()),
        }),
    )
    .await;
    assert!(
        !got.routes.is_empty(),
        "`?filter={needle}` matched nothing — it only appears in a description, so \
         the filter is not reading descriptions"
    );
}

/// An empty or whitespace-only filter means "everything", not "nothing" —
/// `?filter=` from a client that built the query string unconditionally
/// must not come back as an empty catalog, which every caller reads as
/// "this deployment has no routes".
#[tokio::test]
async fn a_blank_filter_is_not_a_filter() {
    let Json(all) = get_catalog(catalog(), Query(CatalogQuery::default())).await;
    for blank in ["", "   "] {
        let Json(got) = get_catalog(
            catalog(),
            Query(CatalogQuery {
                filter: Some(blank.into()),
            }),
        )
        .await;
        assert_eq!(
            got.routes.len(),
            all.routes.len(),
            "a blank filter ({blank:?}) narrowed the table"
        );
    }
}
