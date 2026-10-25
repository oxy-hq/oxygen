//! `assemble::orgs` over hand-built rows: no database. What the reads select is
//! the integration test's (`tests/custom_apps/platform_facts.rs`); this is
//! where an app's customer use and a window's LLM cost are shaped and priced.

use super::app_use::{AppAtRow, AppCountRow};
use super::assemble;
use super::llm_cost::LlmCostRow;
use super::queries::{AppRow, OrgAtRow, OrgRow, Sources};
use super::*;
use chrono::TimeZone;

fn at(d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap()
}

fn org_row(id: Uuid) -> OrgRow {
    OrgRow {
        id,
        slug: "acme".into(),
        name: "Acme".into(),
        created_at: at(1, 0),
        members: 2,
        workspaces: 1,
        first_workspace_at: None,
    }
}

fn app_row(id: Uuid, org_id: Uuid, name: &str) -> AppRow {
    AppRow {
        id,
        org_id,
        workspace_id: Uuid::nil(),
        slug: name.to_lowercase(),
        name: name.into(),
        published_at: None,
    }
}

fn viewed(app_id: Uuid, at: DateTime<Utc>) -> AppAtRow {
    AppAtRow { app_id, at }
}

/// One org with the apps named, and nothing else: no activity of any kind.
fn one_org(org: Uuid, apps: &[(Uuid, &str)]) -> Sources {
    Sources {
        orgs: vec![org_row(org)],
        apps: apps.iter().map(|(id, n)| app_row(*id, org, n)).collect(),
        ..Sources::default()
    }
}

fn llm(org_id: Uuid, idx: i64, model: Option<&str>, tokens: [i64; 4], runs: i64) -> LlmCostRow {
    LlmCostRow {
        org_id,
        idx,
        model: model.map(str::to_string),
        input_tokens: tokens[0],
        output_tokens: tokens[1],
        cache_creation: tokens[2],
        cache_read: tokens[3],
        runs,
    }
}

/// Staff's view is in `last_viewed` (the app's last open, by anyone) and in
/// neither customer read, which skip staff in SQL: the app reads as opened,
/// and as not used by the customer, nor by any of the customer's people.
#[test]
fn a_staff_only_view_is_the_apps_last_open_and_not_the_customers_use() {
    let (org, app) = (Uuid::new_v4(), Uuid::new_v4());
    let mut src = one_org(org, &[(app, "Ops")]);
    src.last_viewed = vec![viewed(app, at(14, 2))];
    let orgs = assemble::orgs(src, &[], 2);
    let ops = &orgs[0].apps[0];
    assert_eq!(ops.last_active_at, Some(at(14, 2)));
    assert_eq!(
        ops.customer_last_active_at, None,
        "staff's open is not the customer's"
    );
    assert_eq!(ops.people7d, 0, "staff are not the customer's people");
    assert_eq!(
        orgs[0].last_activity_at, None,
        "nor the org's customer activity"
    );
}

#[test]
fn each_app_carries_its_own_customer_use() {
    let (org, ops, board) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let mut src = one_org(org, &[(ops, "Ops"), (board, "Board")]);
    // Staff opened Ops after the customer last did.
    src.last_viewed = vec![viewed(ops, at(14, 2)), viewed(board, at(9, 0))];
    src.customer_last_viewed = vec![viewed(ops, at(13, 9))];
    src.people7d = vec![AppCountRow {
        app_id: ops,
        people: 2,
    }];
    let orgs = assemble::orgs(src, &[], 2);
    let [board_app, ops_app] = &orgs[0].apps[..] else {
        panic!("two apps, by name");
    };
    assert_eq!(ops_app.last_active_at, Some(at(14, 2)));
    assert_eq!(ops_app.customer_last_active_at, Some(at(13, 9)));
    assert_eq!(ops_app.people7d, 2);
    assert_eq!(board_app.last_active_at, Some(at(9, 0)));
    assert_eq!(board_app.customer_last_active_at, None);
    assert_eq!(board_app.people7d, 0);
}

