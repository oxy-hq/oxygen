//! Integration tests for the Slack workspace/agent resolution cascade.
//!
//! These tests drive `resolution::workspace_agent::resolve` against a real
//! PostgreSQL database. They skip automatically when `OXY_DATABASE_URL` is unset.
//!
//! To run locally:
//!   OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-app --test slack -E 'test(slack_resolution)'

use entity::{
    org_members, organizations, slack_channel_defaults, slack_installations, slack_user_links,
    slack_user_preferences, users, workspace_members, workspaces,
};
use oxy::database::client::establish_connection;
use oxy_app::integrations::slack::resolution::workspace_agent::Resolution;
use sea_orm::{ActiveModelTrait, ActiveValue};
use uuid::Uuid;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

fn set_test_encryption_key() {
    use base64::{Engine as _, engine::general_purpose};
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            general_purpose::STANDARD.encode([42u8; 32]),
        );
    }
}

/// Seed org + user + org membership, return (org_id, user_id).
async fn seed_org_user() -> (Uuid, Uuid) {
    let conn = establish_connection().await.expect("db connect");

    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(format!("resolution-test-{}@example.com", user_id))),
        name: ActiveValue::Set("Resolution Test User".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed user");

    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("Resolution Org {}", org_id)),
        slug: ActiveValue::Set(format!("resolution-org-{}", org_id)),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed org");

    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(org_members::OrgRole::Member),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed org member");

    (org_id, user_id)
}

/// Seed a Slack installation for the given org, return the installation row.
async fn seed_installation(org_id: Uuid, user_id: Uuid) -> slack_installations::Model {
    use oxy_app::integrations::slack::services::installations::{
        InstallationsService, UpsertInstallation,
    };
    let team_id = format!("T{}", &Uuid::new_v4().simple().to_string()[..8]);
    InstallationsService::upsert(UpsertInstallation {
        org_id,
        team_id: team_id.clone(),
        team_name: format!("Team {team_id}"),
        enterprise_id: None,
        bot_user_id: format!("B_{team_id}"),
        bot_token: format!("xoxb-fake-token-{}", Uuid::new_v4()),
        scopes: "chat:write,users:read".into(),
        installed_by_user_id: user_id,
        installed_by_slack_user_id: format!("U{}", &user_id.simple().to_string()[..8]),
    })
    .await
    .expect("seed installation")
}

/// Seed a slack_user_links row, return the link row.
async fn seed_user_link(
    installation_id: Uuid,
    slack_user_id: &str,
    oxy_user_id: Uuid,
) -> slack_user_links::Model {
    use oxy_app::integrations::slack::services::user_links::{
        CreateLink, LinkMethod, UserLinksService,
    };
    UserLinksService::create(CreateLink {
        installation_id,
        slack_user_id: slack_user_id.to_string(),
        oxy_user_id,
        link_method: LinkMethod::EmailAuto,
    })
    .await
    .expect("seed user link")
}

/// Seed a workspace in an org (no filesystem — path is None), return workspace_id.
async fn seed_workspace(org_id: Uuid, name: &str) -> Uuid {
    let conn = establish_connection().await.expect("db connect");
    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set(name.to_string()),
        org_id: ActiveValue::Set(Some(org_id)),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        path: ActiveValue::Set(None),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        status: ActiveValue::Set(workspaces::WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed workspace");
    ws_id
}

async fn add_workspace_member(workspace_id: Uuid, user_id: Uuid) {
    let conn = establish_connection().await.expect("db connect");
    workspace_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(workspace_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(workspace_members::WorkspaceRole::Member),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed workspace member");
}

// ── tests ────────────────────────────────────────────────────────────────────

/// User has no workspace memberships in the installation's org → NoWorkspaces.
#[tokio::test]
async fn resolves_no_workspaces_when_user_has_none() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_user().await;
    let installation = seed_installation(org_id, user_id).await;
    let slack_user_id = format!("U{}", &Uuid::new_v4().simple().to_string()[..8]);
    let user_link = seed_user_link(installation.id, &slack_user_id, user_id).await;

    // user_id has no workspace memberships — resolve should return NoWorkspaces.
    let resolution = oxy_app::integrations::slack::resolution::workspace_agent::resolve(
        &installation,
        &user_link,
        "",
    )
    .await
    .expect("resolve should not error");

    // The user we seed is an org member (seed_org_user puts them in org_members),
    // but the org has no workspaces — expect OrgHasNoWorkspaces.
    assert!(
        matches!(
            resolution,
            Resolution::NoWorkspaces(
                oxy_app::integrations::slack::resolution::workspace_agent::NoWorkspacesReason::OrgHasNoWorkspaces { .. }
            )
        ),
        "expected NoWorkspaces(OrgHasNoWorkspaces), got: {resolution:?}"
    );
}

/// User with 2 workspaces and a stored preference → Resolved with that preference.
///
/// NOTE: `list_agents_and_default` requires a workspace filesystem (config.yml) to
/// enumerate agents; workspaces without a path return an empty agent list. Because
/// we seed workspaces without a filesystem, the single-workspace silent path
/// (Resolution::Resolved) cannot be exercised purely via DB seeding.
///
/// This test seeds a preference row directly and verifies that `resolve` honors it,
/// bypassing the filesystem-dependent `list_agents_and_default` call.
#[tokio::test]
async fn resolves_from_user_preferences_when_set() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_user().await;
    let installation = seed_installation(org_id, user_id).await;
    let slack_user_id = format!("U{}", &Uuid::new_v4().simple().to_string()[..8]);
    let user_link = seed_user_link(installation.id, &slack_user_id, user_id).await;

    // Seed two workspaces, add user to both.
    let ws1 = seed_workspace(org_id, "workspace-alpha").await;
    let ws2 = seed_workspace(org_id, "workspace-beta").await;
    add_workspace_member(ws1, user_id).await;
    add_workspace_member(ws2, user_id).await;

    // Store a preference: user prefers ws1 with a specific agent.
    let preferred_agent = "agents/my-agent.agentic.yml".to_string();
    let conn = establish_connection().await.expect("db connect");
    slack_user_preferences::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        user_link_id: ActiveValue::Set(user_link.id),
        default_workspace_id: ActiveValue::Set(Some(ws1)),
        default_agent_path: ActiveValue::Set(Some(preferred_agent.clone())),
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed preferences");

    let resolution = oxy_app::integrations::slack::resolution::workspace_agent::resolve(
        &installation,
        &user_link,
        "",
    )
    .await
    .expect("resolve should not error");

    match resolution {
        Resolution::Resolved {
            workspace_id,
            agent_path,
        } => {
            assert_eq!(workspace_id, ws1, "should resolve to preferred workspace");
            assert_eq!(agent_path, preferred_agent, "should use preferred agent");
        }
        other => panic!("expected Resolved, got: {other:?}"),
    }
}

