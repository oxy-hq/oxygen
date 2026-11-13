use super::*;
use oxy_authz::{Cap, PartnerStanding, PlatformRole, PlatformStanding, Scope};

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

const ACME: u128 = 10;
const ZED: u128 = 11;
const CLIENT: u128 = 12;

/// Owner of Acme, plain member of Zed.
fn member() -> PrincipalFacts {
    PrincipalFacts {
        user_id: id(1),
        owned_orgs: vec![id(ACME)],
        admin_orgs: vec![id(ACME)],
        member_orgs: vec![id(ACME), id(ZED)],
        ..Default::default()
    }
}

fn partner() -> PrincipalFacts {
    PrincipalFacts {
        partners: vec![PartnerStanding {
            partner_id: id(90),
            // Its own member org too: reached as a member, listed once.
            client_orgs: vec![id(CLIENT), id(ZED)],
            caps: vec![Cap::ManageMembers],
        }],
        ..member()
    }
}

fn org(n: u128, name: &str) -> OrgRow {
    OrgRow {
        id: id(n),
        name: name.to_string(),
        slug: name.to_lowercase(),
    }
}

fn workspace(n: u128, org: u128, name: &str) -> WorkspaceRow {
    WorkspaceRow {
        id: id(n),
        org_id: id(org),
        name: name.to_string(),
    }
}

fn orgs() -> Vec<OrgRow> {
    vec![org(ZED, "Zed"), org(ACME, "Acme"), org(CLIENT, "Client")]
}

#[test]
fn a_member_is_offered_their_orgs_with_their_role_in_each() {
    let workspaces = [
        workspace(21, ACME, "Staging"),
        workspace(20, ACME, "Default"),
        workspace(30, ZED, "Default"),
        // Someone else's org: never offered.
        workspace(40, CLIENT, "Default"),
    ];
    let out = build(
        &member(),
        &orgs(),
        &workspaces,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert!(!out.can_platform && !out.can_partner);

    let names: Vec<&str> = out.orgs.iter().map(|o| o.org_name.as_str()).collect();
    assert_eq!(names, ["Acme", "Zed"], "by name, and only the caller's");

    let acme = &out.orgs[0];
    assert_eq!(
        (acme.role, acme.via, acme.org_slug.as_str()),
        ("owner", "member", "acme")
    );
    let listed: Vec<(&str, &str)> = acme
        .workspaces
        .iter()
        .map(|w| (w.name.as_str(), w.role))
        .collect();
    assert_eq!(listed, [("Default", "owner"), ("Staging", "owner")]);
    assert_eq!(out.orgs[1].role, "member");
    assert_eq!(out.orgs[1].workspaces[0].role, "member");
}

#[test]
fn a_workspace_elevation_raises_the_role_shown_and_never_lowers_it() {
    let workspaces = [
        workspace(30, ZED, "Default"),
        workspace(20, ACME, "Default"),
    ];
    let elevated = HashMap::from([
        (id(30), WorkspaceRole::Admin),
        // Below the org-derived owner role: no effect.
        (id(20), WorkspaceRole::Viewer),
    ]);
    let out = build(&member(), &orgs(), &workspaces, &elevated, &HashMap::new());
    assert_eq!(out.orgs[0].workspaces[0].role, "owner");
    assert_eq!(out.orgs[1].workspaces[0].role, "admin");
}

#[test]
fn a_partner_is_offered_its_clients_as_an_admin() {
    let workspaces = [workspace(40, CLIENT, "Default")];
    let out = build(
        &partner(),
        &orgs(),
        &workspaces,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert!(out.can_partner && !out.can_platform);

    let client = out
        .orgs
        .iter()
        .find(|o| o.org_id == id(CLIENT))
        .expect("the client is offered");
    assert_eq!((client.role, client.via), ("admin", "partner"));
    assert_eq!(client.workspaces[0].role, "admin");

    // A client the partner is also a member of is listed once, as a member.
    let zed: Vec<&OrgOption> = out.orgs.iter().filter(|o| o.org_id == id(ZED)).collect();
    assert_eq!(zed.len(), 1);
    assert_eq!(zed[0].via, "member");
}

#[test]
fn staff_may_tick_platform_and_are_offered_no_tenant_they_are_not_in() {
    let staff = PrincipalFacts {
        platform: Some(PlatformStanding::from_role(
            PlatformRole::GlobalAdmin,
            Scope::All,
        )),
        ..member()
    };
    let out = build(&staff, &orgs(), &[], &HashMap::new(), &HashMap::new());
    assert!(out.can_platform && !out.can_partner);
    assert_eq!(out.orgs.len(), 2);
}

#[test]
fn an_org_with_no_policy_row_reads_the_defaults() {
    let out = build(&member(), &orgs(), &[], &HashMap::new(), &HashMap::new());
    let wire = serde_json::to_value(&out).unwrap();
    for org in wire["orgs"].as_array().unwrap() {
        assert_eq!(
            org["policy"],
            serde_json::json!({ "max_lifetime_days": null, "allow_all_access_tokens": true })
        );
    }
    assert_eq!(wire["can_platform"], false);
}

#[test]
fn each_org_is_offered_with_its_own_token_policy() {
    // Acme caps lifetimes and refuses all-access tokens; Zed set nothing. The
    // dialog used to be told "no cap, all-access allowed" for both.
    let policies = HashMap::from([(
        id(ACME),
        OrgPolicy {
            max_lifetime_days: Some(90),
            allow_all_access_tokens: false,
            require_environment_on_trust_policies: true,
        },
    )]);
    let out = build(&member(), &orgs(), &[], &HashMap::new(), &policies);
    let wire = serde_json::to_value(&out).unwrap();
    assert_eq!(wire["orgs"][0]["org_name"], "Acme");
    assert_eq!(
        wire["orgs"][0]["policy"],
        serde_json::json!({ "max_lifetime_days": 90, "allow_all_access_tokens": false })
    );
    assert_eq!(wire["orgs"][1]["org_name"], "Zed");
    assert_eq!(
        wire["orgs"][1]["policy"],
        serde_json::json!({ "max_lifetime_days": null, "allow_all_access_tokens": true })
    );

    // Each rule is carried on its own: a cap alone leaves all-access allowed.
    let cap_only = HashMap::from([(
        id(ZED),
        OrgPolicy {
            max_lifetime_days: Some(30),
            ..OrgPolicy::default()
        },
    )]);
    let out = build(&member(), &orgs(), &[], &HashMap::new(), &cap_only);
    assert_eq!(
        out.orgs[1].policy,
        PolicyDto {
            max_lifetime_days: Some(30),
            allow_all_access_tokens: true,
        }
    );
}
