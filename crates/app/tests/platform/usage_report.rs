//! The weekly custom-app usage report, against a real Postgres.
//!
//! What is SQL and nothing else: that the counts are the week's and the week
//! before's, production only, with an org's people counted once across its
//! apps; that one report is written per week however often the pass runs; and
//! that each reader is mailed once, their own slice, unless they said no.
//!
//! The rules for what a report calls out are pure and tested beside them
//! (`usage_report::highlights`). Own database per test through
//! `common::fresh_db`.

mod world;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Duration;
use oxy_app::emails::usage_report::{DeliveryMode, ReportMailer};
use oxy_app::server::api::admin::usage_report::collect::collect;
use oxy_app::server::api::admin::usage_report::{delivery, job, store};

use world::{Outbox, World, now};

#[tokio::test]
async fn the_week_is_counted_in_production_only_with_each_org_s_people_counted_once() {
    let w = World::new().await;
    let poke = w.org("Poke House").await;
    let store_ops = w.app(poke, "Store Ops", true).await;
    let checklists = w.app(poke, "Checklists", true).await;
    let (ana, ben, cy, dev) = (
        w.user("ana@poke.test").await,
        w.user("ben@poke.test").await,
        w.user("cy@poke.test").await,
        w.user("dev@poke.test").await,
    );

    // This week: Ana twice and Ben once on Store Ops; Ana on Checklists too.
    let ana_session = w.view(store_ops, ana, w.this_week(10), "production").await;
    w.view(store_ops, ana, w.this_week(30), "production").await;
    w.view(store_ops, ben, w.this_week(50), "production").await;
    w.view(checklists, ana, w.this_week(60), "production").await;
    // A developer's sandbox is not usage.
    w.view(store_ops, dev, w.this_week(70), "dev-dev").await;
    // The week before: three people.
    for user in [ana, ben, cy] {
        w.view(store_ops, user, w.week_before(20), "production")
            .await;
    }
    // Outside both weeks: neither counted.
    w.view(
        store_ops,
        cy,
        w.period.end + Duration::hours(1),
        "production",
    )
    .await;

    // One session reported an error twice; a pageview is not an error.
    w.event(store_ops, ana, ana_session, "oxy-error", w.this_week(11))
        .await;
    w.event(store_ops, ana, ana_session, "oxy-error", w.this_week(12))
        .await;
    w.event(store_ops, ana, ana_session, "oxy-pageview", w.this_week(13))
        .await;

    for status in [
        "success",
        "success",
        "success",
        "error",
        "timeout",
        "cancelled",
    ] {
        w.call(store_ops, status, w.this_week(40), "production")
            .await;
    }
    w.call(store_ops, "error", w.this_week(41), "staging").await;
    w.call(store_ops, "error", w.week_before(41), "production")
        .await;

    // Two releases to production this week and one the week before. A publish
    // to staging and a rollback are not releases.
    w.release(store_ops, "production", "promote", w.this_week(20))
        .await;
    w.release(store_ops, "production", "promote", w.this_week(90))
        .await;
    w.release(store_ops, "production", "promote", w.week_before(20))
        .await;
    w.release(store_ops, "staging", "publish", w.this_week(21))
        .await;
    w.release(store_ops, "production", "rollback", w.this_week(22))
        .await;

    // Measured before the week, during it, and after it ended.
    w.stored(store_ops, 400, w.week_before(100)).await;
    w.stored(store_ops, 900, w.this_week(100)).await;
    w.stored(store_ops, 5_000, w.period.end + Duration::hours(2))
        .await;

    let report = collect(&w.db, w.period).await.unwrap();
    assert_eq!(report.orgs.len(), 1);
    let org = &report.orgs[0];
    assert_eq!(org.name, "Poke House");
    assert_eq!((org.people, org.prev_people), (2, 3), "Ana is one person");

    let app = org.apps.iter().find(|a| a.app_id == store_ops).unwrap();
    assert_eq!(
        (app.current.views, app.current.people, app.current.sessions),
        (3, 2, 3)
    );
    assert_eq!((app.previous.views, app.previous.people), (3, 3));
    assert_eq!(app.current.error_sessions, 1);
    assert_eq!(
        (app.current.function_calls, app.current.function_failures),
        (6, 2)
    );
    assert_eq!(
        (app.previous.function_calls, app.previous.function_failures),
        (1, 1)
    );
    assert!(!app.first_week);
    assert_eq!((app.current.releases, app.previous.releases), (2, 1));
    // The size at the end of the week and at its start; the later sample is
    // next week's.
    assert_eq!(
        (app.storage_bytes, app.storage_bytes_before),
        (Some(900), Some(400))
    );
    // Busiest first.
    assert_eq!(org.apps[0].app_id, store_ops);
    assert_eq!(org.apps[1].current.people, 1);
    // Never measured is unknown, not empty.
    assert_eq!(org.apps[1].storage_bytes, None);
    assert_eq!(org.storage_bytes(), Some(900));
}

