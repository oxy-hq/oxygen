//! `GET /api/notifications/catalog`: which sources a person sees, the tree
//! under each, and the mutes and unread counts at every node.

use oxy_api_notifications::catalog::handler::catalog_for;
use oxy_api_notifications::delivery::Channel;
use oxy_api_notifications::scope_mutes;
use oxy_api_notifications::settings::model::Node;
use oxy_api_notifications::settings::store;
use serde_json::{Value, json};

use super::fixture::{Signal, manifest, seed, seed_app, signal};
use crate::common::Schema;
use crate::oltp_fx::with_fx_on;
use serde_json::Value as Json;

#[tokio::test]
async fn notification_categories_the_catalogue_lists_each_source_as_a_tree() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let db = &fx.db;
        let me = &w.member;
        // A second app with a catalogue that never notified anyone: listed,
        // the member is in its audience. A third without one: not listed.
        seed_app(db, fx.org_id, w.workspace, "quiet", Some(manifest())).await;
        seed_app(db, fx.org_id, w.workspace, "bare", None).await;
        let unread = |kind, app| Signal {
            org: fx.org_id,
            user: me.id,
            app,
            kind,
            thread: None,
            ago: 1,
            read: false,
        };
        signal(db, unread("temp_alarm", Some(w.app.id))).await;
        signal(db, unread("temp_alarm", Some(w.app.id))).await;
        signal(db, unread("old_kind", Some(w.app.id))).await;
        signal(db, unread("oxy.run_failed", None)).await;
        let overdue = Node::Kind("list_overdue".into());
        store::set_channel(
            db,
            me.id,
            Some(w.app.id),
            &overdue,
            Channel::Email,
            Some(false),
        )
        .await
        .expect("kind mute");
        let mute = scope_mutes::set(db, me, Some(w.app.id), "operations", Channel::Slack, true);
        mute.await.expect("category mute");

        let catalog = catalog_for(db, me).await.expect("catalogue");
        let v = serde_json::to_value(&catalog).unwrap();
        let labels: Vec<&str> = v["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, vec!["Oxygen", "App quiet", "App store-ops"]);

        let oxygen = &v["sources"][0];
        assert_eq!(
            (oxygen["source"].as_str(), &oxygen["app"]),
            (Some("oxygen"), &Value::Null)
        );
        assert_eq!(oxygen["unread"], 1);
        // A plain member sees Workspaces — "your apps are ready" is for
        // everyone — and the failed run they were actually told about. Billing,
        // Monitoring, Ask and Access are not theirs, and no longer appear just
        // because one Oxygen notice did.
        assert_eq!(oxygen_paths(oxygen), vec!["runs", "workspaces"]);
        let run = &oxygen["categories"][0]["kinds"][0];
        assert_eq!(run["kind"], "oxy.run_failed");
        assert_eq!(run["stream"], "operations");
        assert_eq!(v["operator"], false);

        assert_app_source(&v["sources"][2], fx.org_id);
        assert_eq!(v["reach"]["push_devices"], 0);

        // Outside the org, nothing of it.
        let theirs = catalog_for(db, &w.stranger).await.expect("catalogue");
        assert!(theirs.sources.is_empty(), "{:?}", theirs.sources.len());
    })
    .await;
}

