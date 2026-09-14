//! A `Link` header must name a path a client can actually fetch.
//!
//! Every API surface here is mounted under `/api`, and axum's `nest` strips
//! that prefix before an inner handler runs. So a handler that builds its
//! pagination links from a bare `Uri` emits `</documents?...&offset=5>`, and a
//! client resolving that against the request it made — RFC 3986, what every
//! HTTP client does — asks for `/documents` and gets a 404 on page two. The
//! endpoint reports that it pages and then cannot be paged.
//!
//! `OriginalUri` is the extractor that keeps the prefix. It is not a style
//! preference: it is the difference between a working `rel="next"` and one that
//! points off the API. This shipped once, in `documents::handlers::list`, and
//! was invisible to the first HTTP check because the script following the links
//! prepended `/api` itself.
//!
//! Source-level rather than a request, deliberately. Driving the real handler
//! needs an authenticated extractor and a database, which this binary has
//! neither of by design (see `main.rs`) — and the property is a property of the
//! source: which extractor the handler took.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test routing -E 'test(paginated_links)'`

use std::fs;
use std::path::{Path, PathBuf};

/// Every file under `src/server/api` that builds a paginated response.
fn adopters() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/api");
    let mut found = vec![];
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read the api directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src = fs::read_to_string(&path).expect("read a handler");
                if src.contains("pagination::page(") {
                    found.push((path, src));
                }
            }
        }
    }
    found
}

#[test]
fn a_paginated_handler_takes_original_uri() {
    let adopters = adopters();

    // The guard is only worth anything if it is looking at something. A refactor
    // that moves these handlers elsewhere should fail here loudly rather than
    // pass by finding nothing — which is how a check quietly stops checking.
    assert!(
        adopters.len() >= 5,
        "expected the known paginated handlers under src/server/api, found {}",
        adopters.len()
    );

    for (path, src) in adopters {
        let file = path.display();
        assert!(
            src.contains("OriginalUri("),
            "{file} builds a Link header but never takes `OriginalUri`, so the link \
             will be missing the `/api` prefix the router nests it under"
        );
        // The bare extractor, as it appears in an argument list. `Uri` also
        // shows up in imports and in `let uri: Uri = ...` inside tests, and
        // neither of those is a handler taking the stripped path.
        assert!(
            !src.contains("\n    uri: Uri,"),
            "{file} takes a bare `Uri` as a handler argument. Under `nest(\"/api\", ..)` \
             that is the path with the prefix already removed — use `OriginalUri`"
        );
    }
}
