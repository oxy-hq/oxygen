//! Prices the token rollups from `metrics_rollup` and folds them into the response
//! shapes `metrics.rs` returns. Dollar cost is computed at read time from per-model
//! rates (`agentic_llm::pricing`), never stored.
//!
//! Split out of `metrics.rs` by responsibility.

use agentic_llm::pricing::cost_for_call;
use std::collections::BTreeMap;
use uuid::Uuid;

use super::metrics::{DayCost, LlmUsageOverview, ModelCost, OrgCost, OrgUsageDetail, UsageTotals};
use super::metrics_rollup::{DayModelRow, OrgModelRow};

/// Price a single model bucket. `None` model or an unknown model yields
/// `None` cost (tokens still count toward usage, just not dollars).
fn price(model: &Option<String>, input: i64, output: i64, cc: i64, cr: i64) -> Option<f64> {
    let model = model.as_deref()?;
    cost_for_call(
        model,
        input.max(0) as u64,
        output.max(0) as u64,
        cc.max(0) as u64,
        cr.max(0) as u64,
    )
}

/// Fold per-(day, model) rows into the window total + the day series. Shared by
/// the cross-tenant overview and the per-org detail so both price identically.
/// Day order is preserved from the SQL `ORDER BY day`.
fn fold_days(day_rows: &[DayModelRow]) -> (UsageTotals, Vec<DayCost>) {
    let mut total = UsageTotals::default();
    let mut by_day: Vec<DayCost> = Vec::new();
    let mut day_index: BTreeMap<String, usize> = BTreeMap::new();

    for r in day_rows {
        let cost = price(
            &r.model,
            r.input_tokens,
            r.output_tokens,
            r.cache_creation,
            r.cache_read,
        );

        total.input_tokens += r.input_tokens;
        total.output_tokens += r.output_tokens;
        total.cache_creation_tokens += r.cache_creation;
        total.cache_read_tokens += r.cache_read;
        total.run_count += r.run_count;
        if let Some(c) = cost {
            total.cost_usd += c;
            total.priced_run_count += r.run_count;
        }

        let idx = *day_index.entry(r.day.clone()).or_insert_with(|| {
            by_day.push(DayCost {
                day: r.day.clone(),
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                run_count: 0,
            });
            by_day.len() - 1
        });
        let d = &mut by_day[idx];
        d.cost_usd += cost.unwrap_or(0.0);
        d.input_tokens += r.input_tokens;
        d.output_tokens += r.output_tokens;
        d.run_count += r.run_count;
    }

    (total, by_day)
}

pub(super) fn build_org_detail(days: i32, day_rows: Vec<DayModelRow>) -> OrgUsageDetail {
    let (total, by_day) = fold_days(&day_rows);
    OrgUsageDetail {
        window_days: days,
        total,
        by_day,
    }
}

pub(super) fn build_overview(
    days: i32,
    day_rows: Vec<DayModelRow>,
    org_rows: Vec<OrgModelRow>,
) -> LlmUsageOverview {
    let (total, by_day) = fold_days(&day_rows);

    // by_model — fold the same day rows by model and price each bucket.
    let mut by_model: BTreeMap<String, ModelCost> = BTreeMap::new();
    for r in &day_rows {
        let cost = price(
            &r.model,
            r.input_tokens,
            r.output_tokens,
            r.cache_creation,
            r.cache_read,
        );
        let key = r.model.clone().unwrap_or_else(|| "unknown".to_string());
        let m = by_model.entry(key.clone()).or_insert_with(|| ModelCost {
            model: key,
            cost_usd: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            run_count: 0,
        });
        m.input_tokens += r.input_tokens;
        m.output_tokens += r.output_tokens;
        m.cache_creation_tokens += r.cache_creation;
        m.cache_read_tokens += r.cache_read;
        m.run_count += r.run_count;
        if let Some(c) = cost {
            m.cost_usd = Some(m.cost_usd.unwrap_or(0.0) + c);
        }
    }

    // by_org — fold model rows per org, price each, keep the top 10 by cost.
    let mut org_map: BTreeMap<Uuid, OrgCost> = BTreeMap::new();
    for r in &org_rows {
        let cost = price(
            &r.model,
            r.input_tokens,
            r.output_tokens,
            r.cache_creation,
            r.cache_read,
        )
        .unwrap_or(0.0);
        let entry = org_map.entry(r.org_id).or_insert_with(|| OrgCost {
            org_id: r.org_id,
            org_name: r.org_name.clone(),
            org_slug: r.org_slug.clone(),
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            run_count: 0,
        });
        entry.cost_usd += cost;
        entry.input_tokens += r.input_tokens;
        entry.output_tokens += r.output_tokens;
        entry.run_count += r.run_count;
    }
    let mut by_org: Vec<OrgCost> = org_map.into_values().collect();
    by_org.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    by_org.truncate(10);

    let mut by_model: Vec<ModelCost> = by_model.into_values().collect();
    by_model.sort_by(|a, b| {
        b.cost_usd
            .unwrap_or(0.0)
            .partial_cmp(&a.cost_usd.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    LlmUsageOverview {
        window_days: days,
        total,
        by_day,
        by_model,
        by_org,
    }
}
