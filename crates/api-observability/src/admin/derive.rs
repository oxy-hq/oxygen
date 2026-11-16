//! The derived numbers on Admin → Observability, as pure functions.
//!
//! A total with no denominator is not actionable: "$412 this month" does not
//! say whether anything is wrong, and "$412, up from $180, at $1.40 a run
//! against $0.60 last window" does. Everything here turns a measured total
//! into a number someone can act on, and every one of them has an honest
//! answer for an empty window — which is `None`, not zero.
//!
//! Pure on purpose: no clock, no I/O, no rounding for display. The surface
//! that renders these formats them; the API reports them at full precision.

/// A share of a total, as a fraction in `0.0..=1.0`.
///
/// `None` when the total is zero. A zero denominator has **no opinion** about
/// composition — "no spend" and "none of the spend was scheduled" are
/// different facts, and returning 0.0 would draw an empty org as a fully
/// interactive one.
pub fn share(part: f64, total: f64) -> Option<f64> {
    if total <= 0.0 {
        return None;
    }
    Some(part / total)
}

/// Cost per unit — per run, per person, per session.
///
/// `None` on a zero denominator, for the reason above and one more: a window
/// with spend but no runs means the two sides were measured over different
/// things, and a division there would report an enormous per-run cost that
/// looks like a defect in the platform rather than in the question.
pub fn per_unit(cost_usd: f64, units: u64) -> Option<f64> {
    if units == 0 {
        return None;
    }
    Some(cost_usd / units as f64)
}

/// What this window's spend annualises to as a monthly run-rate.
///
/// A 7-day window times 30/7, a 30-day window unchanged. Reported because the
/// question behind a cost page is "what will this cost us", and a 7-day total
/// answers a different one — the mistake Vercel's usage page avoids by
/// defaulting to 30 days.
///
/// `None` for a zero-length window: dividing by it would report an infinite
/// run-rate from a request someone typo'd.
pub fn monthly_run_rate(window_cost_usd: f64, window_days: u32) -> Option<f64> {
    if window_days == 0 {
        return None;
    }
    Some(window_cost_usd / f64::from(window_days) * 30.0)
}

