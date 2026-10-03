//! Unit tests for the LLM-usage metrics folding + pricing. No DB — these
//! exercise `build_overview` over hand-built rows, which is where the
//! per-model pricing and the day/model/org rollups actually happen.

use super::super::metrics_pricing::build_overview;
use super::super::metrics_rollup::{DayModelRow, OrgModelRow};
use uuid::Uuid;

fn day_row(day: &str, model: Option<&str>, input: i64, output: i64, runs: i64) -> DayModelRow {
    DayModelRow {
        day: day.to_string(),
        model: model.map(String::from),
        input_tokens: input,
        output_tokens: output,
        cache_creation: 0,
        cache_read: 0,
        run_count: runs,
    }
}

#[test]
fn totals_and_by_day_sum_across_models() {
    let rows = vec![
        day_row(
            "2026-06-01",
            Some("claude-sonnet-4-6"),
            1_000_000,
            1_000_000,
            2,
        ),
        day_row(
            "2026-06-01",
            Some("claude-haiku-4-5"),
            1_000_000,
            1_000_000,
            1,
        ),
        day_row("2026-06-02", Some("claude-sonnet-4-6"), 2_000_000, 0, 3),
    ];
    let out = build_overview(30, rows, vec![]);

    assert_eq!(out.window_days, 30);
    assert_eq!(out.total.input_tokens, 4_000_000);
    assert_eq!(out.total.output_tokens, 2_000_000);
    assert_eq!(out.total.run_count, 6);
    // All models are priced, so every run counts toward priced_run_count.
    assert_eq!(out.total.priced_run_count, 6);
    // sonnet: $3/M in, $15/M out → day1 (1M in + 1M out) = $3 + $15 = $18.00;
    // haiku:  $0.80/M in, $4/M out → day1 (1M in + 1M out) = $0.80 + $4 = $4.80;
    // day1 total = $22.80. (Previous expected value of 0.0228 was the same
    // arithmetic divided by 1000 by mistake — pre-existing on main before
    // this branch existed.)
    assert_eq!(out.by_day.len(), 2);
    assert_eq!(out.by_day[0].day, "2026-06-01");
    assert!((out.by_day[0].cost_usd - 22.8).abs() < 1e-9);
    assert_eq!(out.by_day[0].run_count, 3);
    assert!(out.total.cost_usd > 0.0);
}

#[test]
fn unknown_model_counts_tokens_but_not_cost() {
    let rows = vec![day_row(
        "2026-06-01",
        Some("some-future-model"),
        5_000_000,
        5_000_000,
        4,
    )];
    let out = build_overview(7, rows, vec![]);

    assert_eq!(out.total.input_tokens, 5_000_000);
    assert_eq!(out.total.run_count, 4);
    // Unknown model → no dollars, and it doesn't inflate priced_run_count.
    assert_eq!(out.total.cost_usd, 0.0);
    assert_eq!(out.total.priced_run_count, 0);
    assert_eq!(out.by_model.len(), 1);
    assert_eq!(out.by_model[0].cost_usd, None);
}

#[test]
fn by_org_is_sorted_desc_and_capped() {
    let org_rows: Vec<OrgModelRow> = (0..15)
        .map(|i| OrgModelRow {
            org_id: Uuid::from_u128(i as u128 + 1),
            org_name: format!("org{i}"),
            org_slug: format!("org{i}"),
            model: Some("claude-sonnet-4-6".to_string()),
            // ascending spend so the top spenders are the high-i orgs
            input_tokens: (i as i64 + 1) * 1_000_000,
            output_tokens: 0,
            cache_creation: 0,
            cache_read: 0,
            run_count: 1,
        })
        .collect();
    let out = build_overview(30, vec![], org_rows);

    assert_eq!(out.by_org.len(), 10, "top-10 cap");
    // Descending by cost.
    for w in out.by_org.windows(2) {
        assert!(w[0].cost_usd >= w[1].cost_usd);
    }
    // Highest spender (i=14) is first.
    assert_eq!(out.by_org[0].org_name, "org14");
}
