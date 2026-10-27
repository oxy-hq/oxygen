import type { ExperimentDefinition } from "@/types/experiment";

/** A compiled definition shaped like `examples/experiments/customer_orders_pilot.experiment.yml`, priced. */
export const definitionFixture: ExperimentDefinition = {
  experiment: "customer_orders_pilot",
  hypothesis: "A customer-level promo lifts order value per customer-day",
  owner: "ops",
  for_each: "customer",
  assign_by: null,
  lever: {
    on: { via: "fruit_summary" },
    off: { via: "fruit_summary" },
    read_back: {
      when_on: "{{orders.total_order_value}} > 0",
      when_off: "{{orders.total_order_value}} >= 0",
      grace_days: 2
    },
    on_close: "off",
    per: "unit"
  },
  design: {
    timezone: "UTC",
    time_dimension: "orders.order_date",
    waves: [
      { switch_on: "2026-03-16", units: ["alice", "bob"] },
      { switch_on: "2026-03-23", units: ["carol", "dave"] }
    ],
    switchback: null,
    strata: null,
    seed: 42,
    pre_days: 56,
    post_days: 56,
    anticipation_days: 3,
    washout_days: 7,
    coverage_floor: 0.9,
    compliance_floor: 0.8
  },
  outcome: {
    target: "orders.total_order_value",
    driver: null,
    family: 1,
    direction: "increase",
    alpha: 0.05,
    power: 0.8,
    mde_at_proposal: 1.25,
    prior_coefficient: null
  },
  substitution_check: { sibling: "orders.total_orders" },
  guardrails: [
    {
      criteria: { type: "anomaly", measure: "orders.refund_rate", significance: 0.01 },
      action: "abort"
    }
  ]
};