/// The store-ops app's own tree: its nesting, its labels, and the mutes at the
/// node each was set on.
///
/// Its own function because the test above would otherwise be one closure doing
/// two jobs, over the complexity limit the code-metrics gate holds a diff to —
/// and because an app's tree has nothing to do with Oxygen's segmentation,
/// which is what that test is now mostly about.
fn assert_app_source(app: &Json, org_id: uuid::Uuid) {
    assert_eq!(app["source"], "app");
    assert_eq!(app["app"]["slug"], "store-ops");
    assert_eq!(
        app["app"]["org_slug"],
        format!("oltp-test-{}", org_id.simple())
    );
    assert_eq!(app["unread"], 3);
    assert_eq!(app["muted"], json!([]));
    let ops = &app["categories"][0];
    assert_eq!(
        (ops["path"].as_str(), ops["label"].as_str()),
        (Some("operations"), Some("Store operations"))
    );
    assert_eq!(ops["muted"], json!(["slack"]));
    assert_eq!(ops["unread"], 2, "its subtree's unread");
    let safety = &ops["categories"][0];
    assert_eq!(safety["description"], "Temperature logs and checks");
    assert_eq!(
        safety["muted"],
        json!([]),
        "a parent's mute is not repeated"
    );
    let alarm = &safety["kinds"][0];
    assert_eq!(alarm["kind"], "temp_alarm");
    assert_eq!(alarm["channels"], json!(["push", "email", "slack"]));
    assert_eq!(alarm["actions"][0]["runs_function"], true);
    // Every app kind is Activity: an app has no operator tier of its own.
    assert_eq!(alarm["stream"], "activity");
    let overdue = ops["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "list_overdue")
        .expect("listed");
    assert_eq!(overdue["muted"], json!(["email"]));
    let general = &app["categories"][1];
    assert_eq!(general["path"], "general");
    let general_kinds: Vec<&str> = general["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["kind"].as_str().unwrap())
        .collect();
    assert_eq!(general_kinds, vec!["old_kind", "shift_note"]);
}

/// The Oxygen source's category paths, in the order the catalogue gives them.
fn oxygen_paths(oxygen: &Json) -> Vec<&str> {
    oxygen["categories"]
        .as_array()
        .map(|cs| cs.iter().map(|c| c["path"].as_str().unwrap()).collect())
        .unwrap_or_default()
}

/// Who sees which of Oxygen's categories, and who gets the stream tabs.
///
/// The question the whole segmentation exists to answer: a crew member and the
/// contractor who writes that chain's pipelines had the same tree, and one
/// `oxy.workspace_ready` — which every member of an org's first workspace gets
/// — used to unlock Billing and Automations switches for both of them for good.
#[tokio::test]
async fn notification_categories_oxygens_tree_is_cut_to_who_each_person_is() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let db = &fx.db;
        let ready = |user| Signal {
            org: fx.org_id,
            user,
            app: None,
            kind: "oxy.workspace_ready",
            thread: None,
            ago: 1,
            read: false,
        };
        // Everyone gets the same notice — the one that used to open everything.
        for person in [&w.member, &w.owner, &w.admin, &w.grantee] {
            signal(db, ready(person.id)).await;
        }

        let seen = |person: &'static str, user: &oxy_auth::types::AuthenticatedUser| {
            let user = user.clone();
            async move {
                let catalog = catalog_for(db, &user).await.expect(person);
                let v = serde_json::to_value(&catalog).unwrap();
                let oxygen = v["sources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["source"] == "oxygen")
                    .cloned()
                    .unwrap_or(Json::Null);
                (
                    oxygen_paths(&oxygen)
                        .into_iter()
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                    v["operator"].as_bool(),
                )
            }
        };

        let (member, member_operator) = seen("member", &w.member).await;
        assert_eq!(member, vec!["workspaces"], "a crew member: the one notice");
        // The point of the whole change, said once: the categories an operator
        // sees and a crew member does not.
        assert!(!member.contains(&"billing".to_string()));
        assert!(!member.contains(&"runs".to_string()));
        assert_eq!(member_operator, Some(false), "and no stream tabs");

        let (owner, owner_operator) = seen("owner", &w.owner).await;
        assert_eq!(owner, vec!["billing", "runs", "workspaces"]);
        assert_eq!(owner_operator, Some(true));

        let (admin, admin_operator) = seen("admin", &w.admin).await;
        assert_eq!(
            admin,
            vec!["runs", "workspaces"],
            "billing_past_due goes to owners only, so an admin gets no \
             switch for a notice they can never receive"
        );
        assert_eq!(admin_operator, Some(true));

        let (grantee, grantee_operator) = seen("grantee", &w.grantee).await;
        assert_eq!(
            grantee,
            vec!["runs", "workspaces"],
            "a Factory grant is operator standing, with none in the org"
        );
        assert_eq!(grantee_operator, Some(true));
    })
    .await;
}

/// The finding the design's builder review turned up (3.1), pinned: narrowing
/// `oxy.run_failed` to the people this audience admits would have silently
/// stopped telling a workspace's own admins that their pipeline failed, because
/// `Ring::Factory` does not admit a workspace role on its own.
///
/// Nothing narrows a send today — the audience is read-path only — and this is
/// the test that fails if that ever changes by accident.
#[tokio::test]
async fn notification_categories_a_plain_member_still_sees_a_run_failure_they_were_sent() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let db = &fx.db;
        signal(
            db,
            Signal {
                org: fx.org_id,
                user: w.member.id,
                app: None,
                kind: "oxy.run_failed",
                thread: None,
                ago: 1,
                read: false,
            },
        )
        .await;
        let catalog = catalog_for(db, &w.member).await.expect("catalogue");
        let v = serde_json::to_value(&catalog).unwrap();
        let oxygen = &v["sources"][0];
        assert_eq!(
            oxygen_paths(oxygen),
            vec!["runs", "workspaces"],
            "the switch for what they were sent is reachable"
        );
        assert_eq!(oxygen["unread"], 1, "and it is still counted");
        assert_eq!(v["operator"], false, "without making them an operator");
    })
    .await;
}
