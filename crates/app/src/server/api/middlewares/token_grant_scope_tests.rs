use super::*;

#[test]
fn the_workspace_and_org_trees_enforce_grants_themselves() {
    for path in [
        "/3f2504e0-4f89-11d3-9a0c-0305e82c3301/threads",
        "/3f2504e0-4f89-11d3-9a0c-0305e82c3301/api-tokens",
        "/orgs",
        "/orgs/3f2504e0-4f89-11d3-9a0c-0305e82c3301/workspaces",
        "/orgs/3f2504e0-4f89-11d3-9a0c-0305e82c3301/members",
    ] {
        assert_eq!(treatment(path), Treatment::Honoured, "{path}");
    }
}

#[test]
fn membership_keyed_flat_routes_are_refused() {
    for path in [
        "/chat/channels",
        "/chat/channels/3f2504e0-4f89-11d3-9a0c-0305e82c3301/messages",
        "/work",
        "/work/3f2504e0-4f89-11d3-9a0c-0305e82c3301",
        "/notifications",
        "/notifications/read-all",
        "/invitations/mine",
        "/invitations/abc/accept",
        "/airhouse/me/credentials",
        "/oltp/me/connection",
        "/user/github/installations",
    ] {
        assert_eq!(treatment(path), Treatment::Refused, "{path}");
    }
}

#[test]
fn a_route_nobody_listed_is_refused() {
    for path in [
        "/",
        "",
        "/something-new",
        "/apps",
        "/apps/other",
        "/user",
        "/user/settings",
        // Not a workspace id, so not the workspace tree.
        "/not-a-uuid/threads",
        "/document-folders/extra",
    ] {
        assert_eq!(treatment(path), Treatment::Refused, "{path}");
    }
}

// ── An all-access token an org has blocked ──────────────────────────────────

const ID: &str = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";

#[test]
fn a_blocked_all_access_token_keeps_the_membership_keyed_routes() {
    for path in [
        "/chat/channels".to_string(),
        format!("/chat/channels/{ID}/join"),
        format!("/chat/channels/{ID}/messages"),
        format!("/chat/channels/{ID}/read"),
        format!("/chat/channels/{ID}/stream"),
        "/work".to_string(),
        format!("/work/{ID}"),
        "/notifications".to_string(),
        "/notifications/read-all".to_string(),
        format!("/notifications/{ID}/read"),
        "/notifications/devices".to_string(),
        "/notifications/vapid-public-key".to_string(),
        "/invitations/mine".to_string(),
        "/invitations/abc/accept".to_string(),
        "/airhouse/version".to_string(),
        "/airhouse/me/credentials".to_string(),
        "/airhouse/me/tokens/eph_ab12".to_string(),
        "/oltp/me/connection".to_string(),
        "/oltp/me/erd".to_string(),
        "/user/github/account".to_string(),
        "/user/github/installations/new-url".to_string(),
        "/user/github/callback".to_string(),
    ] {
        assert_eq!(
            treatment_when_blocked(&path),
            Treatment::Honoured,
            "{path}: a block takes one org away, not the route"
        );
        assert_eq!(
            treatment(&path),
            Treatment::Refused,
            "{path}: and a grant-bound token is refused as before"
        );
    }
}

#[test]
fn a_blocked_token_is_refused_where_nobody_decided() {
    for path in [
        "/",
        "",
        "/something-new",
        "/user",
        "/user/settings",
        "/not-a-uuid/threads",
        // In a membership-keyed family, but not a route anyone listed.
        "/chat",
        "/chat/threads",
        "/work/a/b",
        "/notifications/a/b/c",
        "/airhouse/me",
        "/user/github/repos",
    ] {
        assert_eq!(when_blocked(path), None, "{path}");
        assert_eq!(treatment_when_blocked(path), Treatment::Refused, "{path}");
    }
}

#[test]
fn a_route_that_honours_a_grant_honours_a_block() {
    // They read the token's reach, which leaves a blocked org out by itself.
    for path in [
        "/orgs".to_string(),
        format!("/orgs/{ID}/workspaces"),
        format!("/{ID}/threads"),
        "/apps/mine".to_string(),
        "/documents".to_string(),
        "/admin/orgs-meta".to_string(),
        "/auth/token".to_string(),
        "/user/tokens".to_string(),
        "/_catalog".to_string(),
    ] {
        assert_eq!(treatment_when_blocked(&path), Treatment::Honoured, "{path}");
        assert_eq!(when_blocked(&path), None, "{path} needs no entry");
    }
}

