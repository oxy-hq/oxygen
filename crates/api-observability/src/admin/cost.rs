//! `GET /admin/observability/orgs/{org_id}/cost` — one org's LLM spend, with
//! the dimensions that say why it is what it is.
//!
//! The SQL is [`super::cost_rollup`]; the derived numbers are
//! [`super::derive`]; the rates are `oxy_app_core::llm_usage::price`, shared
//! with `/admin/metrics` and `ctx.platform.facts()` so three surfaces cannot
//! price one run three ways.
//!
//! **What this does not report.** Storage, egress, function compute and OLTP
//! are deliberately absent as *dollars*: nearly all of the platform's ~$4.4k/mo
//! is shared across every org (one fleet, one control-plane Postgres, one
//! Airhouse), and splitting a shared bill by egress bytes invents precision on
//! a line that is 3–6% of revenue. `customer-fleet-scaling.md` §6 did that
//! arithmetic and concluded "LLM and FDE cost per account are the actual
//! variables, and only LLM is instrumented". The app meters that *are*
//! per-org appear on the Performance panel as quantities, not dollars.

use axum::extract::{Path, Query};
use axum::response::Response;
use axum::{Json, Router, routing::get};
use oxy_app_core::AppState;
use oxy_app_core::admin_db::db_err;
use oxy_app_core::llm_usage::price;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::FromQueryResult;
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;

use super::cost_rollup::{self, BucketModelRow, DayModelRow};
use super::{WindowQuery, connect_scoped, cost_to_serve};

/// Window totals. Tokens are always real; `cost_usd` covers only the runs
/// whose model has a rate, and `unpriced_runs` names the rest rather than
/// letting the dollar total quietly stand in for all of it.
#[derive(Debug, Default, Serialize)]
pub struct CostTotals {
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub run_count: i64,
    /// Runs on a model with no known rate: counted in tokens, excluded from
    /// dollars, and stated on the page.
    pub unpriced_runs: i64,
}

/// One day of the window.
#[derive(Debug, Serialize)]
pub struct CostDay {
    pub day: String,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub run_count: i64,
}

/// One value of a dimension — a model, a workspace, a surface, an attendance
/// class. `share` is this bucket's fraction of the window's priced cost, or
/// `None` when nothing was priced.
#[derive(Debug, Serialize)]
pub struct CostBucket {
    /// The dimension's raw value — a model id, a workspace id, `workflow`,
    /// `scheduled`. Stable; the frontend keys off it.
    pub key: String,
    /// What to show. Equal to `key` except for a workspace, where it is the
    /// workspace's name.
    pub label: String,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub run_count: i64,
    pub share: Option<f64>,
}

/// The numbers that make a total actionable. Every one is `None` on an empty
/// window rather than zero — see [`super::derive`].
#[derive(Debug, Default, Serialize)]
pub struct CostDerived {
    /// This window's spend scaled to 30 days.
    pub monthly_run_rate_usd: Option<f64>,
    pub cost_per_run_usd: Option<f64>,
    /// Fraction of input tokens served from cache — the main cost lever.
    pub cache_hit_share: Option<f64>,
    /// Fraction of priced cost from runs a schedule seeded. The share that
    /// grows without anybody deciding to spend it.
    pub unattended_share: Option<f64>,
}

/// The org this is about, so the console can name itself and build links
/// without the cross-tenant fleet rollup. One indexed lookup after the fence,
/// against a scan over every org's runs.
#[derive(Debug, Serialize)]
pub struct OrgIdentity {
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Serialize)]
pub struct OrgCost {
    pub window_days: u32,
    pub org: OrgIdentity,
    pub total: CostTotals,
    pub derived: CostDerived,
    pub by_day: Vec<CostDay>,
    /// The `days` before the window, day by day, so a jump is dated against
    /// a baseline rather than against the chart's own peak. `None` when the
    /// prior window could not be read, or is past run retention — the chart
    /// then draws no comparison.
    pub prior_by_day: Option<Vec<CostDay>>,
    /// The prior window's priced spend; `None` alongside `prior_by_day`.
    pub prior_cost_usd: Option<f64>,
    pub by_model: Vec<CostBucket>,
    pub by_workspace: Vec<CostBucket>,
    pub by_surface: Vec<CostBucket>,
    pub by_attendance: Vec<CostBucket>,
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/observability/orgs/{org_id}/cost", get(org_cost))
}