/// User with 2 workspaces and no preferences → PickerNeeded with both workspaces.
///
/// NOTE: Because workspaces have no filesystem (path is None), `list_agents_and_default`
/// returns an empty agent list for each, so the single-workspace branch does not
/// silently resolve. With 2 workspaces and no prefs, we expect PickerNeeded.
#[tokio::test]
async fn resolves_picker_needed_when_multiple_workspaces_no_prefs() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_user().await;
    let installation = seed_installation(org_id, user_id).await;
    let slack_user_id = format!("U{}", &Uuid::new_v4().simple().to_string()[..8]);
    let user_link = seed_user_link(installation.id, &slack_user_id, user_id).await;

    // Seed 2 workspaces, user is member of both.
    let ws1 = seed_workspace(org_id, "ws-picker-1").await;
    let ws2 = seed_workspace(org_id, "ws-picker-2").await;
    add_workspace_member(ws1, user_id).await;
    add_workspace_member(ws2, user_id).await;

    let resolution = oxy_app::integrations::slack::resolution::workspace_agent::resolve(
        &installation,
        &user_link,
        "",
    )
    .await
    .expect("resolve should not error");

    match resolution {
        Resolution::PickerNeeded { workspaces } => {
            assert_eq!(
                workspaces.len(),
                2,
                "should include both workspaces in picker"
            );
            let ids: Vec<Uuid> = workspaces.iter().map(|w| w.id).collect();
            assert!(ids.contains(&ws1), "ws1 should be in picker");
            assert!(ids.contains(&ws2), "ws2 should be in picker");
        }
        other => panic!("expected PickerNeeded, got: {other:?}"),
    }
}

/// A channel default bypasses the picker: with 2 workspaces and a channel default
/// pointing to ws1, resolve returns Resolved(ws1) without showing a picker.
///
/// NOTE: Because workspaces have no filesystem, `list_agents` returns an empty
/// list → WorkspaceHasNoAgents is returned instead of Resolved. We verify that
/// the channel default branch was taken (not PickerNeeded) by checking the variant.
#[tokio::test]
async fn channel_default_resolves_without_picker() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_user().await;
    let installation = seed_installation(org_id, user_id).await;
    let slack_user_id = format!("U{}", &Uuid::new_v4().simple().to_string()[..8]);
    let user_link = seed_user_link(installation.id, &slack_user_id, user_id).await;

    // Seed 2 workspaces (no filesystem path → no agents).
    let ws1 = seed_workspace(org_id, "ws-channel-default-1").await;
    let ws2 = seed_workspace(org_id, "ws-channel-default-2").await;
    add_workspace_member(ws1, user_id).await;
    add_workspace_member(ws2, user_id).await;

    // Seed a channel default pointing to ws1.
    let channel_id = format!("C{}", Uuid::new_v4().simple().to_string()[..8].to_string());
    let conn = establish_connection().await.expect("db connect");
    slack_channel_defaults::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        installation_id: ActiveValue::Set(installation.id),
        slack_channel_id: ActiveValue::Set(channel_id.clone()),
        workspace_id: ActiveValue::Set(ws1),
        set_by_user_link_id: ActiveValue::Set(Some(user_link.id)),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed channel default");

    let resolution = oxy_app::integrations::slack::resolution::workspace_agent::resolve(
        &installation,
        &user_link,
        &channel_id,
    )
    .await
    .expect("resolve should not error");

    // The channel default is set → resolve should NOT return PickerNeeded.
    // With no filesystem, it returns WorkspaceHasNoAgents for ws1 — that's fine,
    // the key assertion is that PickerNeeded was not returned.
    assert!(
        !matches!(resolution, Resolution::PickerNeeded { .. }),
        "channel default should prevent PickerNeeded, got: {resolution:?}"
    );
    // It should resolve to ws1 specifically (WorkspaceHasNoAgents or Resolved).
    match &resolution {
        Resolution::WorkspaceHasNoAgents { workspace_id, .. } => {
            assert_eq!(
                *workspace_id, ws1,
                "should resolve to channel-default workspace"
            );
        }
        Resolution::Resolved { workspace_id, .. } => {
            assert_eq!(
                *workspace_id, ws1,
                "should resolve to channel-default workspace"
            );
        }
        other => panic!("unexpected resolution: {other:?}"),
    }
}
