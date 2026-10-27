//! The expressions `lever.read_back` and `trigger` guardrails are written in:
//! exactly one `{{view.measure}}`, one comparison, one finite number.
//!
//! Deliberately not the automation `conditional` task's evaluator. That one is
//! private to `agentic-automation`, renders under `UndefinedBehavior::Chainable`
//! and swallows render errors, so a misspelt measure reads as `false` — which a
//! read-back would record as "never landed". Here a malformed expression is a
//! refusal that names what was expected.

mod trigger;
pub use trigger::{Operand, Trigger, parse_trigger};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub measure: String,
    pub op: Op,
    pub rhs: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{expr}` is not a `{{{{view.measure}}}} <op> <number>` comparison: {why}")]
pub struct ExprError {
    pub expr: String,
    pub why: String,
}

/// Two-character operators first, or `>=` would be read as `>` followed by `=1`.
const OPS: [(&str, Op); 6] = [
    (">=", Op::Ge),
    ("<=", Op::Le),
    ("==", Op::Eq),
    ("!=", Op::Ne),
    (">", Op::Gt),
    ("<", Op::Lt),
];

/// Exactly two non-empty segments split by one `.`, each made of word characters
/// (`\w`: alphanumerics and `_`) — the rule airlayer itself reads a
/// `{{view.field}}` reference with (`dotted_ref_regex`: `(\w+)\.(\w+)`).
fn is_view_measure(text: &str) -> bool {
    let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
    text.split_once('.')
        .is_some_and(|(view, measure)| word(view) && word(measure))
}

pub fn parse_comparison(expr: &str) -> Result<Comparison, ExprError> {
    let err = |why: &str| ExprError {
        expr: expr.trim().to_string(),
        why: why.to_string(),
    };
    let (measure, tail) =
        take_ref(expr.trim(), "it must start with `{{view.measure}}`").map_err(err)?;
    let (op, rhs) = op_and_number(tail, "after the measure").map_err(|why| err(&why))?;
    Ok(Comparison {
        measure: measure.to_string(),
        op,
        rhs,
    })
}

/// `{{view.measure}}` at the start of `text`, and what follows it.
fn take_ref<'a>(text: &'a str, missing: &'static str) -> Result<(&'a str, &'a str), &'static str> {
    let rest = text.strip_prefix("{{").ok_or(missing)?;
    let close = rest.find("}}").ok_or("the `{{` is never closed")?;
    let measure = rest[..close].trim();
    if !is_view_measure(measure) {
        return Err("the reference must be one `view.measure`");
    }
    Ok((measure, rest[close + 2..].trim()))
}

/// `<op> <finite number>`, the whole of what is left.
fn op_and_number(tail: &str, after: &str) -> Result<(Op, f64), String> {
    let (op, literal) = OPS
        .iter()
        .find_map(|(tok, op)| tail.strip_prefix(tok).map(|lit| (*op, lit)))
        .ok_or_else(|| format!("expected one of > >= < <= == != {after}"))?;
    let rhs: f64 = literal
        .trim()
        .parse()
        .map_err(|_| "the right-hand side must be a number".to_string())?;
    if !rhs.is_finite() {
        return Err("the right-hand side must be finite".to_string());
    }
    Ok((op, rhs))
}

fn holds(op: Op, value: f64, rhs: f64) -> bool {
    match op {
        Op::Gt => value > rhs,
        Op::Ge => value >= rhs,
        Op::Lt => value < rhs,
        Op::Le => value <= rhs,
        Op::Eq => value == rhs,
        Op::Ne => value != rhs,
    }
}

impl Comparison {
    /// Whether `value` (the measure's value for one unit on one day) satisfies
    /// the comparison. The value is always on the left, as it is written.
    pub fn holds(&self, value: f64) -> bool {
        holds(self.op, value, self.rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(measure: &str, op: Op, rhs: f64) -> Comparison {
        Comparison {
            measure: measure.to_string(),
            op,
            rhs,
        }
    }

    #[test]
    fn parses_the_three_expressions_the_spec_writes() {
        assert_eq!(
            parse_comparison("{{store_days.promo_active}} > 0").unwrap(),
            cmp("store_days.promo_active", Op::Gt, 0.0)
        );
        assert_eq!(
            parse_comparison("{{store_days.promo_active}} == 0").unwrap(),
            cmp("store_days.promo_active", Op::Eq, 0.0)
        );
        assert_eq!(
            parse_comparison("{{store_days.labor_cost_pct}} > 0.34").unwrap(),
            cmp("store_days.labor_cost_pct", Op::Gt, 0.34)
        );
    }

    #[test]
    fn a_two_character_operator_is_not_read_as_its_first_character() {
        assert_eq!(parse_comparison("{{a.b}} >= 1").unwrap().op, Op::Ge);
        assert_eq!(parse_comparison("{{a.b}} <= 1").unwrap().op, Op::Le);
        assert_eq!(parse_comparison("{{a.b}} != 1").unwrap().op, Op::Ne);
    }

    #[test]
    fn whitespace_inside_the_braces_is_tolerated() {
        assert_eq!(
            parse_comparison("  {{ a.b }}   <   -2.5 ").unwrap(),
            cmp("a.b", Op::Lt, -2.5)
        );
    }

    #[test]
    fn refuses_what_it_cannot_evaluate_and_says_what_it_expected() {
        for (bad, needle) in [
            ("store_days.promo_active > 0", "must start with"),
            ("{{store_days.promo_active > 0", "never closed"),
            ("{{promo_active}} > 0", "one `view.measure`"),
            ("{{{a.b}} > 1", "one `view.measure`"),
            ("{{.b}} > 1", "one `view.measure`"),
            ("{{a.}} > 1", "one `view.measure`"),
            ("{{a..b}} > 1", "one `view.measure`"),
            ("{{a.b|round}} > 1", "one `view.measure`"),
            ("{{a.b.c}} > 1", "one `view.measure`"),
            ("{{}} > 1", "one `view.measure`"),
            ("{{a.b}} >", "must be a number"),
            ("{{a.b}} > nan", "must be finite"),
            ("{{a.b}} => 1", "expected one of"),
            ("{{a.b}} > yes", "must be a number"),
            ("{{a.b}} > 1 and {{a.c}} < 2", "must be a number"),
            ("{{a.b}} > inf", "must be finite"),
        ] {
            let err = parse_comparison(bad).expect_err(bad);
            assert!(err.to_string().contains(needle), "{bad}: {err}");
            assert!(
                err.to_string().contains(bad.trim()),
                "the message names the expression: {err}"
            );
        }
    }

    #[test]
    fn identifiers_follow_airlayers_word_character_rule() {
        // airlayer reads `{{view.field}}` with `\w+` on both sides.
        assert_eq!(
            parse_comparison("{{store_days2.Promo_Active}} > 0")
                .unwrap()
                .measure,
            "store_days2.Promo_Active"
        );
        assert!(parse_comparison("{{a-b.c}} > 0").is_err());
    }

    #[test]
    fn holds_compares_the_value_on_the_left() {
        let c = |op| cmp("a.b", op, 1.0);
        assert!(c(Op::Gt).holds(2.0) && !c(Op::Gt).holds(1.0));
        assert!(c(Op::Ge).holds(1.0) && !c(Op::Ge).holds(0.5));
        assert!(c(Op::Lt).holds(0.0) && !c(Op::Lt).holds(1.0));
        assert!(c(Op::Le).holds(1.0) && !c(Op::Le).holds(1.5));
        assert!(c(Op::Eq).holds(1.0) && !c(Op::Eq).holds(1.0001));
        assert!(c(Op::Ne).holds(0.0) && !c(Op::Ne).holds(1.0));
    }
}
