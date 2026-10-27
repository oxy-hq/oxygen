//! What a check reads from the warehouse, behind two seams so tests run with
//! no warehouse: per-unit daily values of a measure, and an anomaly scan over
//! a set of units. The production impls use the workspace's system-mode
//! `MetricTreeRunner`, one `run_time_series` per unit: PR 3's `PanelMatrix`
//! keeps its values private to the engine, so it cannot answer "what was
//! `promo_active` at bondi on 3 March". Which member is the unit and which the
//! day is PR 3's `resolve_spec` — one rule for pricing and read-back.

use std::collections::BTreeMap;
use std::sync::Arc;

use agentic_analytics::MetricTreeRunner;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::query::{FilterOperator, QueryFilter};
use oxy_experiment::Moment;
use oxy_experiment::compliance::Sampling;
use oxy_metric_monitoring::MonitorEntry;
use oxy_metric_monitoring::service::scan_one;

use sea_orm::DatabaseConnection;

use crate::server::experiment::panel::{
    GroupMembership, HistoryBounds, PanelContext, PanelRequest, resolve_groups, resolve_spec,
};

#[async_trait]
pub trait UnitDays: Send + Sync {
    /// `measure` at `unit` per local day in `[from, to]`; only the days returned.
    async fn values(
        &self,
        measure: &str,
        unit: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<BTreeMap<NaiveDate, f64>, String>;
    /// `measure` aggregated over exactly `units` per local day — one unit, or
    /// a group's pinned members (Task 11 reads a group's guardrail this way).
    async fn values_over(
        &self,
        measure: &str,
        units: &[String],
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<BTreeMap<NaiveDate, f64>, String> {
        match units {
            [one] => self.values(measure, one, from, to).await,
            many => Err(format!(
                "this reader cannot read `{measure}` over {} units at once",
                many.len()
            )),
        }
    }
    /// `measure` over `units` per reading — a local day, or a local hour on an
    /// hour-grain design — on the dates `[from, to]`; only the readings returned.
    /// A day is keyed by its midnight. A reader that cannot read hours refuses.
    async fn readings(
        &self,
        measure: &str,
        units: &[String],
        from: NaiveDate,
        to: NaiveDate,
        sampling: Sampling,
    ) -> Result<BTreeMap<Moment, f64>, String> {
        match sampling {
            Sampling::Daily => Ok(self
                .values_over(measure, units, from, to)
                .await?
                .into_iter()
                .map(|(d, v)| (Moment::midnight(d), v))
                .collect()),
            Sampling::Hourly(_) => Err(format!("this reader cannot read `{measure}` per hour")),
        }
    }
    /// Units with `measure` rows in the last `since_days` days.
    async fn units(&self, measure: &str, since_days: u32) -> Result<Vec<String>, String>;
}

#[async_trait]
pub trait Membership: Send + Sync {
    /// Every value of `dimension` on `entity`, with its member units.
    async fn groups(&self, entity: &str, dimension: &str) -> Result<GroupMembership, String>;
}

/// PR 3's `resolve_groups`: the membership pricing randomised over.
pub struct PanelGroups {
    pub db: DatabaseConnection,
    pub ctx: PanelContext,
}

#[async_trait]
impl Membership for PanelGroups {
    async fn groups(&self, entity: &str, dimension: &str) -> Result<GroupMembership, String> {
        resolve_groups(&self.db, &self.ctx, entity, dimension)
            .await
            .map_err(|e| format!("resolving the groups of `{dimension}` on `{entity}`: {e}"))
    }
}

/// One treated unit's anomaly scan: `measure` over exactly `members` (the
/// unit itself, or a group's pinned members).
pub struct AnomalyQuery<'a> {
    pub measure: &'a str,
    pub members: &'a [String],
    pub significance: f64,
    pub since: NaiveDate,
    pub now: DateTime<Utc>,
}

#[async_trait]
pub trait AnomalyScan: Send + Sync {
    /// Every day on or after `since` the detector flags, with its detail.
    /// Which of them count — the unit's treated days — is the caller's call.
    async fn flagged(&self, q: &AnomalyQuery<'_>) -> Result<Vec<(NaiveDate, String)>, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitAxis {
    pub unit_member: String,
    pub time_member: String,
}

/// What the pinned design says about the unit and the day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Axis {
    pub for_each: String,
    pub time_dimension: Option<String>,
}

impl Axis {
    pub fn of(c: &oxy_experiment::ExperimentConfig) -> Self {
        Axis {
            for_each: c.for_each.clone(),
            time_dimension: c.design.time_dimension.clone(),
        }
    }
}

/// The unit and day members for reading `measure` per `for_each` unit-day —
/// PR 3's `resolve_spec`, so pricing and read-back share one rule.
pub fn unit_axis(layer: &SemanticLayer, axis: &Axis, measure: &str) -> Result<UnitAxis, String> {
    let for_each = &axis.for_each;
    let request = PanelRequest {
        entity: for_each.clone(),
        measure: measure.to_string(),
        time_dimension: axis.time_dimension.clone(),
        // Read-back and guardrails read daily values whatever the design's grain.
        grain: oxy_experiment::Grain::Day,
        timezone: None,
        // Resolution reads no history; the bounds only shape the panel query.
        history: HistoryBounds {
            from: None,
            to_exclusive: NaiveDate::MAX,
        },
        // Nor membership or a fleet restriction: which member is the unit does not depend on groups or the fleet.
        membership: None,
        restrict_to: None,
        unit_floor: None,
        filters: vec![],
    };
    let spec = resolve_spec(layer, &request)
        .map_err(|refusal| format!("resolving `{measure}` per `{for_each}` unit-day: {refusal}"))?;
    Ok(UnitAxis {
        unit_member: spec.key_dimension,
        time_member: spec.time_dimension,
    })
}

/// The listed units, named as the panel names them (`panel_rows::render_key`).
/// The listing has already turned each value into text, so a warehouse that types
/// an integer key as a double lists `12.0`; the panel, and every design, says `12`.
pub fn unit_names(listed: Vec<String>) -> Vec<String> {
    let mut names: Vec<String> = listed
        .into_iter()
        .map(|s| match s.strip_suffix(".0") {
            Some(int) if int.parse::<i64>().is_ok() => int.to_string(),
            _ => s,
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

pub struct RunnerUnitDays {
    pub runner: Arc<dyn MetricTreeRunner>,
    pub layer: SemanticLayer,
    pub axis: Axis,
    pub tz: Tz,
}

#[async_trait]
impl UnitDays for RunnerUnitDays {
    async fn values(
        &self,
        measure: &str,
        unit: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<BTreeMap<NaiveDate, f64>, String> {
        self.values_over(measure, &[unit.to_string()], from, to)
            .await
    }

    /// One query: an `Equals` filter's values are OR'd, so several units read
    /// as their aggregate — a group's day under `assign_by`.
    async fn values_over(
        &self,
        measure: &str,
        units: &[String],
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<BTreeMap<NaiveDate, f64>, String> {
        let axis = unit_axis(&self.layer, &self.axis, measure)?;
        let filter = QueryFilter {
            member: Some(axis.unit_member),
            operator: Some(FilterOperator::Equals),
            values: units.to_vec(),
            and: None,
            or: None,
        };
        let end = to.succ_opt().unwrap_or(to);
        let rows = self
            .runner
            .run_time_series(
                measure.to_string(),
                axis.time_member,
                "day".into(),
                (from.to_string(), end.to_string()),
                vec![filter],
                Some(self.tz.name().to_string()),
            )
            .await
            .map_err(|e| format!("reading `{measure}` at `{}`: {e:?}", units.join("`, `")))?;
        Ok(rows
            .into_iter()
            .filter_map(|(ts, v)| {
                Some((
                    NaiveDate::parse_from_str(ts.get(..10)?, "%Y-%m-%d").ok()?,
                    v,
                ))
            })
            .filter(|(day, _)| *day >= from && *day <= to)
            .collect())
    }

    async fn readings(
        &self,
        measure: &str,
        units: &[String],
        from: NaiveDate,
        to: NaiveDate,
        sampling: Sampling,
    ) -> Result<BTreeMap<Moment, f64>, String> {
        if sampling == Sampling::Daily {
            let days = self.values_over(measure, units, from, to).await?;
            return Ok(days
                .into_iter()
                .map(|(d, v)| (Moment::midnight(d), v))
                .collect());
        }
        let axis = unit_axis(&self.layer, &self.axis, measure)?;
        let filter = QueryFilter {
            member: Some(axis.unit_member),
            operator: Some(FilterOperator::Equals),
            values: units.to_vec(),
            and: None,
            or: None,
        };
        let end = to.succ_opt().unwrap_or(to);
        let rows = self
            .runner
            .run_time_series(
                measure.to_string(),
                axis.time_member,
                "hour".into(),
                (from.to_string(), end.to_string()),
                vec![filter],
                Some(self.tz.name().to_string()),
            )
            .await
            .map_err(|e| {
                format!(
                    "reading `{measure}` at `{}` per hour: {e:?}",
                    units.join("`, `")
                )
            })?;
        Ok(rows
            .into_iter()
            .filter_map(|(ts, v)| Some((hour_of(&ts)?, v)))
            .filter(|(m, _)| m.date() >= from && m.date() <= to)
            .collect())
    }

    async fn units(&self, measure: &str, since_days: u32) -> Result<Vec<String>, String> {
        let axis = unit_axis(&self.layer, &self.axis, measure)?;
        self.runner
            .get_dimension_values(axis.unit_member, measure.to_string(), since_days)
            .await
            .map(unit_names)
            .map_err(|e| format!("listing the units of `{measure}`: {e:?}"))
    }
}

/// A `MonitorEntry` scanning `q.measure` over exactly `q.members` — what
/// `group_by` expands one value into (`service.rs:172-200`), built through the
/// entry's own deserializer (it is `deny_unknown_fields`).
pub fn monitor_entry(
    axis: &UnitAxis,
    q: &AnomalyQuery<'_>,
    tz: Tz,
) -> Result<MonitorEntry, String> {
    serde_json::from_value(serde_json::json!({
        "measure": q.measure,
        "time_dimension": axis.time_member,
        "granularity": "day",
        "sensitivity": super::guardrails::sensitivity_for(q.significance)?,
        "filters": [{ "member": axis.unit_member, "values": q.members }],
        "timezone": tz.name(),
    }))
    .map_err(|e| format!("building the anomaly scan for `{}`: {e}", q.measure))
}

pub struct MonitorAnomalyScan {
    pub runner: Arc<dyn MetricTreeRunner>,
    pub layer: SemanticLayer,
    pub axis: Axis,
    pub tz: Tz,
}

#[async_trait]
impl AnomalyScan for MonitorAnomalyScan {
    async fn flagged(&self, q: &AnomalyQuery<'_>) -> Result<Vec<(NaiveDate, String)>, String> {
        let axis = unit_axis(&self.layer, &self.axis, q.measure)?;
        let entry = monitor_entry(&axis, q, self.tz)?;
        let scan = scan_one(self.runner.clone(), &entry, q.now, None)
            .await
            .map_err(|e| e.to_string())?;
        if scan.coverage.is_warming_up() {
            return Err(format!(
                "`{}` at `{}` has {} of the {} days of history the detector needs",
                q.measure,
                q.members.join("`, `"),
                scan.coverage.measured,
                scan.coverage.required
            ));
        }
        Ok(scan
            .anomalies
            .iter()
            .map(|a| (a.timestamp.with_timezone(&self.tz).date_naive(), a))
            .filter(|(day, _)| *day >= q.since)
            .map(|(day, a)| {
                let detail = format!(
                    "observed {:.4}, expected {:.4} (band {:.4} to {:.4})",
                    a.observed, a.expected, a.lower, a.upper
                );
                (day, detail)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = r#"
name: store_days
datasource: local
table: store_days
entities:
  - name: store
    type: primary
    key: store
dimensions:
  - name: store
    type: string
    expr: store
  - name: day
    type: date
    expr: day
measures:
  - name: promo_active
    type: sum
    expr: promo_active
"#;

    fn axis(for_each: &str) -> Axis {
        Axis {
            for_each: for_each.into(),
            time_dimension: None,
        }
    }

    fn layer(yaml: &str) -> SemanticLayer {
        let view = oxy_airlayer_compat::parse_view_yaml(yaml).expect("view parses");
        SemanticLayer::new(vec![view], None)
    }

    #[test]
    fn a_listed_integer_key_is_named_as_the_panel_names_it() {
        // DuckDB types an integer key as a double; the listing renders `1.0`, the
        // panel `1`. A pool pinned as `1.0` names units the estimate never reads.
        let listed = ["1.0", "10.0", "22", "bondi", "2.5", "-3.0"]
            .map(String::from)
            .to_vec();
        assert_eq!(unit_names(listed), ["-3", "1", "10", "2.5", "22", "bondi"]);
    }

    #[test]
    fn the_unit_is_the_entity_key_and_the_day_is_the_views_date_dimension() {
        assert_eq!(
            unit_axis(&layer(VIEW), &axis("store"), "store_days.promo_active").unwrap(),
            UnitAxis {
                unit_member: "store_days.store".into(),
                time_member: "store_days.day".into(),
            }
        );
    }

    #[test]
    fn refusals_are_pr_3s_and_name_the_measure_being_read() {
        let err =
            unit_axis(&layer(VIEW), &axis("location"), "store_days.promo_active").unwrap_err();
        assert!(
            err.starts_with("resolving `store_days.promo_active` per `location`"),
            "{err}"
        );
        let two_dates = VIEW.replace(
            "measures:",
            "  - name: opened_on\n    type: date\n    expr: opened_on\nmeasures:",
        );
        let err = unit_axis(
            &layer(&two_dates),
            &axis("store"),
            "store_days.promo_active",
        )
        .unwrap_err();
        assert!(
            err.contains("opened_on"),
            "the refusal names the candidates: {err}"
        );
        let named = Axis {
            for_each: "store".into(),
            time_dimension: Some("store_days.day".into()),
        };
        assert_eq!(
            unit_axis(&layer(&two_dates), &named, "store_days.promo_active")
                .unwrap()
                .time_member,
            "store_days.day",
            "design.time_dimension settles it (C2a)"
        );
    }

    #[test]
    fn the_anomaly_scan_is_one_unit_as_its_own_group_by_value() {
        let axis = UnitAxis {
            unit_member: "store_days.store".into(),
            time_member: "store_days.day".into(),
        };
        let members = vec!["bondi".to_string()];
        let q = AnomalyQuery {
            measure: "store_days.refund_rate",
            members: &members,
            significance: 0.95,
            since: "2026-03-02".parse().unwrap(),
            now: "2026-03-10T00:00:00Z".parse().unwrap(),
        };
        let entry =
            monitor_entry(&axis, &q, chrono_tz::Australia::Sydney).expect("a valid monitor entry");
        assert_eq!(entry.measure, "store_days.refund_rate");
        assert_eq!(entry.time_dimension, "store_days.day");
        assert_eq!(entry.granularity, oxy_metric_monitoring::Granularity::Day);
        assert_eq!(entry.sensitivity, oxy_metric_monitoring::Sensitivity::High);
        assert_eq!(
            entry.group_by, None,
            "the runner expands it, over exactly the treated units"
        );
        assert_eq!(entry.filters.len(), 1);
        assert_eq!(
            (
                entry.filters[0].member.as_str(),
                entry.filters[0].values.clone()
            ),
            ("store_days.store", members)
        );
        assert_eq!(entry.timezone.as_deref(), Some("Australia/Sydney"));
    }
}

/// A time-series bucket's local hour: `YYYY-MM-DD[T ]HH…`.
fn hour_of(ts: &str) -> Option<Moment> {
    let day = NaiveDate::parse_from_str(ts.get(..10)?, "%Y-%m-%d").ok()?;
    let hour = match ts.get(10..13) {
        Some(h) if h.starts_with(['T', ' ']) => h[1..].parse::<u32>().ok()?,
        _ => 0,
    };
    day.and_hms_opt(hour, 0, 0).map(Moment)
}
