use chrono::Utc;
use entity::{notifications, org_members, org_team_members, org_teams};
use oxy_app::server::experiment::{lever, notify};
use sea_orm::{ActiveModelTrait, ActiveValue::Set};
use uuid::Uuid;

use super::fixture::*;

#[tokio::test]
async fn a_lever_resolves_by_name_at_the_promoted_revision() {
    // PR 3's resolver, as the runner calls it: the path the runner loads and
    // the revision it was found at — the two things a flip records.
    let fx = Fx::seed(DESIGN).await;
    let head = fx.head().await;
    let lever = lever::resolve_for_runner(&fx.db, fx.ws, "promo_on")
        .await
        .unwrap()
        .expect("resolves");
    assert_eq!(
        (lever.file_path.as_str(), lever.revision_id),
        ("automations/promo_on.automation.yml", head)
    );
    // Renamed after registration: the promoted revision no longer has it.
    fx.promote(
        utc("2026-02-05T00:00:00Z"),
        DESIGN,
        &["promo_start", "promo_off"],
    )
    .await;
    let missing = lever::resolve_for_runner(&fx.db, fx.ws, "promo_on")
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        missing.contains("promo_on"),
        "PR 3's refusal names the automation: {missing}"
    );
}

#[tokio::test]
async fn an_owner_email_notifies_that_member_once_per_reason() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    notify::notify_owner(&fx.db, &run, OWNER_EMAIL, "aborted on request: test").await;
    notify::notify_owner(&fx.db, &run, OWNER_EMAIL, "aborted on request: test").await;
    let notices = fx.notices().await;
    assert_eq!(notices.len(), 1, "one notice per (run, reason)");
    assert_eq!(notices[0].user_id, fx.owner);
    assert_eq!(notices[0].subject_kind.as_deref(), Some("experiment_run"));
    assert_eq!(
        notices[0].subject_id.as_deref(),
        Some(run.id.to_string().as_str())
    );
    assert_eq!(notices[0].body.as_deref(), Some("aborted on request: test"));
}

#[tokio::test]
async fn an_owner_email_matches_its_member_whatever_the_case() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    let shouted = OWNER_EMAIL.to_ascii_uppercase();
    let (people, resolved) = notify::recipients(&fx.db, fx.org, &shouted)
        .await
        .expect("recipients");
    assert!(resolved, "{shouted} is the owner, not an unknown name");
    assert_eq!(people, vec![fx.owner]);
}

#[tokio::test]
async fn an_owner_team_notifies_its_members() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    let analyst = seed_member(
        &fx.db,
        fx.org,
        "analyst@acme.test",
        org_members::OrgRole::Member,
    )
    .await;
    let now = Utc::now().fixed_offset();
    let team = org_teams::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(fx.org),
        name: Set("ops".into()),
        description: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        created_by: Set(None),
    }
    .insert(&fx.db)
    .await
    .expect("seed team");
    org_team_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        team_id: Set(team.id),
        user_id: Set(analyst),
        created_at: Set(now),
        created_by: Set(None),
    }
    .insert(&fx.db)
    .await
    .expect("seed team member");

    notify::notify_owner(&fx.db, &run, "ops", "guardrail breached: x").await;
    let notices = fx.notices().await;
    assert_eq!(
        notices.iter().map(|n| n.user_id).collect::<Vec<_>>(),
        vec![analyst]
    );
}

#[tokio::test]
async fn an_owner_that_matches_nobody_reaches_the_org_officers_and_says_so() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    notify::notify_owner(
        &fx.db,
        &run,
        "nobody@elsewhere.test",
        "guardrail breached: x",
    )
    .await;
    let notices = fx.notices().await;
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].user_id, fx.owner, "the org owner");
    assert!(
        notices[0].title.contains("matched no member or team"),
        "{}",
        notices[0].title
    );
}

#[tokio::test]
async fn a_notice_half_written_still_reaches_the_recipients_left_out() {
    // A DB error mid-loop leaves some recipients told and some not; the retry
    // must tell only the rest.
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    let analyst = seed_member(
        &fx.db,
        fx.org,
        "analyst@acme.test",
        org_members::OrgRole::Member,
    )
    .await;
    let now = Utc::now().fixed_offset();
    let team = org_teams::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(fx.org),
        name: Set("ops".into()),
        description: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        created_by: Set(None),
    }
    .insert(&fx.db)
    .await
    .expect("seed team");
    for user_id in [fx.owner, analyst] {
        org_team_members::ActiveModel {
            id: Set(Uuid::new_v4()),
            team_id: Set(team.id),
            user_id: Set(user_id),
            created_at: Set(now),
            created_by: Set(None),
        }
        .insert(&fx.db)
        .await
        .expect("seed team member");
    }
    let reason = "guardrail breached: x";
    notifications::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(fx.org),
        user_id: Set(fx.owner),
        kind: Set(notify::NOTICE_KIND.into()),
        title: Set("Experiment".into()),
        body: Set(Some(reason.into())),
        link: Set(None),
        subject_kind: Set(Some(notify::SUBJECT_KIND.into())),
        subject_id: Set(Some(run.id.to_string())),
        created_at: Set(Utc::now().fixed_offset()),
        read_at: Set(None),
        message_id: Set(None),
        app_id: Set(None),
        thread_key: Set(None),
    }
    .insert(&fx.db)
    .await
    .expect("the first recipient was written before the error");

    notify::notify_owner(&fx.db, &run, "ops", reason).await;
    notify::notify_owner(&fx.db, &run, "ops", reason).await;
    let mut users: Vec<Uuid> = fx.notices().await.iter().map(|n| n.user_id).collect();
    users.sort();
    let mut want = vec![fx.owner, analyst];
    want.sort();
    assert_eq!(users, want, "each recipient told exactly once");
}
