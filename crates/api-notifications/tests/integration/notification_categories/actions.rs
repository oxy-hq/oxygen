//! What tapping an action resolves to: the link to open, the function to run
//! as the caller, or a refusal — read against the catalogue the notification
//! is read against today.

use oxy_api_notifications::action_call::FunctionCall;
use oxy_api_notifications::actions::{Plan, Refused, plan};
use oxy_api_notifications::inbox_view::Links;
use serde_json::json;

use super::fixture::{APP_SLUG, Signal, publish, seed, signal};
use crate::common::Schema;
use crate::oltp_fx::with_fx_on;

#[tokio::test]
async fn notification_categories_an_action_opens_runs_a_function_or_is_refused() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let db = &fx.db;
        let mine = |kind, app| Signal {
            org: fx.org_id,
            user: w.member.id,
            app,
            kind,
            thread: None,
            ago: 1,
            read: false,
        };
        let alarm = signal(db, mine("temp_alarm", Some(w.app.id))).await;
        let overdue = signal(db, mine("list_overdue", Some(w.app.id))).await;
        let run = signal(db, mine("oxy.run_failed", None)).await;
        let links = Links::default();
        let tap = |id, action: &'static str, user| {
            let links = links.clone();
            async move { plan(db, user, id, action, &links).await.map(|(_, p)| p) }
        };

        assert_eq!(
            tap(alarm, "ack", &w.member).await,
            Ok(Plan::Function(FunctionCall {
                org_slug: format!("oltp-test-{}", fx.org_id.simple()),
                app_slug: APP_SLUG.into(),
                function: "ack-alarm".into(),
                notification_id: alarm,
                action: "ack".into(),
                link: None,
            }))
        );
        assert_eq!(
            tap(overdue, "open", &w.member).await,
            Ok(Plan::Open { url: None })
        );
        assert_eq!(
            tap(run, "open", &w.member).await,
            Ok(Plan::Open { url: None })
        );

        // Not mine: nothing to say about it.
        assert_eq!(tap(alarm, "ack", &w.other).await, Err(Refused::NotFound));
        // Oxygen's actions are code; one it lacks never existed.
        assert_eq!(tap(run, "ack", &w.member).await, Err(Refused::NotFound));
        assert_eq!(
            tap(alarm, "Bad Id", &w.member).await,
            Err(Refused::NotFound)
        );
        // The app's live build does not declare it.
        assert!(matches!(
            tap(alarm, "snooze", &w.member).await,
            Err(Refused::NotDeclared(_))
        ));

        // A new build drops the action: the old notification's button is 409.
        let mut m = super::fixture::manifest();
        m["notifications"]["kinds"]["temp_alarm"]["actions"] = json!([]);
        publish(db, &w.app, Some(m)).await;
        assert!(matches!(
            tap(alarm, "ack", &w.member).await,
            Err(Refused::NotDeclared(_))
        ));
    })
    .await;
}