#[tokio::test]
async fn a_first_week_is_an_app_nobody_had_opened_before() {
    let w = World::new().await;
    let org = w.org("Rivermark").await;
    let new = w.app(org, "Crew Board", true).await;
    let returning = w.app(org, "Old Tool", true).await;
    w.app(org, "Draft", false).await;
    let idle_org = w.org("Not Started").await;
    w.app(idle_org, "Sketch", false).await;
    let kai = w.user("kai@rivermark.test").await;

    w.view(new, kai, w.this_week(5), "production").await;
    w.view(returning, kai, w.this_week(6), "production").await;
    w.view(returning, kai, w.week_before(-24 * 14), "production")
        .await;

    let report = collect(&w.db, w.period).await.unwrap();
    // An org whose only app is an unused draft has nothing to report.
    assert_eq!(report.orgs.len(), 1);
    let first: HashMap<&str, bool> = report.orgs[0]
        .apps
        .iter()
        .map(|a| (a.name.as_str(), a.first_week))
        .collect();
    assert_eq!(
        first,
        HashMap::from([("Crew Board", true), ("Old Tool", false)])
    );
}

#[tokio::test]
async fn one_report_is_written_per_week_and_each_reader_is_mailed_their_slice_once() {
    let w = World::new().await;
    // SAFETY: nextest runs each test in its own process (`fresh_db` asserts it).
    unsafe { std::env::set_var("OXY_OWNER", "root@usage.test") };
    let (poke, rivermark, empty) = (
        w.org("Poke House").await,
        w.org("Rivermark").await,
        w.org("Nobody Yet").await,
    );
    let user = w.user("ana@poke.test").await;
    let store_ops = w.app(poke, "Store Ops", true).await;
    let crew_board = w.app(rivermark, "Crew Board", true).await;
    w.app(empty, "Unopened", true).await;
    w.view(store_ops, user, w.this_week(10), "production").await;
    w.view(crew_board, user, w.this_week(11), "production")
        .await;

    w.grant("admin@usage.test", "global_admin", None).await;
    w.grant("bounded@usage.test", "global_admin", Some(&[rivermark]))
        .await;
    w.grant("empty@usage.test", "global_admin", Some(&[empty]))
        .await;
    w.grant("operator@usage.test", "app_operator", None).await;
    w.grant("quiet@usage.test", "global_admin", None).await;
    store::set_wants_email(&w.db, "Quiet@usage.test", false, "quiet@usage.test")
        .await
        .unwrap();

    // The pass runs many times a week; the week has one report.
    let report = job::report_to_deliver(&w.db, now()).await.unwrap().unwrap();
    let again = job::report_to_deliver(&w.db, now()).await.unwrap().unwrap();
    assert_eq!(report.id, again.id);
    assert_eq!(w.count("custom_app_usage_reports").await, 1);
    assert_eq!(report.snapshot.period, w.period);

    // Owed: root and the admins who did not say no. Not the App Operator.
    let owed = delivery::pending(&w.db, &report).await.unwrap();
    let mut emails: Vec<&str> = owed.iter().map(|s| s.email.as_str()).collect();
    emails.sort();
    assert_eq!(
        emails,
        [
            "admin@usage.test",
            "bounded@usage.test",
            "empty@usage.test",
            "root@usage.test"
        ]
    );

    // The provider refuses one address the first time.
    let outbox = Arc::new(Outbox::default());
    outbox
        .refuse_once
        .lock()
        .unwrap()
        .push("admin@usage.test".into());
    let mailer = ReportMailer::with_provider(outbox.clone(), DeliveryMode::Email);
    let done = delivery::deliver(&w.db, &report, owed, &mailer, None)
        .await
        .unwrap();
    assert_eq!((done.sent, done.failed, done.nothing_to_say), (2, 1, 1));
    assert_eq!(outbox.to(), ["bounded@usage.test", "root@usage.test"]);

    // Root reads every org; the bounded grant reads its own and no other.
    let root = outbox.text_for("root@usage.test");
    assert!(
        root.contains("Poke House") && root.contains("Rivermark"),
        "{root}"
    );
    let bounded = outbox.text_for("bounded@usage.test");
    assert!(bounded.contains("Rivermark"), "{bounded}");
    assert!(
        !bounded.contains("Poke House") && !bounded.contains("Store Ops"),
        "{bounded}"
    );

    // The refused send gave its claim back; the next pass sends it, and only it.
    // The reader with nothing in reach is asked about again, and still skipped.
    let owed = delivery::pending(&w.db, &report).await.unwrap();
    let mut emails: Vec<&str> = owed.iter().map(|s| s.email.as_str()).collect();
    emails.sort();
    assert_eq!(emails, ["admin@usage.test", "empty@usage.test"]);
    let done = delivery::deliver(&w.db, &report, owed, &mailer, None)
        .await
        .unwrap();
    assert_eq!((done.sent, done.failed), (1, 0));
    assert_eq!(
        outbox.to(),
        ["admin@usage.test", "bounded@usage.test", "root@usage.test"]
    );
    assert_eq!(w.count("custom_app_usage_report_deliveries").await, 3);
}

