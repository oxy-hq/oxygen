//! The price request body (index C5's `PriceRequest`): one to six candidates.

use serde::{Deserialize, Serialize};

use super::candidate::Candidate;

/// Candidates per request (index C5: `1..=6`).
pub const MAX_CANDIDATES: usize = 6;

/// The price request's *input*: the candidate designs to compare. Each becomes
/// its own proposal row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceBody {
    pub candidates: Vec<Candidate>,
}

impl PriceBody {
    /// A 400's reason. Each candidate is validated on its own face; the engine
    /// still refuses what it cannot place.
    pub fn validate(&self) -> Result<(), String> {
        if self.candidates.is_empty() {
            return Err("candidates must list at least one design".to_string());
        }
        if self.candidates.len() > MAX_CANDIDATES {
            return Err(format!(
                "at most {MAX_CANDIDATES} candidates, got {}",
                self.candidates.len()
            ));
        }
        for (i, c) in self.candidates.iter().enumerate() {
            c.validate()
                .map_err(|e| format!("candidate {}: {e}", i + 1))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use serde_json::json;

    use super::{MAX_CANDIDATES, PriceBody};
    use crate::server::experiment::candidate::{Candidate, SwitchbackCandidate, WavesCandidate};

    fn waves(sizes: &[usize]) -> Candidate {
        Candidate::Waves(WavesCandidate {
            wave_sizes: sizes.to_vec(),
            first_switch_on: NaiveDate::from_ymd_opt(2026, 9, 7).unwrap().into(),
            spacing_days: 14,
            blocks: None,
            pairs: false,
            assign_by: None,
            alpha: None,
        })
    }

    fn body(candidates: Vec<Candidate>) -> PriceBody {
        PriceBody { candidates }
    }

    #[test]
    fn no_candidates_is_refused() {
        assert!(
            body(vec![])
                .validate()
                .unwrap_err()
                .contains("at least one")
        );
    }

    #[test]
    fn more_than_six_candidates_are_refused_and_six_are_fine() {
        assert_eq!(MAX_CANDIDATES, 6);
        assert!(
            body(vec![waves(&[2]); 7])
                .validate()
                .unwrap_err()
                .contains("at most 6")
        );
        assert!(body(vec![waves(&[2]); 6]).validate().is_ok());
    }

    #[test]
    fn an_invalid_candidate_is_named_by_its_position() {
        let err = body(vec![waves(&[2]), waves(&[3, 0])])
            .validate()
            .unwrap_err();
        assert!(
            err.starts_with("candidate 2:") && err.contains("wave 2"),
            "{err}"
        );
    }

    #[test]
    fn the_body_is_the_c5_wire_shape() {
        let b = body(vec![
            waves(&[4, 2]),
            Candidate::Switchback(SwitchbackCandidate {
                first_period_on: NaiveDate::from_ymd_opt(2026, 9, 7).unwrap().into(),
                period_days: 7,
                pairs: 6,
                alpha: Some(0.2),
            }),
        ]);
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["candidates"][0]["shape"], "waves");
        assert_eq!(
            v["candidates"][1],
            json!({"shape": "switchback", "first_period_on": "2026-09-07", "period_days": 7, "pairs": 6, "alpha": 0.2})
        );
        assert_eq!(serde_json::from_value::<PriceBody>(v).unwrap(), b);
    }
}
