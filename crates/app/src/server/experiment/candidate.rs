//! The price request's candidate designs (index C5): a request names several,
//! each becomes its own proposal, and they are listed side by side.

use oxy_experiment::Moment;
use serde::{Deserialize, Serialize};

/// At most this many waves in one candidate; a calendar of more is a different tool.
const MAX_WAVES: usize = 52;
/// Longest gap between waves, in days.
const MAX_SPACING_DAYS: i64 = 366;
/// Longest switchback period, in days (eight weeks).
const MAX_PERIOD_DAYS: u32 = 56;
/// Most switchback pairs in one candidate.
const MAX_PAIRS: usize = 26;

/// One design the request asks to have priced. Wire shape = index C5, tagged by
/// `shape`; the tag is required and an unknown one is rejected, never defaulted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Candidate {
    Waves(WavesCandidate),
    Switchback(SwitchbackCandidate),
}

/// Waves of units — or of groups, under `assign_by` — optionally blocked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WavesCandidate {
    pub wave_sizes: Vec<usize>,
    /// A date, or at hour grain a local whole hour (`2026-10-26T14:00`).
    pub first_switch_on: Moment,
    /// Ignored with one wave. (C5 types it as required; a single-wave caller
    /// may omit it, which is the only leniency.)
    #[serde(default)]
    pub spacing_days: i64,
    /// `>= 2` → stratified into this many blocks by baseline; omitted = unblocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<usize>,
    /// Matched pairs: units paired by baseline and one of each pair treated, on one
    /// switch date. Pricing sets the wave and the blocks, so `wave_sizes` stays empty
    /// and `blocks` absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pairs: bool,
    /// A dimension of `for_each`: randomise its values instead of single units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assign_by: Option<String>,
    /// Overrides `outcome.alpha` for this candidate (the directional preset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
}

/// The whole fleet alternating on and off: `pairs` consecutive pairs of
/// `period_days` periods from `first_period_on`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwitchbackCandidate {
    /// As `first_switch_on`.
    pub first_period_on: Moment,
    pub period_days: u32,
    pub pairs: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
}

fn valid_alpha(alpha: Option<f64>) -> Result<(), String> {
    match alpha {
        Some(a) if !(a > 0.0 && a < 1.0) => {
            Err(format!("alpha must be strictly between 0 and 1, got {a}"))
        }
        _ => Ok(()),
    }
}

impl WavesCandidate {
    fn validate(&self) -> Result<(), String> {
        if self.pairs {
            return self.validate_pairs();
        }
        if self.wave_sizes.is_empty() {
            return Err("wave_sizes must list at least one wave".to_string());
        }
        if self.wave_sizes.len() > MAX_WAVES {
            return Err(format!(
                "at most {MAX_WAVES} waves, got {}",
                self.wave_sizes.len()
            ));
        }
        if let Some(i) = self.wave_sizes.iter().position(|&n| n == 0) {
            return Err(format!("wave {} has size 0", i + 1));
        }
        if self.wave_sizes.len() > 1 && !(1..=MAX_SPACING_DAYS).contains(&self.spacing_days) {
            return Err(format!(
                "spacing_days must be between 1 and {MAX_SPACING_DAYS} when there is more than one wave, got {}",
                self.spacing_days
            ));
        }
        if self.blocks.is_some_and(|b| b < 2) {
            return Err(
                "blocks must be at least 2 when given; omit it for an unblocked draw".to_string(),
            );
        }
        if self
            .assign_by
            .as_deref()
            .is_some_and(|s| s.trim().is_empty())
        {
            return Err("assign_by must name a dimension of for_each when given".to_string());
        }
        valid_alpha(self.alpha)
    }

    /// Pairs set the wave and the blocks themselves; giving either too is refused.
    fn validate_pairs(&self) -> Result<(), String> {
        if !self.wave_sizes.is_empty() {
            return Err(
                "pairs choose their own wave (one unit of each pair); leave wave_sizes empty"
                    .to_string(),
            );
        }
        if self.blocks.is_some() {
            return Err(
                "pairs choose their own blocks (one per pair); leave blocks out".to_string(),
            );
        }
        valid_alpha(self.alpha)
    }

