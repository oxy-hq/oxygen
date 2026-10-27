//! A trigger guardrail's expression: one measure, or the ratio of two, against a
//! number — `{{ads.spend}} / {{ads.sales}} > 0.6`. A ratio is evaluated per unit-day
//! as numerator / denominator; a day whose denominator is 0 has no reading.
//! Read-back stays [`super::parse_comparison`]: a lever lands or it does not.

use super::{ExprError, Op, holds, op_and_number, take_ref};

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Measure(String),
    Ratio {
        numerator: String,
        denominator: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    pub lhs: Operand,
    pub op: Op,
    pub rhs: f64,
}

pub fn parse_trigger(expr: &str) -> Result<Trigger, ExprError> {
    let err = |why: &str| ExprError {
        expr: expr.trim().to_string(),
        why: why.to_string(),
    };
    let start = "it must start with `{{view.measure}}`";
    let (first, tail) = take_ref(expr.trim(), start).map_err(err)?;
    let (lhs, tail) = match tail.strip_prefix('/') {
        Some(rest) => {
            let (second, tail) =
                take_ref(rest.trim(), "expected `{{view.measure}}` after `/`").map_err(err)?;
            let lhs = Operand::Ratio {
                numerator: first.to_string(),
                denominator: second.to_string(),
            };
            (lhs, tail)
        }
        None => (Operand::Measure(first.to_string()), tail),
    };
    let (op, rhs) = op_and_number(tail, "after the measure or ratio").map_err(|why| err(&why))?;
    Ok(Trigger { lhs, op, rhs })
}

impl Trigger {
    /// The measures to read, numerator first.
    pub fn measures(&self) -> Vec<&str> {
        match &self.lhs {
            Operand::Measure(m) => vec![m],
            Operand::Ratio {
                numerator,
                denominator,
            } => vec![numerator, denominator],
        }
    }

    /// The left-hand side for one unit-day, from the values of [`Self::measures`]
    /// in order; `None` when a ratio's denominator is 0 (there is no reading).
    pub fn value(&self, values: &[f64]) -> Option<f64> {
        match (&self.lhs, values) {
            (Operand::Measure(_), [v]) => Some(*v),
            (Operand::Ratio { .. }, [n, d]) => (*d != 0.0).then(|| n / d),
            _ => None,
        }
    }

    pub fn holds(&self, value: f64) -> bool {
        holds(self.op, value, self.rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ratio(n: &str, d: &str) -> Operand {
        Operand::Ratio {
            numerator: n.into(),
            denominator: d.into(),
        }
    }

    #[test]
    fn a_single_measure_parses_as_before() {
        let t = parse_trigger("{{store_days.labor_cost_pct}} > 0.34").unwrap();
        assert_eq!(t.lhs, Operand::Measure("store_days.labor_cost_pct".into()));
        assert_eq!((t.op, t.rhs), (Op::Gt, 0.34));
        assert_eq!(t.measures(), vec!["store_days.labor_cost_pct"]);
    }

    #[test]
    fn a_ratio_of_two_measures_parses_with_its_parts_in_order() {
        let t =
            parse_trigger(" {{ ad_days.spend }} / {{ad_days.attributed_sales}} >= 0.6 ").unwrap();
        assert_eq!(t.lhs, ratio("ad_days.spend", "ad_days.attributed_sales"));
        assert_eq!((t.op, t.rhs), (Op::Ge, 0.6));
        assert_eq!(
            t.measures(),
            vec!["ad_days.spend", "ad_days.attributed_sales"]
        );
    }

    #[test]
    fn a_ratio_reads_numerator_over_denominator_and_no_reading_when_it_is_zero() {
        let t = parse_trigger("{{a.s}} / {{a.r}} > 0.6").unwrap();
        assert_eq!(t.value(&[90.0, 100.0]), Some(0.9));
        assert!(t.holds(t.value(&[90.0, 100.0]).unwrap()));
        assert_eq!(t.value(&[90.0, 0.0]), None, "no sales: no ACoS that day");
        let single = parse_trigger("{{a.s}} > 1").unwrap();
        assert_eq!(single.value(&[2.0]), Some(2.0));
    }

    #[test]
    fn refuses_anything_but_one_measure_or_one_ratio() {
        for (bad, needle) in [
            ("{{a.b}} / 2 > 1", "`{{view.measure}}` after `/`"),
            ("{{a.b}} * {{a.c}} > 1", "expected one of"),
            ("{{a.b}} / {{a.c}} / {{a.d}} > 1", "expected one of"),
            ("{{a.b}} / {{c}} > 1", "one `view.measure`"),
            ("{{a.b}} / {{a.c}}", "expected one of"),
        ] {
            let err = parse_trigger(bad).expect_err(bad);
            assert!(err.to_string().contains(needle), "{bad}: {err}");
        }
    }
}