#[test]
fn every_blocked_decision_names_one_membership_keyed_route_once() {
    for (i, (route, blocked, how)) in WHEN_BLOCKED.iter().enumerate() {
        assert!(
            refused_family(route).is_some(),
            "{route} is in no refused family, so it needs no entry"
        );
        assert!(!how.is_empty(), "{route} does not say how");
        // No other entry answers for this route, by name or by `{param}`. Two
        // entries may share a path only where `when_blocked` picks one by rule,
        // the router's: at the first segment where they differ, the literal
        // wins. Two of the same shape would leave it to the list's order.
        let rivals: Vec<&str> = WHEN_BLOCKED
            .iter()
            .map(|(other, ..)| *other)
            .filter(|other| other != route && share_a_path(route, other))
            .filter(|other| shape(route) == shape(other))
            .collect();
        assert!(
            rivals.is_empty(),
            "{route} is decided {} times: {rivals:?}",
            rivals.len() + 1
        );
        assert_eq!(
            when_blocked(route),
            Some(*blocked),
            "{route} is answered by another entry"
        );
        assert!(
            WHEN_BLOCKED[..i].iter().all(|(other, ..)| other != route),
            "{route} is listed twice"
        );
    }
}

/// Whether some request path is an instance of both catalog routes.
fn share_a_path(a: &str, b: &str) -> bool {
    let a: Vec<&str> = a.split('/').filter(|s| !s.is_empty()).collect();
    let b: Vec<&str> = b.split('/').filter(|s| !s.is_empty()).collect();
    a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|(x, y)| x == y || x.starts_with('{') || y.starts_with('{'))
}

#[test]
fn a_literal_route_is_decided_before_a_param_sibling() {
    assert!(share_a_path(
        "/notifications/{id}",
        "/notifications/devices"
    ));
    assert!(share_a_path("/a/{x}/b", "/a/c/{y}"), "both match /a/c/b");
    assert!(!share_a_path(
        "/notifications/{id}",
        "/notifications/{id}/read"
    ));
    // `/notifications/{id}` is listed before `devices`; the literal still wins,
    // spelled exactly or not.
    for path in ["/notifications/devices", "/notifications/devices/"] {
        assert_eq!(when_blocked(path), Some(Blocked::NoOrgData), "{path}");
    }
    assert_eq!(
        when_blocked("/notifications/vapid-public-key"),
        Some(Blocked::NoOrgData)
    );
    // Both carry a `{param}`, and `/notifications/devices/read` is an instance
    // of each: the one literal first is the router's pick.
    assert!(share_a_path(
        "/notifications/devices/{id}",
        "/notifications/{id}/read"
    ));
    assert_eq!(
        when_blocked("/notifications/devices/read"),
        Some(Blocked::NoOrgData)
    );
    assert_eq!(
        when_blocked(&format!("/notifications/devices/{ID}")),
        Some(Blocked::NoOrgData)
    );
    assert_eq!(
        when_blocked(&format!("/notifications/{ID}/read")),
        Some(Blocked::LeftOut)
    );
    assert_eq!(when_blocked("/notifications/test"), Some(Blocked::LeftOut));
    assert_eq!(
        when_blocked(&format!("/notifications/{ID}")),
        Some(Blocked::LeftOut),
        "one notification is left out like the inbox"
    );
}

#[test]
fn a_param_stands_for_exactly_one_segment() {
    assert!(is_route("/work/{id}", &format!("/work/{ID}")));
    assert!(is_route("/work/{id}", "/work/anything/"));
    assert!(!is_route("/work/{id}", "/work"));
    assert!(!is_route("/work/{id}", "/work/a/b"));
    assert!(!is_route("/work", "/works"));
    // `/notifications/{id}` is listed too: the literal sibling still wins.
    assert_eq!(
        when_blocked("/notifications/devices"),
        Some(Blocked::NoOrgData)
    );
    assert_eq!(
        when_blocked(&format!("/notifications/{ID}/read")),
        Some(Blocked::LeftOut)
    );
    assert_eq!(
        when_blocked(&format!("/notifications/{ID}")),
        Some(Blocked::LeftOut)
    );
}

#[test]
fn token_management_answers_for_itself() {
    // These must reach their handler: the contract answers a token 403
    // `session_required` there, `/auth/token` describes the calling token, and
    // `/auth/browser-ticket` signs a browser in as it — narrowed as it is.
    for path in [
        "/user/tokens",
        "/user/tokens/3f2504e0-4f89-11d3-9a0c-0305e82c3301/regenerate",
        "/user/token-options",
        "/auth/token",
        "/auth/cli/authorize",
        "/auth/browser-ticket",
    ] {
        assert_eq!(treatment(path), Treatment::Honoured, "{path}");
    }
}
