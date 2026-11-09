//! Which units a run compares: every unit with `outcome.target` rows in the design's
//! own **pre window**, read as the panel reads them (the design's entity, time
//! dimension, grain and timezone). One rule for a live run's first check and a
//! retrospective run's estimate, so both pin from the same history, never from the
//! units the warehouse holds today.

use async_trait::async_trait;
use chrono::NaiveDate;
use oxy_experiment::ExperimentConfig;
use oxy_experiment::timeline::first_switch;

use crate::experiment::panel::{HistoryBounds, PanelContext, PanelOutcome, PanelRequest};
use crate::experiment::reads::time_dimension_for;
use crate::experiment::source::PanelSource;

#[async_trait]
pub trait UnitUniverse: Send + Sync {
    /// Every unit with `outcome.target` rows in `[from, to)` — the design's pre window
    /// ([`pre_window`]) or a fleet's lookback ([`fleet_window`]). Never empty: an empty
    /// read is an `Err`, since a pin is write-once and an empty one would stand for good.
    async fn units(
        &self,
        c: &ExperimentConfig,
        window: (NaiveDate, NaiveDate),
    ) -> Result<Vec<String>, String>;
}

/// The production universe: the panel, through the workspace's promoted model.
pub struct PanelUnits<'a> {
    pub panels: &'a dyn PanelSource,
    pub ctx: PanelContext,
}

#[async_trait]
impl UnitUniverse for PanelUnits<'_> {
    async fn units(
        &self,
        c: &ExperimentConfig,
        window: (NaiveDate, NaiveDate),
    ) -> Result<Vec<String>, String> {
        window_units(self.panels, &self.ctx, c, window).await
    }
}

/// `[first switch − anticipation − periods, first switch − anticipation)`, in periods of
/// `design.grain`: the history before anything could have moved the units.
pub fn pre_window(c: &ExperimentConfig, periods: u32) -> Option<(NaiveDate, NaiveDate)> {
    let grain = c.design.grain;
    let first = first_switch(c)?.date();
    let to = grain.sub_covering(first, c.design.anticipation_days)?;
    Some((grain.sub_covering(to, periods)?, to))
}

/// The pre window has not finished by `today` (its last day is still to come or still
/// loading): a pin now would hold only the units seen so far, and it is write-once.
pub fn pre_window_open(c: &ExperimentConfig, periods: u32, today: NaiveDate) -> bool {
    pre_window(c, periods).is_some_and(|(_, to)| today < to)
}

/// A switchback fleet's `periods` of history: the pre window once it has ended; before
/// that, the same length ending `today`. A fleet must be pinned before the first period
/// starts (its flips wait on it), so it cannot wait for the window to end, and a full
/// lookback ending earlier beats part of one.
pub fn fleet_window(
    c: &ExperimentConfig,
    periods: u32,
    today: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let (from, to) = pre_window(c, periods)?;
    if today >= to {
        return Some((from, to));
    }
    Some((c.design.grain.sub_covering(today, periods)?, today))
}

/// The panel request that lists the units in the pre window.
pub fn pre_window_request(c: &ExperimentConfig, from: NaiveDate, to: NaiveDate) -> PanelRequest {
    let target = &c.outcome.target;
    PanelRequest {
        entity: c.for_each.clone(),
        measure: target.clone(),
        time_dimension: time_dimension_for(c.design.time_dimension.as_deref(), target)
            .map(str::to_string),
        grain: c.design.grain,
        timezone: Some(c.design.timezone.name().to_string()),
        history: HistoryBounds {
            from: Some(from),
            to_exclusive: to,
        },
        membership: None,
        restrict_to: None,
        unit_floor: None,
        filters: vec![],
    }
}

/// Every unit with `outcome.target` rows in the pre window of `periods` periods.
pub async fn pre_window_units(
    panels: &dyn PanelSource,
    ctx: &PanelContext,
    c: &ExperimentConfig,
    periods: u32,
) -> Result<Vec<String>, String> {
    let window = pre_window(c, periods).ok_or("the design has no first switch day")?;
    window_units(panels, ctx, c, window).await
}

