# Experiments docs charts

Draws the SVG charts in `docs/images/experiments/` from the simulated example
data in `oxy-hq/hai-test-examples` (branch `ads-campaign-experiment`), so the
numbers in `docs/build/world-model/experiments/` can be re-derived. Pure
Python, no dependencies.

```sh
cd hai-test-examples && python3 scripts/gen_experiment_data.py && python3 scripts/gen_ads_data.py
cd ../oxygen-internal
EXAMPLES_DB=../hai-test-examples/.db python3 scripts/experiment-docs-charts/make.py
python3 scripts/experiment-docs-charts/anatomy.py
EXAMPLES_DB=../hai-test-examples/.db python3 scripts/experiment-docs-charts/grain_interim.py
```

`make.py` prints the numbers the worked examples quote. The comparisons are a
reimplementation of the report's (per-unit after-minus-before, Welch, and the
staggered permutation test with 2,000 seeded shuffles), so a live report may
differ in the last decimal place. `placebo-power.svg` uses a simulated panel
shaped like the shopper data, not the example data itself.

`grain_interim.py` draws the grain and interim charts and prints their numbers:
the staggered promo's wave 1 read per day and per week (no washout, 28 days
either side), its interim reading on each of 42 days (the trimmed design the
interim reads, compared by 500 seeded reshuffles; the shaded range is ±1.96 ×
the reshuffles' spread, an approximation of the report's test-inverted
interval), and how often a no-effect panel shaped like the shoppers crosses
α = 0.05 on at least one day when read daily (1,000 simulated runs, Welch).