/// One org's spend over the window.
///
/// The staff-scope fence runs first ([`connect_scoped`]): `PlatformOperate` is
/// held by every Global Admin regardless of bound, so unfenced this would read
/// another tenant's spend. An out-of-scope org answers **not found**, which is
/// what a scope boundary looks like everywhere in this console.
async fn org_cost(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Query(q): Query<WindowQuery>,
) -> Result<Json<OrgCost>, Response> {
    let days = q.days();
    let db = connect_scoped(&actor, org_id).await?;
    let org = load_identity(&db, org_id).await.map_err(db_err)?;

    let day_rows = cost_rollup::fetch_days(&db, days, org_id)
        .await
        .map_err(db_err)?;
    let (total, by_day) = fold_days(&day_rows);

    let prior = read_prior(&db, days, org_id).await;
    let prior_cost_usd = prior.as_ref().map(|(t, _)| t.cost_usd);
    let prior_by_day = prior.map(|(_, d)| d);

    let by_model = fold_buckets(
        &cost_rollup::fetch_by_model(&db, days, org_id)
            .await
            .map_err(db_err)?,
        total.cost_usd,
    );
    let by_workspace = fold_buckets(
        &cost_rollup::fetch_by_workspace(&db, days, org_id)
            .await
            .map_err(db_err)?,
        total.cost_usd,
    );
    let by_surface = fold_buckets(
        &cost_rollup::fetch_by_surface(&db, days, org_id)
            .await
            .map_err(db_err)?,
        total.cost_usd,
    );
    let by_attendance = fold_buckets(
        &cost_rollup::fetch_by_attendance(&db, days, org_id)
            .await
            .map_err(db_err)?,
        total.cost_usd,
    );

    let derived = derive_from(&total, days, &by_attendance);

    Ok(Json(OrgCost {
        window_days: days as u32,
        org,
        total,
        derived,
        by_day,
        prior_by_day,
        prior_cost_usd,
        by_model,
        by_workspace,
        by_surface,
        by_attendance,
    }))
}

/// The `days` before the window, folded — or `None`, and the page draws no
/// comparison.
///
/// A comparison, not a figure the page stands on: if it cannot be read it is
/// dropped and logged, and every other number still renders. Past run retention
/// it is not read at all — those runs are deleted, so the window would read as
/// empty and the spend as new.
async fn read_prior(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    org_id: Uuid,
) -> Option<(CostTotals, Vec<CostDay>)> {
    let horizon = cost_to_serve::retention_horizon_days(false);
    if !cost_to_serve::prior_retained(days.max(0) as u32, horizon) {
        return None;
    }
    match cost_rollup::fetch_prior_days(db, days, org_id).await {
        Ok(rows) => Some(fold_days(&rows)),
        Err(e) => {
            tracing::warn!(error = %e, %org_id, "prior-window daily cost read failed");
            None
        }
    }
}

/// The org's name and slug.
///
/// A direct lookup, not a row plucked out of the fleet rollup: that rollup
/// joins `agentic_runs` × `agentic_run_events` for every tenant over the
/// window, and a page about one org has no business running it.
///
/// A missing row keeps the id as the name — the fence already proved the
/// caller may see this org, so an absent `organizations` row is a race with a
/// deletion, not an authorization question.
async fn load_identity(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
) -> Result<OrgIdentity, sea_orm::DbErr> {
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct Row {
        name: String,
        slug: String,
    }
    let row = Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT name, slug FROM organizations WHERE id = $1",
        [org_id.into()],
    ))
    .one(db)
    .await?;
    Ok(match row {
        Some(r) => OrgIdentity {
            org_id,
            name: r.name,
            slug: r.slug,
        },
        None => OrgIdentity {
            org_id,
            name: org_id.to_string(),
            slug: String::new(),
        },
    })
}

