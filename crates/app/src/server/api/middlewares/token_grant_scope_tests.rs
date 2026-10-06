use super::*;

#[test]
fn the_workspace_and_org_trees_enforce_grants_themselves() {
    for path in [
        "/3f2504e0-4f89-11d3-9a0c-0305e82c3301/threads",
        "/3f2504e0-4f89-11d3-9a0c-0305e82c3301/api-keys",
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
    for (i, (route, _, how)) in WHEN_BLOCKED.iter().enumerate() {
        assert!(
            refused_family(route).is_some(),
            "{route} is in no refused family, so it needs no entry"
        );
        assert!(!how.is_empty(), "{route} does not say how");
        // No other entry answers for this route, by name or by `{param}`.
        let claimed = WHEN_BLOCKED
            .iter()
            .filter(|(other, ..)| is_route(other, route) || is_route(route, other))
            .count();
        assert_eq!(claimed, 1, "{route} is decided {claimed} times");
        assert!(
            WHEN_BLOCKED[..i].iter().all(|(other, ..)| other != route),
            "{route} is listed twice"
        );
    }
}

#[test]
fn a_param_stands_for_exactly_one_segment() {
    assert!(is_route("/work/{id}", &format!("/work/{ID}")));
    assert!(is_route("/work/{id}", "/work/anything/"));
    assert!(!is_route("/work/{id}", "/work"));
    assert!(!is_route("/work/{id}", "/work/a/b"));
    assert!(!is_route("/work", "/works"));
    assert_eq!(
        when_blocked("/notifications/devices"),
        Some(Blocked::NoOrgData)
    );
    assert_eq!(
        when_blocked(&format!("/notifications/{ID}/read")),
        Some(Blocked::LeftOut)
    );
}

#[test]
fn token_management_answers_for_itself() {
    // These must reach their handler: the contract answers a token 403
    // `session_required` there, and `/auth/token` describes the calling token.
    for path in [
        "/user/tokens",
        "/user/tokens/3f2504e0-4f89-11d3-9a0c-0305e82c3301/regenerate",
        "/user/token-options",
        "/auth/token",
        "/auth/cli/authorize",
    ] {
        assert_eq!(treatment(path), Treatment::Honoured, "{path}");
    }
}