/// The org's last activity is the newer of its non-view activity and its
/// apps' customer last views: the same probe, so the two cannot disagree.
#[test]
fn last_activity_folds_in_the_customers_last_view_of_any_app() {
    let (org, ops) = (Uuid::new_v4(), Uuid::new_v4());
    let views_newest = |other: DateTime<Utc>| {
        let mut src = one_org(org, &[(ops, "Ops")]);
        src.customer_last_viewed = vec![viewed(ops, at(13, 9))];
        src.last_activity = vec![OrgAtRow {
            org_id: org,
            at: Some(other),
        }];
        assemble::orgs(src, &[], 1)[0].last_activity_at
    };
    assert_eq!(
        views_newest(at(12, 0)),
        Some(at(13, 9)),
        "the view is newer"
    );
    assert_eq!(
        views_newest(at(13, 11)),
        Some(at(13, 11)),
        "the call is newer"
    );
}

/// Each model's tokens are priced on their own and summed into the window. A
/// model the rate table lacks, or none at all, adds no dollars and counts its
/// runs as unpriced; a row outside the windows is dropped.
#[test]
fn llm_cost_prices_each_model_and_an_unpriced_one_adds_nothing() {
    let (org, quiet) = (Uuid::new_v4(), Uuid::new_v4());
    let mut src = one_org(org, &[]);
    src.orgs.push(OrgRow {
        name: "Quiet".into(),
        ..org_row(quiet)
    });
    let million = 1_000_000;
    src.llm = vec![
        // Sonnet: $3/M in, $15/M out (Admin → Metrics' own example).
        llm(
            org,
            0,
            Some("claude-sonnet-4-6"),
            [million, million, 0, 0],
            2,
        ),
        llm(
            org,
            0,
            Some("no-such-model"),
            [5 * million, million, 0, 0],
            1,
        ),
        llm(org, 0, None, [million, 0, 0, 0], 2),
        // last7d: 1M cache reads on Sonnet, $0.30.
        llm(org, 1, Some("claude-sonnet-4-6"), [0, 0, 0, million], 1),
        llm(
            org,
            5,
            Some("claude-sonnet-4-6"),
            [million, million, 0, 0],
            1,
        ),
    ];
    let orgs = assemble::orgs(src, &[], 2);
    let acme = orgs.iter().find(|o| o.id == org).unwrap();
    let week = acme.weekly[0];
    assert!(
        (week.llm_cost_usd - 18.0).abs() < 1e-9,
        "got {}",
        week.llm_cost_usd
    );
    assert_eq!(
        week.llm_unpriced_runs, 3,
        "the unknown model's run and the two with none"
    );
    assert!(
        (acme.last7d.llm_cost_usd - 0.30).abs() < 1e-9,
        "got {}",
        acme.last7d.llm_cost_usd
    );
    assert_eq!(acme.last7d.llm_unpriced_runs, 0);
    let quiet = orgs.iter().find(|o| o.id == quiet).unwrap();
    assert_eq!(
        quiet.weekly[0],
        OxyPlatformWeek::default(),
        "no runs, no cost"
    );
    assert_eq!(quiet.last7d.llm_cost_usd, 0.0);
}

/// Pricing is Admin → Metrics' own function, not a copy: the same bucket
/// prices the same there.
#[test]
fn llm_cost_is_the_admin_metrics_price() {
    let org = Uuid::new_v4();
    let mut src = one_org(org, &[]);
    let tokens = [123_456, 7_890, 1_000, 250_000];
    src.llm = vec![llm(org, 0, Some("claude-haiku-4-5"), tokens, 4)];
    let week = assemble::orgs(src, &[], 1)[0].last7d;
    let model = Some("claude-haiku-4-5".to_string());
    let metrics =
        oxy_app_core::llm_usage::price(&model, tokens[0], tokens[1], tokens[2], tokens[3]);
    assert_eq!(Some(week.llm_cost_usd), metrics);
    assert!(week.llm_cost_usd > 0.0, "a priced model");
}