/// Fold per-(day, model) rows into the window total and the day series.
///
/// Each (day, model) bucket is priced at **its own** rate before anything is
/// summed. Summing tokens across models first and pricing the sum would charge
/// every token at one model's rate.
///
/// Day order comes from the SQL's `ORDER BY day`, and is preserved: a
/// `BTreeMap` index keeps the first-seen position rather than re-sorting
/// lexically, so a series stays in the order the query produced.
fn fold_days(rows: &[DayModelRow]) -> (CostTotals, Vec<CostDay>) {
    let mut total = CostTotals::default();
    let mut by_day: Vec<CostDay> = Vec::new();
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();

    for r in rows {
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
        match cost {
            Some(c) => total.cost_usd += c,
            // Tokens counted, dollars not knowable. Named, not dropped.
            None => total.unpriced_runs += r.run_count,
        }

        let at = *index.entry(r.day.as_str()).or_insert_with(|| {
            by_day.push(CostDay {
                day: r.day.clone(),
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                run_count: 0,
            });
            by_day.len() - 1
        });
        let day = &mut by_day[at];
        day.cost_usd += cost.unwrap_or(0.0);
        day.input_tokens += r.input_tokens;
        day.output_tokens += r.output_tokens;
        day.run_count += r.run_count;
    }

    (total, by_day)
}

/// Fold per-(bucket, model) rows into one row per bucket, priced per model,
/// ordered by cost descending — which is the order the question is asked in.
fn fold_buckets(rows: &[BucketModelRow], window_cost: f64) -> Vec<CostBucket> {
    let mut by_key: BTreeMap<&str, CostBucket> = BTreeMap::new();

    for r in rows {
        let cost = price(
            &r.model,
            r.input_tokens,
            r.output_tokens,
            r.cache_creation,
            r.cache_read,
        )
        .unwrap_or(0.0);
        let bucket = by_key
            .entry(r.bucket.as_str())
            .or_insert_with(|| CostBucket {
                key: r.bucket.clone(),
                label: r.label.clone(),
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                run_count: 0,
                share: None,
            });
        bucket.cost_usd += cost;
        bucket.input_tokens += r.input_tokens;
        bucket.output_tokens += r.output_tokens;
        bucket.run_count += r.run_count;
    }

    let mut out: Vec<CostBucket> = by_key.into_values().collect();
    for b in &mut out {
        b.share = super::derive::share(b.cost_usd, window_cost);
    }
    // Cost descending, then key, so equal-cost buckets have a stable order
    // rather than one that changes between requests.
    out.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
    out
}

