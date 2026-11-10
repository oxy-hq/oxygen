//! Who may tell whom, decided two ways that must agree: per app
//! (`preferences::may_be_told_by`, through `app_audience` — the rule
//! `ctx.notify` resolves with) and for every app at once
//! (`catalog::audience::told_by_many`, from the person's own rows) — and the
//! catalogue costing the same number of queries however many apps and org
//! members there are.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use entity::{app_members, apps, org_frontline_members};
use oxy_api_notifications::catalog::audience::told_by_many;
use oxy_api_notifications::catalog::handler::catalog_for;
use oxy_api_notifications::preferences::may_be_told_by;
use oxy_app_core::audience::PersonReach;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection};
use uuid::Uuid;

use super::fixture::{
    Signal, draft, join, manifest, seed, seed_app, seed_bare_app, seed_user, signal, signed_in,
};
use crate::common::Schema;
use crate::oltp_fx::{seed_org, with_fx_on};

async fn crew(db: &DatabaseConnection, org: Uuid, status: &str, grants: &[Uuid]) -> Uuid {
    let user = seed_user(db).await;
    org_frontline_members::ActiveModel {
        org_id: Set(org),
        user_id: Set(user),
        status: Set(status.into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("enrol the worker");
    for app in grants {
        app_members::ActiveModel {
            id: Set(Uuid::new_v4()),
            app_id: Set(*app),
            user_id: Set(user),
            role: Set("member".into()),
            created_at: Set(Utc::now().fixed_offset()),
            created_by: Set(None),
        }
        .insert(db)
        .await
        .expect("grant the app");
    }
    user
}

/// Each of `apps` as `may_be_told_by` answers it, after checking the batched
/// path answers the same.
async fn told(db: &DatabaseConnection, user: Uuid, apps: &[&apps::Model]) -> Vec<bool> {
    let reach = PersonReach::load(db, user).await.expect("reach");
    let rows: Vec<apps::Model> = apps.iter().map(|a| (*a).clone()).collect();
    let batched = told_by_many(db, user, &reach, &rows)
        .await
        .expect("batched");
    let mut out = Vec::new();
    for app in apps {
        let one = may_be_told_by(db, user, app).await.expect("per app");
        assert_eq!(
            one,
            batched.contains(&app.id),
            "the per-app and batched paths disagree on {} for {user}",
            app.slug
        );
        out.push(one);
    }
    out
}

#[tokio::test]
async fn notification_categories_who_may_tell_whom_is_one_rule_per_app_or_batched() {
    with_fx_on(Schema::All, |fx| async move {
        let db = &fx.db;
        let w = seed(db, fx.org_id).await;
        let store = &w.app;
        // The rule does not narrow by visibility: an app restricted to its
        // granted members and teams still reaches every org member.
        let mut restricted: apps::ActiveModel =
            seed_app(db, fx.org_id, w.workspace, "restricted", Some(manifest()))
                .await
                .into();
        restricted.visibility = Set("members".into());
        let restricted = restricted.update(db).await.expect("restrict the app");
        let bare = seed_bare_app(db, fx.org_id, w.workspace, "drafted").await;
        let drafted = draft(db, &bare, Some(manifest())).await;
        let elsewhere_org = seed_org(db).await;
        let elsewhere = seed_app(db, elsewhere_org, w.workspace, "elsewhere", None).await;
        let apps = [store, &restricted, &drafted, &elsewhere];

        let granted = crew(db, fx.org_id, "active", &[store.id]).await;
        let ungranted = crew(db, fx.org_id, "active", &[]).await;
        let suspended = crew(db, fx.org_id, "suspended", &[store.id]).await;
        // Granted on an app of an org they are not crew in.
        let crew_elsewhere = crew(db, fx.org_id, "active", &[elsewhere.id]).await;
        let receiver = seed_user(db).await;
        signal(
            db,
            Signal {
                org: elsewhere_org,
                user: receiver,
                app: Some(elsewhere.id),
                kind: "late",
                thread: None,
                ago: 1,
                read: false,
            },
        )
        .await;

        let cases = [
            ("a member", w.member.id, [true, true, true, false]),
            ("crew granted one app", granted, [true, false, false, false]),
            ("crew without a grant", ungranted, [false; 4]),
            ("suspended crew with a grant", suspended, [false; 4]),
            ("crew granted another org's app", crew_elsewhere, [false; 4]),
            ("received from it", receiver, [false, false, false, true]),
            ("nobody", w.stranger.id, [false; 4]),
        ];
        for (who, user, expected) in cases {
            assert_eq!(told(db, user, &apps).await, expected, "{who}");
        }
    })
    .await;
}

/// Statements `catalog_for` sends for `user`, counted on a handle of its own.
async fn queries_for(db: &DatabaseConnection, user: &AuthenticatedUser) -> (usize, usize) {
    let count = Arc::new(AtomicUsize::new(0));
    let mut counted = db.clone();
    let seen = count.clone();
    counted.set_metric_callback(move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
    });
    let catalog = catalog_for(&counted, user).await.expect("catalogue");
    (count.load(Ordering::Relaxed), catalog.sources.len())
}

#[tokio::test]
async fn notification_categories_the_catalogue_costs_the_same_for_one_app_or_many() {
    with_fx_on(Schema::All, |fx| async move {
        let db = &fx.db;
        let w = seed(db, fx.org_id).await;
        let bare = seed_bare_app(db, fx.org_id, w.workspace, "draft-0").await;
        draft(db, &bare, Some(manifest())).await;
        signal(
            db,
            Signal {
                org: fx.org_id,
                user: w.member.id,
                app: Some(w.app.id),
                kind: "temp_alarm",
                thread: None,
                ago: 1,
                read: false,
            },
        )
        .await;
        let (few, listed) = queries_for(db, &w.member).await;
        assert_eq!(listed, 2, "both apps are listed");

        // Ten more apps with catalogues — two of them never promoted, whose
        // live build is their draft — and a roster of forty more people.
        for i in 0..8 {
            seed_app(
                db,
                fx.org_id,
                w.workspace,
                &format!("app-{i}"),
                Some(manifest()),
            )
            .await;
        }
        for i in 1..3 {
            let bare = seed_bare_app(db, fx.org_id, w.workspace, &format!("draft-{i}")).await;
            draft(db, &bare, Some(manifest())).await;
        }
        for _ in 0..40 {
            join(db, fx.org_id, seed_user(db).await).await;
        }
        crew(db, fx.org_id, "active", &[w.app.id]).await;

        let (many, listed) = queries_for(db, &w.member).await;
        assert_eq!(listed, 12, "every app is listed");
        assert_eq!(
            many, few,
            "the catalogue's queries must not grow with apps or members"
        );
        // A loose ceiling as well, so a constant that creeps up is seen too.
        assert!(few <= 20, "{few} queries for one person's catalogue");

        // Someone whose standing is a frontline grant pays the same.
        let worker = signed_in(crew(db, fx.org_id, "active", &[w.app.id]).await);
        let (theirs, listed) = queries_for(db, &worker).await;
        assert_eq!(listed, 1, "their one granted app");
        assert!(theirs <= few + 1, "{theirs} queries for a worker");
    })
    .await;
}
