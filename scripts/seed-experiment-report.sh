#!/usr/bin/env bash
# Seeds ONE finished switchback run and its released report into the cloud
# backend's Postgres, so the agentic flow `experiments-price-draft` (case 2) has a
# directional report to read. Run AFTER `oxy seed` (it needs the Demo workspace, whose
# promoted revision the report's views resolve at) and BEFORE the runner. Idempotent: a second run inserts nothing.
#
# WHY A SCRIPT: a report exists only once a run has registered, flipped, closed and
# estimated, which takes weeks; the browser and the agentic harness's read-only setup
# surface cannot make one. The rows below are what creating the experiment
# (experiments + its one design row in experiment_designs, m20261016_000001), registering it
# (experiment_runs, which freezes that design: frozen_at = the run's registered_at) and PR 5's estimate job (experiment_results,
# m20261012_000001) leave behind, written against those migrations' columns; ON_ERROR_STOP makes a mismatch
# fail loudly, not seed half a row. `experiment_results.compliance` is the stored
# ComplianceSummary (with n_treated / n_control); `effect` is a stand-in `{}` because the
# page renders `report` alone (the audit fields are never read by the UI).
#
#   OXY_DATABASE_URL=postgresql://postgres:postgres@localhost:15432/oxy \
#     bash scripts/seed-experiment-report.sh
set -euo pipefail

: "${OXY_DATABASE_URL:?set OXY_DATABASE_URL to the cloud backend Postgres URL}"
# `oxy seed`'s own fixed Demo workspace id (crates/app/src/cli/commands/seed.rs).
WORKSPACE_ID="${OXY_SEED_WORKSPACE_ID:-70787bb2-e11b-5488-b2c3-02e60d5fc7d3}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# The released report — the same bytes web-app's directionalFixture.test.tsx renders.
REPORT="$(cat "$ROOT/web-app/tests/agentic/fixtures/experiments/directional-switchback-result.json")"

# The pinned design (C3 `experiment_runs.config`): a 4-pair switchback at alpha 0.2.
read -r -d '' CONFIG <<'JSON' || true
{
  "experiment": "customer_orders_switchback",
  "hypothesis": "A customer-level promo lifts order value per customer-day",
  "owner": "hai@oxy.tech",
  "for_each": "customer",
  "lever": {
    "on": { "via": "fruit_summary" },
    "off": { "via": "fruit_summary" },
    "read_back": {
      "when_on": "{{orders.total_order_value}} > 0",
      "when_off": "{{orders.total_order_value}} >= 0",
      "grace_days": 2
    },
    "on_close": "off",
    "per": "wave"
  },
  "design": {
    "timezone": "UTC",
    "time_dimension": "orders.order_date",
    "switchback": {
      "period_days": 7,
      "periods": [
        { "from": "2026-03-02", "state": "on" },
        { "from": "2026-03-09", "state": "off" },
        { "from": "2026-03-16", "state": "off" },
        { "from": "2026-03-23", "state": "on" },
        { "from": "2026-03-30", "state": "on" },
        { "from": "2026-04-06", "state": "off" },
        { "from": "2026-04-13", "state": "off" },
        { "from": "2026-04-20", "state": "on" }
      ]
    },
    "seed": 8841207,
    "anticipation_days": 0,
    "washout_days": 2,
    "coverage_floor": 0.9,
    "compliance_floor": 0.8
  },
  "outcome": {
    "target": "orders.total_order_value",
    "driver": "orders.order_count",
    "family": 1,
    "direction": "increase",
    "alpha": 0.2,
    "power": 0.8,
    "mde_at_proposal": 8.1,
    "prior_coefficient": { "value": 1.7, "source": "fitted" }
  },
  "guardrails": []
}
JSON

psql "$OXY_DATABASE_URL" -v ON_ERROR_STOP=1 \
  -v ws="$WORKSPACE_ID" -v report="$REPORT" -v config="$CONFIG" <<'SQL'
WITH experiment AS (
  INSERT INTO experiments (workspace_id, name, hypothesis, owner, created_by)
  VALUES (:'ws'::uuid, 'customer_orders_switchback', (:'config'::jsonb) ->> 'hypothesis',
          (:'config'::jsonb) ->> 'owner', '00000000-0000-0000-0000-000000000000')
  ON CONFLICT (workspace_id, name) DO NOTHING
  RETURNING id
), design AS (
  INSERT INTO experiment_designs (experiment_id, config, content_hash, created_by, updated_by,
                                  created_at, updated_at, frozen_at)
  SELECT experiment.id, :'config'::jsonb, md5(:'config'), '00000000-0000-0000-0000-000000000000',
         '00000000-0000-0000-0000-000000000000', now() - interval '61 days',
         now() - interval '61 days', now() - interval '60 days'
  FROM experiment
  RETURNING id, experiment_id
), run AS (
  INSERT INTO experiment_runs
    (id, created_at, workspace_id, experiment_id, design_id, registered_by, registered_at,
     state, next_check_at, updated_at)
  SELECT gen_random_uuid(), now(), :'ws'::uuid, design.experiment_id, design.id,
         '00000000-0000-0000-0000-000000000000', now() - interval '60 days', 'reported',
         now(), now()
  FROM design
  RETURNING id
)
INSERT INTO experiment_results
  (id, created_at, run_id, effect, ratio, prediction, compliance, report, coefficient_patch,
   aborted)
SELECT gen_random_uuid(), now(), run.id, '{}'::jsonb, NULL,
       (:'report'::jsonb) -> 'prediction',
       '{"c_treated":1,"c_control_leak":0,"restated_mde":null,"below_floor":false,"n_treated":6,"n_control":12}'::jsonb,
       :'report'::jsonb, NULL, false
FROM run;
SQL

echo "seeded experiment customer_orders_switchback (a reported directional switchback)"
