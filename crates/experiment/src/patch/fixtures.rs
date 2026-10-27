//! The `.view.yml` text every patch test edits. Two measures carry an edge from
//! `promo_redemptions`: only `beverages_revenue`'s may change.

pub const VIEW: &str = "\
name: store_days
# Daily store rollup.
measures:
  - name: promo_redemptions
    type: sum
    expr: promo_redemptions

  - name: beverages_revenue
    type: sum
    expr: beverages_revenue
    drivers:
      # declared by ops, 2026-01
      - measure: store_days.promo_redemptions
        direction: positive
        strength: strong
        description: Each redemption pulls a beverage add-on.
      - measure: store_days.foot_traffic
        coefficient: 0.5
        form: linear
  - name: entree_revenue
    type: sum
    expr: entree_revenue
    drivers:
      - measure: store_days.promo_redemptions
        coefficient: -1.0
        form: linear
";

/// The qualitative edge as it appears in [`VIEW`] — the block tests replace.
pub const QUALITATIVE_EDGE: &str = "\
      - measure: store_days.promo_redemptions
        direction: positive
        strength: strong
        description: Each redemption pulls a beverage add-on.
";

pub const TARGET: &str = "store_days.beverages_revenue";
pub const DRIVER: &str = "store_days.promo_redemptions";
pub const REFS_ENTRY: &str = "experiment:beverage_promo_pilot@v3";

/// How the compile boundary reads a view: YAML text into a `serde_json::Value`.
pub fn view_value(text: &str) -> serde_json::Value {
    serde_yaml::from_str(text).expect("fixture parses")
}
