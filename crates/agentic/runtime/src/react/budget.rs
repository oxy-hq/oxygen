//! Run budgets: turns, tokens and cost, checked between model turns.

use agentic_core::react::{Entry, TurnUsage};

use super::driver::Budgets;

/// Accumulates usage across the turns of one run.
///
/// Tokens are input + output (cache reads and writes are not added: they are
/// already priced into `cost_usd` and would double-count the prompt). Cost is
/// the sum of the turns that report a price; a turn with `cost_usd: None`
/// contributes nothing.
#[derive(Debug, Clone)]
pub struct BudgetMeter {
    budgets: Budgets,
    turns: u32,
    tokens: u64,
    cost_usd: f64,
}

impl BudgetMeter {
    pub fn new(budgets: Budgets) -> Self {
        Self {
            budgets,
            turns: 0,
            tokens: 0,
            cost_usd: 0.0,
        }
    }

    /// A meter charged for every recorded tool-calling turn in `entries`
    /// (the turns a resumed run already took). A final reply is never charged,
    /// as in the live loop.
    pub(super) fn preloaded(budgets: Budgets, entries: &[Entry]) -> Self {
        let mut m = Self::new(budgets);
        for e in entries {
            match e {
                Entry::Assistant(t) if !t.tool_calls.is_empty() => m.charge(&t.usage),
                _ => {}
            }
        }
        m
    }

    /// Adds one completed model turn.
    pub fn charge(&mut self, usage: &TurnUsage) {
        self.turns += 1;
        self.tokens += usage.input_tokens + usage.output_tokens;
        self.cost_usd += usage.cost_usd.unwrap_or(0.0);
    }

    /// `Some("budget exceeded: <which> <used>/<limit>")` once a limit is hit.
    /// Turns end the run at the limit; tokens and cost when they pass it.
    pub fn exceeded(&self) -> Option<String> {
        let b = &self.budgets;
        if self.turns >= b.max_turns {
            return Some(format!(
                "budget exceeded: turns {}/{}",
                self.turns, b.max_turns
            ));
        }
        if self.tokens > b.max_tokens {
            return Some(format!(
                "budget exceeded: tokens {}/{}",
                self.tokens, b.max_tokens
            ));
        }
        match b.max_cost_usd {
            Some(max) if self.cost_usd > max => Some(format!(
                "budget exceeded: cost {:.4}/{:.4}",
                self.cost_usd, max
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(i: u64, o: u64, cost: Option<f64>) -> TurnUsage {
        TurnUsage {
            input_tokens: i,
            output_tokens: o,
            cost_usd: cost,
            ..TurnUsage::default()
        }
    }

    #[test]
    fn react_meter_sums_tokens_and_skips_unpriced_turns() {
        let mut m = BudgetMeter::new(Budgets {
            max_turns: 10,
            max_tokens: 100,
            max_cost_usd: Some(1.0),
        });
        m.charge(&usage(40, 10, None));
        assert_eq!(m.exceeded(), None);
        m.charge(&usage(40, 11, Some(0.5)));
        assert_eq!(m.exceeded().unwrap(), "budget exceeded: tokens 101/100");
    }
}