    /// Matched pairs over `n` units (or groups) as the plain candidate they price as:
    /// one wave of `n / 2`, in `n / 2` baseline blocks. An odd unit joins a block as a
    /// control. Any other candidate is returned as it is.
    pub fn resolved(&self, n: usize) -> WavesCandidate {
        if !self.pairs {
            return self.clone();
        }
        WavesCandidate {
            wave_sizes: vec![n / 2],
            blocks: Some(n / 2),
            pairs: false,
            ..self.clone()
        }
    }
}

impl SwitchbackCandidate {
    fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_PERIOD_DAYS).contains(&self.period_days) {
            return Err(format!(
                "period_days must be between 1 and {MAX_PERIOD_DAYS}, got {}",
                self.period_days
            ));
        }
        if !(1..=MAX_PAIRS).contains(&self.pairs) {
            return Err(format!(
                "pairs must be between 1 and {MAX_PAIRS}, got {}",
                self.pairs
            ));
        }
        valid_alpha(self.alpha)
    }
}

impl Candidate {
    /// The α override, if the candidate carries one.
    pub fn alpha(&self) -> Option<f64> {
        match self {
            Candidate::Waves(w) => w.alpha,
            Candidate::Switchback(s) => s.alpha,
        }
    }

    /// The grouping dimension, if any. A switchback has none.
    pub fn assign_by(&self) -> Option<&str> {
        match self {
            Candidate::Waves(w) => w.assign_by.as_deref(),
            Candidate::Switchback(_) => None,
        }
    }

    /// A 400's reason. The engine still refuses what it cannot place (more units
    /// asked for than exist); this only refuses what is malformed on its face.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Candidate::Waves(w) => w.validate(),
            Candidate::Switchback(s) => s.validate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use serde_json::json;