/// The derived numbers, from the totals and the attendance split.
///
/// `unattended_share` is read off the attendance buckets rather than
/// recomputed, so the headline share and the bar beneath it cannot disagree.
fn derive_from(total: &CostTotals, days: i32, attendance: &[CostBucket]) -> CostDerived {
    let scheduled = attendance
        .iter()
        .find(|b| b.key == "scheduled")
        .map(|b| b.cost_usd)
        .unwrap_or(0.0);
    // Priced dollars over PRICED runs. `cost_usd` excludes runs on a model
    // with no rate, so dividing by every run reports an org with unpriced
    // models as cheaper per run than it is — the silent folding-in this
    // surface exists to avoid, and the column hint says "priced cost ÷ runs".
    let priced_runs = (total.run_count - total.unpriced_runs).max(0) as u64;
    CostDerived {
        monthly_run_rate_usd: super::derive::monthly_run_rate(total.cost_usd, days.max(0) as u32),
        cost_per_run_usd: super::derive::per_unit(total.cost_usd, priced_runs),
        cache_hit_share: super::derive::cache_hit_share(
            total.input_tokens,
            total.cache_creation_tokens,
            total.cache_read_tokens,
        ),
        unattended_share: super::derive::share(scheduled, total.cost_usd),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(day: &str, model: Option<&str>, input: i64, output: i64, runs: i64) -> DayModelRow {
        DayModelRow {
            day: day.into(),
            model: model.map(Into::into),
            input_tokens: input,
            output_tokens: output,
            cache_creation: 0,
            cache_read: 0,
            run_count: runs,
        }
    }

    fn bucket(
        key: &str,
        label: &str,
        model: Option<&str>,
        input: i64,
        runs: i64,
    ) -> BucketModelRow {
        BucketModelRow {
            bucket: key.into(),
            label: label.into(),
            model: model.map(Into::into),
            input_tokens: input,
            output_tokens: 0,
            cache_creation: 0,
            cache_read: 0,
            run_count: runs,
        }
    }

    /// A run on a model with no rate keeps its tokens in the total and is
    /// counted as unpriced. Dropping it would make the token total agree with
    /// the dollar total and hide exactly the usage nobody can price — the one
    /// thing a cost page must not do silently.
    #[test]
    fn unpriced_runs_keep_their_tokens_and_are_counted() {
        let rows = [day("2026-11-01", Some("not-a-real-model"), 1_000, 500, 3)];
        let (total, by_day) = fold_days(&rows);
        assert_eq!(total.input_tokens, 1_000, "tokens are real");
        assert_eq!(total.run_count, 3);
        assert_eq!(total.unpriced_runs, 3, "the unpriced runs are named");
        assert_eq!(total.cost_usd, 0.0, "and contribute no dollars");
        assert_eq!(by_day[0].run_count, 3, "the day still counts the runs");
    }

    /// A model with no id at all (`llm_end` carried none) is unpriced too, not
    /// a panic and not a zero-token row.
    #[test]
    fn a_run_with_no_model_at_all_is_unpriced() {
        let (total, _) = fold_days(&[day("2026-11-01", None, 10, 10, 1)]);
        assert_eq!(total.unpriced_runs, 1);
        assert_eq!(total.input_tokens, 10);
    }

    /// Two models on one day collapse into one day row, and the day's cost is
    /// the sum of each priced at its own rate. One day row per model would
    /// draw the trend with twice the points and half the height each.
    #[test]
    fn a_day_with_two_models_is_one_row() {
        let rows = [
            day("2026-11-01", Some("claude-opus-5"), 1_000, 100, 1),
            day("2026-11-01", Some("claude-haiku-5-5"), 2_000, 200, 2),
        ];
        let (total, by_day) = fold_days(&rows);
        assert_eq!(by_day.len(), 1, "one row per day: {by_day:?}");
        assert_eq!(by_day[0].run_count, 3);
        assert_eq!(total.input_tokens, 3_000);
        // Both are priced models, so nothing is unpriced and the day's cost is
        // the sum of two different rates — not one rate over 3,000 tokens.
        assert_eq!(total.unpriced_runs, 0);
        assert!(by_day[0].cost_usd > 0.0);
    }

    /// The day series keeps the order the query produced. Re-sorting would be
    /// harmless for `YYYY-MM-DD` and wrong the moment the label format changes;
    /// the SQL already orders it, so the fold must not reorder.
    #[test]
    fn the_day_series_preserves_query_order() {
        let rows = [
            day("2026-11-01", Some("claude-opus-5"), 1, 1, 1),
            day("2026-11-02", Some("claude-opus-5"), 1, 1, 1),
            day("2026-11-03", Some("claude-opus-5"), 1, 1, 1),
        ];
        let (_, by_day) = fold_days(&rows);
        let days: Vec<&str> = by_day.iter().map(|d| d.day.as_str()).collect();
        assert_eq!(days, ["2026-11-01", "2026-11-02", "2026-11-03"]);
    }

    /// Buckets come back cost-descending, because "where did the money go" is
    /// asked in that order.
    #[test]
    fn buckets_are_ordered_by_cost_descending() {
        let rows = [
            bucket("small", "small", Some("claude-haiku-5-5"), 1_000, 1),
            bucket("large", "large", Some("claude-opus-5"), 1_000_000, 1),
        ];
        let out = fold_buckets(&rows, 0.0);
        assert_eq!(out[0].key, "large", "the expensive bucket leads: {out:?}");
    }

    /// Equal-cost buckets are ordered by key, so two requests do not return
    /// the same data in two orders — a table that reshuffles on refresh reads
    /// as live data when nothing changed.
    #[test]
    fn equal_cost_buckets_have_a_stable_order() {
        let rows = [
            bucket("b", "b", Some("nope"), 10, 1),
            bucket("a", "a", Some("nope"), 10, 1),
            bucket("c", "c", Some("nope"), 10, 1),
        ];
        let keys: Vec<String> = fold_buckets(&rows, 0.0)
            .into_iter()
            .map(|b| b.key)
            .collect();
        assert_eq!(keys, ["a", "b", "c"]);
    }

    /// A bucket's share is of the window's priced cost, and is `None` when
    /// nothing was priced — not 0%, which would draw every bar empty and read
    /// as "no spend here" rather than "no rate for any of it".
    #[test]
    fn bucket_shares_are_absent_when_nothing_was_priced() {
        let rows = [bucket("x", "x", Some("not-a-real-model"), 10, 1)];
        let out = fold_buckets(&rows, 0.0);
        assert_eq!(out[0].share, None);
    }

    /// The headline unattended share is read off the attendance buckets, so
    /// the number and the bar beneath it come from one computation. Two
    /// computations is how a page ends up disagreeing with itself.
    #[test]
    fn the_unattended_share_comes_from_the_attendance_buckets() {
        let total = CostTotals {
            cost_usd: 100.0,
            run_count: 10,
            input_tokens: 1_000,
            cache_read_tokens: 1_000,
            ..Default::default()
        };
        let attendance = vec![
            CostBucket {
                key: "scheduled".into(),
                label: "scheduled".into(),
                cost_usd: 75.0,
                input_tokens: 0,
                output_tokens: 0,
                run_count: 7,
                share: Some(0.75),
            },
            CostBucket {
                key: "interactive".into(),
                label: "interactive".into(),
                cost_usd: 25.0,
                input_tokens: 0,
                output_tokens: 0,
                run_count: 3,
                share: Some(0.25),
            },
        ];
        let d = derive_from(&total, 30, &attendance);
        assert_eq!(d.unattended_share, Some(0.75));
        assert_eq!(d.cost_per_run_usd, Some(10.0));
        assert_eq!(d.monthly_run_rate_usd, Some(100.0));
        assert_eq!(d.cache_hit_share, Some(0.5));
    }

    /// `$/run` divides priced dollars by **priced** runs. Counting the
    /// unpriced ones in the denominator makes an org on an unknown model look
    /// cheaper per run than it is, which is the comparison the fleet list is
    /// read for.
    #[test]
    fn cost_per_run_divides_by_priced_runs_only() {
        let total = CostTotals {
            cost_usd: 100.0,
            run_count: 200,
            unpriced_runs: 100,
            ..Default::default()
        };
        let d = derive_from(&total, 30, &[]);
        assert_eq!(
            d.cost_per_run_usd,
            Some(1.0),
            "$100 over the 100 priced runs"
        );
    }

    /// Every run unpriced means no per-run cost at all, not a division by
    /// zero dressed as a figure.
    #[test]
    fn all_runs_unpriced_yields_no_cost_per_run() {
        let total = CostTotals {
            run_count: 50,
            unpriced_runs: 50,
            ..Default::default()
        };
        assert_eq!(derive_from(&total, 30, &[]).cost_per_run_usd, None);
    }

    /// An org with no scheduled runs reports a 0% unattended share, not a
    /// missing one: there *was* spend, and none of it was unattended. That is
    /// a fact, unlike the empty-window case.
    #[test]
    fn no_scheduled_runs_is_a_zero_share_not_an_absent_one() {
        let total = CostTotals {
            cost_usd: 40.0,
            run_count: 4,
            ..Default::default()
        };
        let d = derive_from(&total, 7, &[]);
        assert_eq!(d.unattended_share, Some(0.0));
    }

    /// An org with no spend at all reports no shares and no per-unit cost.
    #[test]
    fn an_empty_window_derives_nothing() {
        let d = derive_from(&CostTotals::default(), 30, &[]);
        assert_eq!(d.unattended_share, None);
        assert_eq!(d.cost_per_run_usd, None);
        assert_eq!(d.cache_hit_share, None);
        assert_eq!(d.monthly_run_rate_usd, Some(0.0), "a real zero run-rate");
    }
}
