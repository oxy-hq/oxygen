#!/usr/bin/env bash
# Create every experiment example in a workspace, price the pre-registered
# drafts, and (with --run-past) analyse the past-data ones.
#
#   bash examples/experiments/load_examples.sh --env local --workspace <id>
#   bash examples/experiments/load_examples.sh --target http://127.0.0.1:3000 --workspace <id> --run-past
#
# Every argument but --run-past is passed to each `oxyc` call. The workspace
# must hold this repo's examples/ (the exp_* views, the levers under
# levers/, and the data from scripts/gen_experiments_data.py), e.g. one
# seeded with `oxy seed --workspace-path ./examples`.
#
# Creating and pricing only write drafts and proposals. --run-past starts
# each past-data example's retrospective run, the experiment's only run:
# irreversible, so it is opt-in. Nothing here registers a design.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
RUN_PAST=0
ARGS=()
for a in "$@"; do
  if [ "$a" = "--run-past" ]; then RUN_PAST=1; else ARGS+=("$a"); fi
done
OXYC="${OXYC:-oxyc}"
ox() { "$OXYC" experiments "$@" "${ARGS[@]}"; }

# A Monday about two weeks out: every candidate below starts there, which
# is also a period start at week grain.
FIRST="$(python3 -c 'import datetime as d; t=d.date.today()+d.timedelta(14); print(t+d.timedelta((7-t.weekday())%7))')"

for f in "$HERE"/customer_orders_pilot.experiment.yml "$HERE"/past_data/*.experiment.yml "$HERE"/pre_registered/*.experiment.yml; do
  ox create --file "$f" || echo "skipped $(basename "$f") (it may exist already)"
done

price() { name=$1; shift; echo "== pricing $name"; ox price "$name" "$@" --wait || true; }
price self_checkout_rollout \
  --candidate "{shape: waves, wave_sizes: [8, 8, 8], first_switch_on: $FIRST, spacing_days: 14, blocks: 4}" \
  --candidate "{shape: waves, wave_sizes: [24], first_switch_on: $FIRST}" \
  --candidate "{shape: waves, wave_sizes: [], pairs: true, first_switch_on: $FIRST}"
price double_points_switchback_plan \
  --candidate "{shape: switchback, first_period_on: $FIRST, period_days: 7, pairs: 6}" \
  --candidate "{shape: switchback, first_period_on: $FIRST, period_days: 7, pairs: 6, alpha: 0.2}"
price weekly_store_promo \
  --candidate "{shape: waves, wave_sizes: [16], first_switch_on: $FIRST, blocks: 4}"
price hourly_kiosk_prompt \
  --candidate "{shape: switchback, first_period_on: $FIRST, period_days: 12, pairs: 14}"
price brand_promo_groups \
  --candidate "{shape: waves, wave_sizes: [9], first_switch_on: $FIRST, assign_by: exp_skus.brand, blocks: 3}" \
  --candidate "{shape: waves, wave_sizes: [45], first_switch_on: $FIRST}"
price kit_deal_one_date_plan \
  --candidate "{shape: waves, wave_sizes: [5], first_switch_on: $FIRST}"

if [ "$RUN_PAST" = 1 ]; then
  for f in "$HERE"/past_data/*.experiment.yml; do
    ox retrospective "$(basename "$f" .experiment.yml)" --yes || true
  done
  echo "reports land within a few minutes: oxyc experiments result <name>"
fi
