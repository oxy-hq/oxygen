//! A notification's Slack DM reaches the right person: a member who never
//! talked to the bot is found by their email, linked once, and DMed; anyone
//! who cannot be matched safely is `not_linked`, never someone else.
//!
//! Drives [`SlackDm`] against the real database and a wiremock Slack, so the
//! link it writes and the calls it makes are both observable. Skips when
//! `OXY_DATABASE_URL` is unset.
//!
//! `cargo nextest run -p oxy-api-notifications --test integration -E 'test(slack_notify_dm)'`

use entity::{org_members, organizations, slack_user_links, users};
use oxy::database::client::establish_connection;
use oxy_api_notifications::channels::ChannelOutcome;
use oxy_api_notifications::slack_dm::{SlackDm, SlackMessage};
use oxy_app::integrations::slack::services::installations::{
    InstallationsService, UpsertInstallation,
};
use oxy_app::integrations::slack::services::user_links::{
    CreateLink, LinkMethod, UserLinksService,
};
use oxy_oltp::inbox::InboxMessage;
use oxy_slack_client::SlackClient;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WITH_LOOKUP: &str = "chat:write,users:read,users:read.email";

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

struct Org {
    db: DatabaseConnection,
    id: Uuid,
    installation: Uuid,
}

async fn seed_user(db: &DatabaseConnection) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let email = format!("notify-{}@example.com", id.simple());
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.clone())),
        name: ActiveValue::Set("Notify Test".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed user");
    (id, email)
}

async fn join(org: &Org, user: Uuid) {
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org.id),
        user_id: ActiveValue::Set(user),
        role: ActiveValue::Set(org_members::OrgRole::Member),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&org.db)
    .await
    .expect("seed member");
}

/// An org with Slack connected, granted `scopes`.
async fn seed_org(scopes: &str) -> Org {
    set_test_encryption_key();
    let db = establish_connection().await.expect("db connect");
    let (installer, _) = seed_user(&db).await;
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(format!("Notify Org {id}")),
        slug: ActiveValue::Set(format!("notify-org-{id}")),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&db)
    .await
    .expect("seed org");
    let team_id = format!("T{}", &Uuid::new_v4().simple().to_string()[..8]);
    let installation = InstallationsService::upsert(UpsertInstallation {
        org_id: id,
        team_id: team_id.clone(),
        team_name: format!("Team {team_id}"),
        enterprise_id: None,
        bot_user_id: format!("B_{team_id}"),
        bot_token: "xoxb-notify-test".into(),
        scopes: scopes.into(),
        installed_by_user_id: installer,
        installed_by_slack_user_id: format!("U{}", &installer.simple().to_string()[..8]),
    })
    .await
    .expect("seed installation");
    Org {
        db,
        id,
        installation: installation.id,
    }
}

/// Slack answers a lookup of `email` with `answer`, `times` times exactly.
async fn lookup(
    server: &MockServer,
    email: &str,
    status: u16,
    answer: serde_json::Value,
    times: u64,
) {
    Mock::given(method("POST"))
        .and(path("/users.lookupByEmail"))
        .and(body_string_contains(format!(
            "email={}",
            email.replace('@', "%40")
        )))
        .respond_with(ResponseTemplate::new(status).set_body_json(answer))
        .expect(times)
        .named(format!("lookup {email}"))
        .mount(server)
        .await;
}