#[tokio::test]
async fn a_start_in_mid_week_writes_the_report_and_mails_nobody() {
    let w = World::new().await;
    let wednesday = w.period.end + Duration::days(2) + Duration::hours(9);
    assert!(
        job::report_to_deliver(&w.db, wednesday)
            .await
            .unwrap()
            .is_none(),
        "the report's Monday is over"
    );
    // The console still has last week to show.
    let written = store::latest(&w.db).await.unwrap().unwrap();
    assert_eq!(written.snapshot.period, w.period);
}

#[tokio::test]
async fn the_email_is_on_until_someone_turns_it_off_and_comes_back_on() {
    let w = World::new().await;
    assert!(store::wants_email(&w.db, "admin@usage.test").await.unwrap());
    store::set_wants_email(&w.db, "admin@usage.test", false, "Root@usage.test")
        .await
        .unwrap();
    assert!(
        !store::wants_email(&w.db, " Admin@Usage.Test ")
            .await
            .unwrap()
    );
    assert!(store::wants_email(&w.db, "other@usage.test").await.unwrap());
    // Who turned it off is kept with the answer.
    let stored = store::preferences(&w.db).await.unwrap();
    assert_eq!(
        stored["admin@usage.test"].updated_by.as_deref(),
        Some("root@usage.test")
    );

    store::set_wants_email(&w.db, "ADMIN@usage.test", true, "admin@usage.test")
        .await
        .unwrap();
    assert!(store::wants_email(&w.db, "admin@usage.test").await.unwrap());
    assert_eq!(w.count("staff_notification_preferences").await, 1);
}

#[tokio::test]
async fn an_admin_can_switch_the_email_off_for_another_person_but_only_for_a_recipient() {
    let w = World::new().await;
    // SAFETY: nextest runs each test in its own process (`fresh_db` asserts it).
    unsafe { std::env::set_var("OXY_OWNER", "root@usage.test") };
    w.grant("admin@usage.test", "global_admin", None).await;
    w.grant("operator@usage.test", "app_operator", None).await;

    let all = delivery::recipients(&w.db).await.unwrap();
    let emails: Vec<&str> = all.iter().map(|r| r.staff.email.as_str()).collect();
    assert_eq!(emails, ["root@usage.test", "admin@usage.test"]);
    assert!(all.iter().all(|r| r.enabled && r.updated_by.is_none()));

    let off = delivery::set_enabled(&w.db, "Admin@usage.test", false, "root@usage.test")
        .await
        .unwrap()
        .expect("the admin is a recipient");
    assert!(!off.enabled);
    assert_eq!(off.updated_by.as_deref(), Some("root@usage.test"));
    assert!(off.updated_at.is_some());

    // An App Operator is not someone the report is for: nothing is stored.
    let not_one = delivery::set_enabled(&w.db, "operator@usage.test", false, "root@usage.test")
        .await
        .unwrap();
    assert!(not_one.is_none());
    assert_eq!(w.count("staff_notification_preferences").await, 1);

    // The weekly pass no longer owes that person the report.
    let report = job::report_to_deliver(&w.db, now()).await.unwrap().unwrap();
    let owed = delivery::pending(&w.db, &report).await.unwrap();
    let emails: Vec<&str> = owed.iter().map(|s| s.email.as_str()).collect();
    assert_eq!(emails, ["root@usage.test"]);
}

#[tokio::test]
async fn old_reports_are_dropped_with_their_deliveries() {
    let w = World::new().await;
    let report = job::report_to_deliver(&w.db, now()).await.unwrap().unwrap();
    assert!(
        store::claim_delivery(&w.db, report.id, "root@usage.test")
            .await
            .unwrap()
    );
    assert!(
        !store::claim_delivery(&w.db, report.id, "Root@usage.test")
            .await
            .unwrap(),
        "an address is claimed once"
    );
    // Eleven weeks on it is still kept; twelve weeks on it is not.
    let later = |weeks: i64| w.period.start + Duration::weeks(weeks);
    assert_eq!(store::prune(&w.db, later(11)).await.unwrap(), 0);
    assert_eq!(store::prune(&w.db, later(12)).await.unwrap(), 1);
    assert_eq!(w.count("custom_app_usage_report_deliveries").await, 0);
    assert!(store::latest(&w.db).await.unwrap().is_none());
}
