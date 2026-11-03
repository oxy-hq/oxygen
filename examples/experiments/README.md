# Experiment examples

One example per way the platform reads an experiment, each backed by data in
which the effect is planted at a known size, so every report can be checked
against the truth.

An experiment is a record on the deployment, not a compiled file, so these
`.experiment.yml` files are the bodies you create drafts from
(`oxyc experiments create --file …`). What they point at is compiled with the
workspace like anything else:

| Piece | Where |
| --- | --- |
| Data | `examples/.db/exp_*.csv`, written by [`scripts/gen_experiments_data.py`](../scripts/gen_experiments_data.py) (deterministic) |
| Views | `examples/semantics/views/exp_*.view.yml`; entities `exp_store` (48 stores) and `exp_sku` (90 SKUs) |
| Levers | [`levers/`](levers/): one automation per lever shape (`per: unit`, `per: wave`, grouped). They only record the call, so a registered run reports every flip unconfirmed: swap in a real lever to run one |

## Load them

Against a workspace that holds this `examples/` folder (for a local stack,
`oxy seed --workspace-path ./examples`):

```bash
bash examples/experiments/load_examples.sh --env local --workspace <workspace-id>
```

That creates every draft and prices each pre-registered one. Add `--run-past`
to also start the past-data analyses; each is its experiment's only run and
cannot be undone, so it is opt-in. Reports land within a few minutes:

```bash
oxyc experiments result kit_deal_synthetic_control --env local --workspace <workspace-id>
```

## Past data (`past_data/`)

Each is analysed with **Analyse past data** (`oxyc experiments retrospective`).
All but the switchback declare `design.assignment: observed`: the change was
chosen, not drawn, so the report reads *observational*. Every report is
labelled *RETROSPECTIVE — not pre-registered*.

| File | Shows | Planted truth | Measured on this data |
| --- | --- | --- | --- |
| `kit_deal_welch` | One switch date, Welch (the default) | +32 units/SKU-day on 3 kits | +33.9 ± 7.5 |
| `kit_deal_permutation` | One switch date, `one_date_test: permutation` | same | +33.9 ± 5.4 |
| `kit_deal_synthetic_control` | `comparison: synthetic_control`, `exclude` | same | +34.8 ± 7.0; pre-fit 0.48 to 0.73 |
| `kit_deal_proportional` | `outcome.scale: proportional` (% lift per unit) | +40% | +46% (+38% to +55%) |
| `search_badge_conversion` | Ratio outcome, `outcome.denominator` | conversion +0.009 | +0.0075 (+0.004 to +0.011) |
| `spring_sale_carryover` | `switch_off` + `carryover_days` | +261 $/store-day, then −157 | +261 ± 89, then pull-forward −160 (−232 to −88) |
| `self_checkout_staggered` | Three switch dates (permutation), per-wave effects, substitution check | +173 $/store-day | +159 ± 40 |
| `double_points_switchback` | Switchback with periods and `seed`, directional α = 0.2 | +208 $/store-day | directional, +217 ± 67 |

Read the three `kit_deal_*` reads side by side: the same deal, three
comparisons. Welch learns the treated spread from three SKUs; permutation reads
them against every untreated SKU; the synthetic control builds each kit its own
donor mix, keeping out the two cannibalised siblings, and adds the chart of
each kit against its synthetic twin.

## Pre-registered (`pre_registered/`)

Drafts with no waves, seed or MDE: pricing writes those. Price the candidates
in `load_examples.sh` (or in the page's **Price** form), compare them on
**mde/unit-day**, `use` one, then `register` before its first date. Each was
checked to price and register.

| File | Shows | What pricing says |
| --- | --- | --- |
| `self_checkout_rollout` | Staggered waves with strata, one date, matched pairs; driver, substitution check, two guardrails | MDE about 2% for each; warns that the trigger guardrail sets one bar for stores of different sizes |
| `double_points_switchback_plan` | Fleet switchback, `lever.per: wave`, at α 0.05 and the directional 0.2 | the switchback's assumptions, and the read-back's arrival lag against `grace_days` |
| `weekly_store_promo` | `design.grain: week` (counts in weeks, Monday dates) | a non-Monday first date is refused and the next Monday named |
| `hourly_kiosk_prompt` | `design.grain: hour`, 12-hour switchback periods, `grace_days: 0` | MDE per store-hour; hourly revenue is noisy |
| `brand_promo_groups` | `assign_by: exp_skus.brand` beside the per-SKU design | the grouped MDE is per brand-day, divided down to per SKU-day to compare |
| `kit_deal_one_date_plan` | One switch date with `one_date_test: permutation`, drawn at random | priced through the staggered placebo |

`customer_orders_pilot.experiment.yml` is the fruit-orders draft the
`experiments-price-draft` browser flow types into the New experiment form; keep
it in sync with that flow.

## Changing the data

Edit the constants at the top of `scripts/gen_experiments_data.py`, rerun it,
and update the truth columns above and the *Planted truth* line in the
affected files. The generator's random stream depends on every draw, so any
change redraws all of the data: rerun every past-data example and refresh the
*Measured* column, not just the one you changed.
