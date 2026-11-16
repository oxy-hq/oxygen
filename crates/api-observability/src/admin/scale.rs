//! How big one org is against the rest, per dimension.
//!
//! The question behind the fleet list's table is "what does each org cost us".
//! The question behind this is different: **which org is an outlier, and on
//! which axis.** An account that is 12× the median on LLM but 1.2× on storage
//! is a different business to serve from one where that is reversed, and no
//! column of absolute figures makes that legible — the biggest tenant is
//! simply biggest everywhere, and the shape is lost.
//!
//! ## Against the median, not the mean
//!
//! Six orgs with one dominant tenant: the mean *is* that tenant, so every other
//! org indexes near zero against it and the view says nothing. The median is
//! the typical account, which is the comparison an operator actually wants, and
//! it does not move when the largest tenant doubles.
//!
//! ## This module supplies the median, and only the median
//!
//! The division is the client's: `web-app/src/pages/admin/AdminObservability/
//! scaleDimensions.ts` owns `indexOf`, including the rule that a zero or absent
//! median yields **no index** rather than an `∞×`, and the rule that a measured
//! zero is a real `0×`. It owns them because that is where the index is
//! rendered beside the absolute figure — "40× the median" over a median of two
//! requests is noise wearing a suit, and the two are only honest together.
//!
//! A Rust `index_against` and a `rank_fraction` lived here, tested and
//! documented, with no caller: the index duplicated the client's rule and the
//! rank was never rendered anywhere, though its doc said it was "reported
//! beside the index". Both are gone. Send the median; let the surface that
//! draws the comparison compute it.

/// The middle value of a sample, or `None` for an empty one.
///
/// Even-length samples average the two middle values, which is the ordinary
/// definition and matters here: with four orgs the median must not silently
/// become "the second one".
pub fn median(values: &[f64]) -> Option<f64> {
    let mut finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return None;
    }
    finite.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = finite.len() / 2;
    Some(if finite.len() % 2 == 0 {
        (finite[mid - 1] + finite[mid]) / 2.0
    } else {
        finite[mid]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The median is the typical account and does not move when the largest
    /// tenant doubles. This is the property the whole view rests on: with a
    /// mean, one dominant org makes every other org index to nearly zero and
    /// the comparison says nothing.
    #[test]
    fn the_median_ignores_a_dominant_outlier() {
        let normal = [10.0, 12.0, 14.0, 16.0, 5_000.0];
        let doubled = [10.0, 12.0, 14.0, 16.0, 10_000.0];
        assert_eq!(median(&normal), Some(14.0));
        assert_eq!(median(&doubled), Some(14.0), "the median must not move");

        // The mean would, by a factor of nearly two — which is why it is not
        // what this module uses.
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        assert!(mean(&doubled) > mean(&normal) * 1.8);
    }

    #[test]
    fn an_even_sample_averages_the_two_middle_values() {
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
        assert_eq!(median(&[10.0]), Some(10.0));
        assert_eq!(median(&[]), None);
    }

    /// Non-finite values never reach the sort. A NaN from a future division
    /// makes `partial_cmp` return `None` and the sort order undefined, which
    /// would silently pick a wrong median rather than fail.
    #[test]
    fn non_finite_values_are_excluded_from_the_sample() {
        assert_eq!(median(&[1.0, f64::NAN, 3.0]), Some(2.0));
        assert_eq!(median(&[f64::NAN, f64::INFINITY]), None);
    }
}
