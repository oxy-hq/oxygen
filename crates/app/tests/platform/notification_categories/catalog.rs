//! `GET /api/notifications/catalog`: which sources a person sees, the tree
//! under each, and the mutes and unread counts at every node.

use oxy_app::server::api::notifications::catalog::handler::catalog_for;
use oxy_app::server::api::notifications::delivery::Channel;
use oxy_app::server::api::notifications::scope_mutes;
use oxy_app::server::api::notifications::settings::model::Node;
use oxy_app::server::api::notifications::settings::store;
use serde_json::{Value, json};

use super::fixture::{Signal, manifest, seed, seed_app, signal};
use crate::common::Schema;
use crate::oltp_provisioner::with_fx_on;

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
        let paths: Vec<&str> = oxygen["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            vec![
                "access",
                "ask",
                "billing",
                "monitoring",
                "runs",
                "workspaces"
            ]
        );
        // Categories sort by path, so "access" and "ask" precede "billing".
        let billing = &oxygen["categories"][2]["kinds"][0];
        assert_eq!(
            (billing["critical"].as_bool(), billing["level"].as_str()),
            (Some(true), Some("critical"))
        );

        let app = &v["sources"][2];
        assert_eq!(app["source"], "app");
        assert_eq!(app["app"]["slug"], "store-ops");
        assert_eq!(
            app["app"]["org_slug"],
            format!("oltp-test-{}", fx.org_id.simple())
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
        assert_eq!(v["reach"]["push_devices"], 0);

        // Outside the org, nothing of it.
        let theirs = catalog_for(db, &w.stranger).await.expect("catalogue");
        assert!(theirs.sources.is_empty(), "{:?}", theirs.sources.len());
    })
    .await;
}