/// Slack accepts `times` DMs to `slack_user`.
async fn dm(server: &MockServer, slack_user: &str, times: u64) {
    Mock::given(method("POST"))
        .and(path("/chat.postMessage"))
        .and(body_partial_json(json!({ "channel": slack_user })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
        .expect(times)
        .named(format!("dm {slack_user}"))
        .mount(server)
        .await;
}

fn content() -> InboxMessage {
    InboxMessage {
        id: Uuid::new_v4(),
        app_id: None,
        kind: "account_needs_attention".into(),
        title: "BMG: trust at risk".into(),
        body: Some("17% of runs failed this week.".into()),
        link: None,
        email_subject: None,
        email_html: None,
        email_text: None,
    }
}

async fn send(org: &Org, server: &MockServer, user: Uuid) -> ChannelOutcome {
    send_run(org, server, &[user]).await.remove(0)
}

/// One delivery run to `users`, in order, as `delivery` makes it: one
/// [`SlackDm`] for the whole send.
async fn send_run(org: &Org, server: &MockServer, users: &[Uuid]) -> Vec<ChannelOutcome> {
    let dm = SlackDm::load(
        &org.db,
        org.id,
        users,
        SlackClient::with_base_url(server.uri()),
    )
    .await
    .expect("load")
    .expect("the org has Slack");
    let content = content();
    let message = SlackMessage {
        content: &content,
        url: Some("https://app.example/customer-apps/o/a/?account=1"),
        sender: "Accounts Insight",
    };
    let mut out = Vec::new();
    for user in users {
        out.push(dm.send(*user, &message).await);
    }
    out
}

async fn link_of(org: &Org, user: Uuid) -> Option<slack_user_links::Model> {
    slack_user_links::Entity::find()
        .filter(slack_user_links::Column::InstallationId.eq(org.installation))
        .filter(slack_user_links::Column::OxyUserId.eq(user))
        .one(&org.db)
        .await
        .expect("read link")
}

#[tokio::test]
async fn a_member_who_never_talked_to_the_bot_is_found_by_email_once_and_dmed() {
    if db_unavailable() {
        return;
    }
    let org = seed_org(WITH_LOOKUP).await;
    let (member, email) = seed_user(&org.db).await;
    join(&org, member).await;
    let server = MockServer::start().await;
    let slack_user = format!("U{}", &member.simple().to_string()[..9]);
    lookup(
        &server,
        &email,
        200,
        json!({ "ok": true, "user": { "id": slack_user } }),
        1,
    )
    .await;
    dm(&server, &slack_user, 2).await;

    assert_eq!(send(&org, &server, member).await, ChannelOutcome::Sent);
    let link = link_of(&org, member).await.expect("linked");
    assert_eq!(link.slack_user_id, slack_user);
    assert_eq!(link.link_method, "email_lookup");

    // The next notification goes through the saved link: no second lookup.
    assert_eq!(send(&org, &server, member).await, ChannelOutcome::Sent);
    server.verify().await;
}

#[tokio::test]
async fn someone_not_in_slack_is_not_linked_and_not_looked_up_again_within_the_hour() {
    if db_unavailable() {
        return;
    }
    let org = seed_org(WITH_LOOKUP).await;
    let (member, email) = seed_user(&org.db).await;
    join(&org, member).await;
    let server = MockServer::start().await;
    lookup(
        &server,
        &email,
        200,
        json!({ "ok": false, "error": "users_not_found" }),
        1,
    )
    .await;

    assert_eq!(
        send(&org, &server, member).await,
        ChannelOutcome::Skipped("not_linked")
    );
    assert_eq!(
        send(&org, &server, member).await,
        ChannelOutcome::Skipped("not_linked")
    );
    assert!(link_of(&org, member).await.is_none());
    server.verify().await;
}

#[tokio::test]
async fn a_bot_or_a_slack_account_linked_to_someone_else_is_never_used() {
    if db_unavailable() {
        return;
    }
    let org = seed_org(WITH_LOOKUP).await;
    let server = MockServer::start().await;

    // A bot account with the member's address.
    let (botted, bot_email) = seed_user(&org.db).await;
    join(&org, botted).await;
    lookup(
        &server,
        &bot_email,
        200,
        json!({ "ok": true, "user": { "id": "UBOT", "is_bot": true } }),
        1,
    )
    .await;
    assert_eq!(
        send(&org, &server, botted).await,
        ChannelOutcome::Skipped("not_linked")
    );

    // A Slack account another member linked first, by magic link.
    let (first, _) = seed_user(&org.db).await;
    join(&org, first).await;
    let taken = format!("U{}", &first.simple().to_string()[..9]);
    UserLinksService::create(CreateLink {
        installation_id: org.installation,
        slack_user_id: taken.clone(),
        oxy_user_id: first,
        link_method: LinkMethod::MagicLink,
    })
    .await
    .expect("seed link");
    let (second, second_email) = seed_user(&org.db).await;
    join(&org, second).await;
    lookup(
        &server,
        &second_email,
        200,
        json!({ "ok": true, "user": { "id": taken } }),
        1,
    )
    .await;
    dm(&server, &taken, 0).await;

    assert_eq!(
        send(&org, &server, second).await,
        ChannelOutcome::Skipped("not_linked")
    );
    assert!(link_of(&org, second).await.is_none());
    assert_eq!(
        link_of(&org, first).await.expect("kept").slack_user_id,
        taken
    );
    server.verify().await;
}

#[tokio::test]
async fn nobody_outside_the_org_and_no_install_without_the_scope_is_looked_up() {
    if db_unavailable() {
        return;
    }
    let server = MockServer::start().await;

    // Not a member of the org that installed Slack.
    let org = seed_org(WITH_LOOKUP).await;
    let (outsider, outsider_email) = seed_user(&org.db).await;
    lookup(
        &server,
        &outsider_email,
        200,
        json!({ "ok": true, "user": { "id": "UOUT" } }),
        0,
    )
    .await;
    assert_eq!(
        send(&org, &server, outsider).await,
        ChannelOutcome::Skipped("not_linked")
    );

    // An installation granted before `users:read.email` was requested.
    let old = seed_org("chat:write,users:read").await;
    let (member, email) = seed_user(&old.db).await;
    join(&old, member).await;
    lookup(
        &server,
        &email,
        200,
        json!({ "ok": true, "user": { "id": "UOLD" } }),
        0,
    )
    .await;
    assert_eq!(
        send(&old, &server, member).await,
        ChannelOutcome::Skipped("not_linked")
    );
    server.verify().await;
}

#[tokio::test]
async fn a_rate_limited_lookup_fails_the_delivery_and_stops_the_runs_other_lookups() {
    if db_unavailable() {
        return;
    }
    let org = seed_org(WITH_LOOKUP).await;
    let (first, first_email) = seed_user(&org.db).await;
    let (second, second_email) = seed_user(&org.db).await;
    let (linked, _) = seed_user(&org.db).await;
    for user in [first, second, linked] {
        join(&org, user).await;
    }
    let linked_slack = format!("U{}", &linked.simple().to_string()[..9]);
    UserLinksService::create(CreateLink {
        installation_id: org.installation,
        slack_user_id: linked_slack.clone(),
        oxy_user_id: linked,
        link_method: LinkMethod::MagicLink,
    })
    .await
    .expect("seed link");
    let server = MockServer::start().await;
    lookup(
        &server,
        &first_email,
        429,
        json!({ "ok": false, "error": "ratelimited" }),
        1,
    )
    .await;
    // Throttled: asking again in the same run would only be another 429.
    lookup(
        &server,
        &second_email,
        200,
        json!({ "ok": true, "user": { "id": "USECOND" } }),
        0,
    )
    .await;
    dm(&server, &linked_slack, 1).await;

    assert_eq!(
        send_run(&org, &server, &[first, second, linked]).await,
        vec![
            ChannelOutcome::Failed("provider_error"),
            ChannelOutcome::Failed("provider_error"),
            ChannelOutcome::Sent,
        ],
        "both unlinked people are left for the retry; the linked one is still told"
    );
    assert!(link_of(&org, first).await.is_none());
    assert!(link_of(&org, second).await.is_none());
    server.verify().await;
}
