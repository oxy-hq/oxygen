//! Moved from `oxy-app`'s `token_grant_scope_tests`: it checks the flat-route
//! treatment of a token that reaches less than its bearer — grant-bound, or
//! all-access and blocked by an org — against the generated route table.

use oxy_app::server::api::middlewares::token_grant_scope::{
    Blocked, REFUSED, Treatment, WHEN_BLOCKED, refused_family, treatment, treatment_when_blocked,
    when_blocked,
};

use crate::catalog;

/// The flat routes of the protected surface, relative to `/api`: everything
/// outside the org and workspace trees, whose middlewares check the path's id.
fn flat_routes() -> Vec<(String, String)> {
    catalog()
        .routes
        .iter()
        .filter(|route| route.surface == "org")
        .map(|route| {
            let path = route
                .path
                .strip_prefix("/api")
                .expect("catalog paths carry the /api prefix");
            (route.method.to_string(), path.to_string())
        })
        .filter(|(_, path)| {
            !path.starts_with("/orgs/{org_id}") && !path.starts_with("/{workspace_id}")
        })
        .collect()
}

/// Every flat route the protected surface mounts is either honoured or in
/// [`REFUSED`] with its reason. The catalog is read out of the router source
/// at build time, so a new flat route lands here without anyone remembering.
#[test]
fn every_flat_route_has_a_decided_treatment() {
    let flat = flat_routes();
    for (method, path) in &flat {
        match treatment(path) {
            Treatment::Honoured => assert_eq!(
                refused_family(path),
                None,
                "{path} is honoured but also listed as refused"
            ),
            Treatment::Refused => assert!(
                refused_family(path).is_some(),
                "{method} {path} is a flat route with no decided treatment for a grant-bound \
                 token: honour its grants and list it in `treatment`, or name it in REFUSED"
            ),
        }
    }
    assert!(
        flat.len() > 40,
        "the catalog walk lost the flat routes ({})",
        flat.len()
    );
}

#[test]
fn every_refused_family_still_exists() {
    for (prefix, why) in REFUSED {
        let mounted = catalog().routes.iter().any(|route| {
            route
                .path
                .strip_prefix("/api")
                .is_some_and(|path| refused_family(path) == Some(*prefix))
        });
        assert!(mounted, "{prefix} ({why}) is no longer mounted — drop it");
    }
}

/// The same walk for an **all-access token an org has blocked**: a flat route
/// that does not honour a token's reach by itself must say, in
/// [`WHEN_BLOCKED`], what such a token gets — left out, nothing to leave out,
/// or refused. Undecided is refused at run time; this makes it a build failure
/// first, so a new `/chat/…` route cannot quietly answer with a blocked org's
/// rows, nor quietly 404.
#[test]
fn every_membership_keyed_route_decides_for_a_blocked_token() {
    let mut decided = 0;
    for (method, path) in flat_routes() {
        if treatment(&path) == Treatment::Honoured {
            assert_eq!(
                treatment_when_blocked(&path),
                Treatment::Honoured,
                "{path} honours a grant, so it honours a block by the same means"
            );
            assert_eq!(
                when_blocked(&path),
                None,
                "{path} honours a token's reach itself — drop its WHEN_BLOCKED entry"
            );
            continue;
        }
        let Some(blocked) = when_blocked(&path) else {
            panic!(
                "{method} {path} answers from raw membership and says nothing of a token an org \
                 has blocked: make its handler leave the blocked orgs out and list it in \
                 WHEN_BLOCKED, or list it as `Blocked::Refused`"
            );
        };
        decided += 1;
        let expected = match blocked {
            Blocked::LeftOut | Blocked::NoOrgData => Treatment::Honoured,
            Blocked::Refused => Treatment::Refused,
        };
        assert_eq!(treatment_when_blocked(&path), expected, "{method} {path}");
    }
    assert!(
        decided > 20,
        "the walk lost the membership-keyed routes ({decided})"
    );
}

#[test]
fn every_blocked_decision_is_for_a_route_still_mounted() {
    let flat = flat_routes();
    for (route, _, how) in WHEN_BLOCKED {
        assert!(
            flat.iter().any(|(_, path)| path == route),
            "{route} ({how}) is no longer mounted as written — drop or respell it"
        );
    }
}