/// How much of the prompt was served from cache.
///
/// `cache_read / (input + cache_creation + cache_read)` — **the same
/// definition `agentic_core::ui_stream` documents**, so this surface and the
/// chat stream cannot report two different cache rates for one workload.
///
/// All three buckets belong in the denominator because pricing treats them as
/// three: a token written to cache is a prompt token that is *not* counted in
/// `input_tokens`. Leaving cache-creation out overstates the share, and
/// overstates it most for the workload that just started caching — exactly
/// when an operator is deciding whether it worked. With 1k input, 10k
/// cache-creation and 90k cache-read, the wrong denominator reads 98.9% where
/// the true share is 89%.
///
/// This is the main LLM cost lever and it is invisible on every other surface:
/// a prompt whose cached prefix stops matching costs several times as much per
/// run with no other symptom.
pub fn cache_hit_share(
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> Option<f64> {
    let read = input_tokens.max(0) + cache_creation_tokens.max(0) + cache_read_tokens.max(0);
    if read == 0 {
        return None;
    }
    Some(cache_read_tokens.max(0) as f64 / read as f64)
}

/// Change from one window to the one before it, as a fraction.
///
/// `None` when the previous window was zero: everything is an infinite
/// increase from nothing, and rendering that as `+∞%` or `+100%` both mislead.
/// The caller says "new" instead.
pub fn delta_share(current: f64, previous: f64) -> Option<f64> {
    if previous <= 0.0 {
        return None;
    }
    Some((current - previous) / previous)
}

/// Failure rate over a request count.
///
/// Mirrors `OrgAppTotals::failure_ratio` and exists separately because the
/// fleet list computes it from two columns it already holds rather than from a
/// totals struct. `None` on no traffic: no requests have no opinion about
/// reliability, and 0.0 would render a dead org as a perfect one.
pub fn failure_rate(failed: u64, requests: u64) -> Option<f64> {
    if requests == 0 {
        return None;
    }
    Some(failed as f64 / requests as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ratio refuses a zero denominator rather than answering zero. This
    /// is the whole contract of the module: an empty window must look empty,
    /// not healthy. The admin console's own rule — "a source that did not
    /// answer is a third outcome, not an empty one" — is the same mistake one
    /// layer up, and it shipped a 500 rendered under a green tick.
    #[test]
    fn every_ratio_has_no_opinion_on_an_empty_window() {
        assert_eq!(share(0.0, 0.0), None);
        assert_eq!(per_unit(12.0, 0), None);
        assert_eq!(monthly_run_rate(12.0, 0), None);
        assert_eq!(cache_hit_share(0, 0, 0), None);
        assert_eq!(delta_share(5.0, 0.0), None);
        assert_eq!(failure_rate(0, 0), None);
    }

    /// A negative or zero total is refused too, not just an exactly-zero one.
    /// Nothing should produce a negative total, and if something does, a
    /// negative share is a worse symptom than a missing one.
    #[test]
    fn a_nonpositive_total_is_refused_not_inverted() {
        assert_eq!(share(1.0, -1.0), None);
        assert_eq!(delta_share(1.0, -1.0), None);
    }

    #[test]
    fn shares_and_per_unit_costs_divide_as_stated() {
        assert_eq!(share(25.0, 100.0), Some(0.25));
        assert_eq!(per_unit(10.0, 4), Some(2.5));
        assert_eq!(failure_rate(3, 12), Some(0.25));
    }

    /// A 7-day window scales to 30 days; a 30-day window does not move. The
    /// run-rate is the number a budget conversation uses, so getting the
    /// scaling backwards would understate a weekly figure by 4×.
    #[test]
    fn the_run_rate_scales_the_window_to_a_month() {
        assert_eq!(monthly_run_rate(70.0, 7), Some(300.0));
        assert_eq!(monthly_run_rate(300.0, 30), Some(300.0));
        // A 90-day window reports a *smaller* monthly figure than its total,
        // which is the point: the total is three months of spend.
        assert_eq!(monthly_run_rate(900.0, 90), Some(300.0));
    }

    /// The cache share counts cached tokens against every prompt token, so it
    /// cannot exceed 1.
    #[test]
    fn the_cache_share_cannot_exceed_one() {
        assert_eq!(cache_hit_share(0, 0, 500), Some(1.0));
        assert_eq!(cache_hit_share(500, 0, 500), Some(0.5));
        assert_eq!(cache_hit_share(1000, 0, 0), Some(0.0));
        for (input, created, cached) in [(1, 2, 10_000), (10_000, 5, 1), (7, 11, 13)] {
            let s = cache_hit_share(input, created, cached).expect("non-empty");
            assert!((0.0..=1.0).contains(&s), "share out of range: {s}");
        }
    }

    /// Cache-creation tokens are in the denominator, matching the definition
    /// `agentic_core::ui_stream` already documents. Leaving them out
    /// overstates the share — and overstates it most for the workload that
    /// just started caching, which is exactly when someone is deciding
    /// whether caching worked.
    #[test]
    fn cache_creation_tokens_are_in_the_denominator() {
        // 1k input, 10k created, 90k read. The wrong denominator gives 98.9%.
        let s = cache_hit_share(1_000, 10_000, 90_000).expect("non-empty");
        assert!((s - 0.891).abs() < 0.001, "expected ~89%, got {s}");
    }

    /// Negative token counts cannot drag a share below zero or above one. They
    /// should never occur, but they arrive from a JSONB cast and the ratio is
    /// rendered as a bar — a negative width is a broken page, not a bad number.
    #[test]
    fn negative_token_counts_are_clamped_not_propagated() {
        assert_eq!(cache_hit_share(-5, -5, 10), Some(1.0));
        assert_eq!(cache_hit_share(10, 0, -5), Some(0.0));
    }

    /// A drop reads as a negative delta, not an absolute one. A page that
    /// showed "40% change" for a halving would be read as a rise.
    #[test]
    fn a_fall_reads_as_a_negative_delta() {
        assert_eq!(delta_share(50.0, 100.0), Some(-0.5));
        assert_eq!(delta_share(150.0, 100.0), Some(0.5));
    }
}