    use super::*;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 7).unwrap()
    }

    fn waves(sizes: &[usize], spacing: i64) -> WavesCandidate {
        WavesCandidate {
            wave_sizes: sizes.to_vec(),
            first_switch_on: date().into(),
            spacing_days: spacing,
            blocks: None,
            pairs: false,
            assign_by: None,
            alpha: None,
        }
    }

    fn switchback(period_days: u32, pairs: usize) -> SwitchbackCandidate {
        SwitchbackCandidate {
            first_period_on: date().into(),
            period_days,
            pairs,
            alpha: None,
        }
    }

    fn err(c: impl Into<Candidate>) -> String {
        c.into().validate().unwrap_err()
    }

    impl From<WavesCandidate> for Candidate {
        fn from(c: WavesCandidate) -> Self {
            Candidate::Waves(c)
        }
    }
    impl From<SwitchbackCandidate> for Candidate {
        fn from(c: SwitchbackCandidate) -> Self {
            Candidate::Switchback(c)
        }
    }

    #[test]
    fn no_waves_is_refused() {
        assert!(err(waves(&[], 0)).contains("at least one wave"));
    }

    #[test]
    fn a_zero_size_wave_is_refused() {
        assert!(err(waves(&[3, 0], 14)).contains("wave 2"));
    }

    #[test]
    fn several_waves_need_a_positive_spacing() {
        assert!(Candidate::from(waves(&[3, 3], 0)).validate().is_err());
        assert!(Candidate::from(waves(&[3, 3], 367)).validate().is_err());
        assert!(Candidate::from(waves(&[3, 3], 14)).validate().is_ok());
    }

    #[test]
    fn too_many_waves_are_refused() {
        assert!(Candidate::from(waves(&vec![1; 53], 7)).validate().is_err());
        assert!(Candidate::from(waves(&vec![1; 52], 7)).validate().is_ok());
    }

    #[test]
    fn one_wave_needs_no_spacing_and_any_value_is_accepted() {
        assert!(Candidate::from(waves(&[4], 0)).validate().is_ok());
        assert!(Candidate::from(waves(&[4], 99)).validate().is_ok());
    }

    #[test]
    fn pairs_choose_their_own_waves_and_blocks() {
        let mut c = waves(&[], 0);
        c.pairs = true;
        c.wave_sizes = vec![];
        assert!(c.validate().is_ok(), "pairs need no wave sizes");
        c.blocks = Some(4);
        assert!(err(Candidate::Waves(c.clone())).contains("pairs"));
        c.blocks = None;
        c.wave_sizes = vec![3, 3];
        assert!(err(Candidate::Waves(c)).contains("pairs"));
    }

    #[test]
    fn blocks_must_be_at_least_two_when_given() {
        for bad in [0, 1] {
            let mut c = waves(&[4], 0);
            c.blocks = Some(bad);
            assert!(err(c).contains("blocks"), "{bad}");
        }
        let mut ok = waves(&[4], 0);
        ok.blocks = Some(3);
        assert!(Candidate::from(ok).validate().is_ok());
    }

    #[test]
    fn a_blank_assign_by_is_refused() {
        let mut c = waves(&[4], 0);
        c.assign_by = Some("  ".to_string());
        assert!(err(c).contains("assign_by"));
    }

    #[test]
    fn alpha_must_be_strictly_between_zero_and_one() {
        for bad in [0.0, 1.0, -0.1, 1.5, f64::NAN] {
            let mut c = waves(&[4], 0);
            c.alpha = Some(bad);
            assert!(err(c).contains("alpha"), "{bad}");
            let mut s = switchback(7, 6);
            s.alpha = Some(bad);
            assert!(err(s).contains("alpha"), "{bad}");
        }
        let mut ok = waves(&[4], 0);
        ok.alpha = Some(0.2);
        assert_eq!(Candidate::from(ok).alpha(), Some(0.2));
    }

    #[test]
    fn a_switchback_needs_a_period_and_at_least_one_pair() {
        assert!(err(switchback(0, 6)).contains("period_days"));
        assert!(err(switchback(57, 6)).contains("period_days"));
        assert!(err(switchback(7, 0)).contains("pairs"));
        assert!(err(switchback(7, 27)).contains("pairs"));
        assert!(Candidate::from(switchback(7, 6)).validate().is_ok());
    }

    #[test]
    fn the_wire_shape_is_the_c5_candidate() {
        let c = Candidate::Waves(waves(&[4, 2], 14));
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(
            v,
            json!({"shape": "waves", "wave_sizes": [4, 2], "first_switch_on": "2026-09-07", "spacing_days": 14})
        );
        assert_eq!(serde_json::from_value::<Candidate>(v).unwrap(), c);

        let s = Candidate::Switchback(SwitchbackCandidate {
            alpha: Some(0.2),
            ..switchback(7, 4)
        });
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(
            v,
            json!({"shape": "switchback", "first_period_on": "2026-09-07", "period_days": 7, "pairs": 4, "alpha": 0.2})
        );
        assert_eq!(serde_json::from_value::<Candidate>(v).unwrap(), s);
    }

    #[test]
    fn a_shape_the_api_does_not_know_is_rejected_not_defaulted() {
        let v = json!({"shape": "crossover", "first_switch_on": "2026-09-07", "wave_sizes": [1]});
        assert!(serde_json::from_value::<Candidate>(v).is_err());
        let missing = json!({"wave_sizes": [1], "first_switch_on": "2026-09-07"});
        assert!(serde_json::from_value::<Candidate>(missing).is_err());
    }

    #[test]
    fn accessors_read_through_the_shape() {
        let mut w = waves(&[4], 0);
        w.assign_by = Some("sku.category".to_string());
        assert_eq!(Candidate::Waves(w).assign_by(), Some("sku.category"));
        assert_eq!(Candidate::Switchback(switchback(7, 6)).assign_by(), None);
    }
}