/// Every unit with `outcome.target` rows in `[from, to)`. An empty answer is an `Err`:
/// an empty pin would stand for good.
pub async fn window_units(
    panels: &dyn PanelSource,
    ctx: &PanelContext,
    c: &ExperimentConfig,
    (from, to): (NaiveDate, NaiveDate),
) -> Result<Vec<String>, String> {
    let target = &c.outcome.target;
    let units = match panels
        .load(ctx, &pre_window_request(c, from, to))
        .await
        .map_err(|e| format!("read the units of `{target}`: {e}"))?
    {
        PanelOutcome::Panel(m) | PanelOutcome::Thinned { panel: m, .. } => m.units().to_vec(),
        PanelOutcome::NoHistory { .. } => Vec::new(),
        PanelOutcome::Refused { reason } => return Err(reason),
    };
    if units.is_empty() {
        return Err(format!(
            "the warehouse holds no `{target}` rows from {from} to before {to}, the design's \
             pre window, so there are no units to compare yet"
        ));
    }
    Ok(units)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use oxy_shared::errors::OxyError;
    use uuid::Uuid;

    use super::*;
    use crate::experiment::test_design::design;

    #[derive(Default)]
    struct Recording(Mutex<Vec<PanelRequest>>);

    #[async_trait]
    impl PanelSource for Recording {
        async fn load(
            &self,
            _: &PanelContext,
            req: &PanelRequest,
        ) -> Result<PanelOutcome, OxyError> {
            self.0.lock().unwrap().push(req.clone());
            Ok(PanelOutcome::NoHistory {
                measure: req.measure.clone(),
                entity: req.entity.clone(),
            })
        }
    }

    #[tokio::test]
    async fn units_are_read_over_the_designs_pre_window_in_its_timezone() {
        let c = design();
        let panels = Recording::default();
        let universe = PanelUnits {
            panels: &panels,
            ctx: PanelContext {
                workspace_id: Uuid::nil(),
                revision_id: Uuid::nil(),
                requested_by: Uuid::nil(),
            },
        };
        let pre = c.design.pre_days.expect("pre_days");
        let (from, to) = pre_window(&c, pre).unwrap();
        let err = universe.units(&c, (from, to)).await.unwrap_err();
        assert!(
            err.contains("pre window"),
            "an empty read never pins: {err}"
        );
        let req = panels.0.lock().unwrap()[0].clone();
        assert_eq!(req.history.from, Some(from));
        assert_eq!(req.history.to_exclusive, to);
        assert_eq!(req.measure, c.outcome.target);
        assert_eq!(req.entity, c.for_each);
        assert_eq!(req.grain, c.design.grain);
        assert_eq!(req.timezone.as_deref(), Some(c.design.timezone.name()));
        assert!(
            to <= first_switch(&c).unwrap().date(),
            "before the first switch"
        );
    }

    #[test]
    fn the_pre_window_is_open_until_its_last_day_is_over() {
        let c = design();
        let pre = c.design.pre_days.expect("pre_days");
        let (from, to) = pre_window(&c, pre).unwrap();
        assert!(pre_window_open(&c, pre, from));
        assert!(
            pre_window_open(&c, pre, to.pred_opt().unwrap()),
            "its last day"
        );
        assert!(!pre_window_open(&c, pre, to));
    }

    #[test]
    fn a_fleet_read_before_its_window_ends_is_a_full_lookback_ending_today() {
        let c = design();
        let (from, to) = pre_window(&c, 28).unwrap();
        assert_eq!(fleet_window(&c, 28, to), Some((from, to)));
        let early = to - chrono::Days::new(10);
        assert_eq!(
            fleet_window(&c, 28, early),
            Some((early - chrono::Days::new(28), early))
        );
    }
}
