#!/usr/bin/env bash
# Collect the four sealed results of one ramen-shop evaluation.
#   WORKSPACE_ID=<uuid> RUN_STAGGERED=<name> RUN_GROUPED=<name> RUN_SWITCHBACK=<name> RUN_NULL=<name> \
#     scripts/eval/ramen_shop/collect.sh <env> <out_dir>
# `oxyc api` exits non-zero on the 409 a run that is not `reported` answers, and `set -e`
# stops here: this script never judges a run that is still sealed. An evaluation, not CI.
# Runbook: internal-docs/2026-09-30-experiments-pr5-results-plan.md, Task 11 Step 7.
set -euo pipefail
: "${WORKSPACE_ID:?set WORKSPACE_ID}"
env="${1:?usage: collect.sh <env> <out_dir>}"
out="${2:?usage: collect.sh <env> <out_dir>}"
mkdir -p "$out"
for label in staggered grouped switchback null; do
  var="RUN_$(printf '%s' "$label" | tr '[:lower:]' '[:upper:]')"
  name="${!var:?set $var to the experiment name}"
  # Write to a temp file and move it into place: a failed fetch leaves no partial or stale file.
  tmp="$(mktemp "${out}/.${label}.XXXXXX")"
  trap 'rm -f "$tmp"' EXIT
  oxyc api "/api/${WORKSPACE_ID}/experiments/${name}/result" --env "$env" > "$tmp"
  mv "$tmp" "${out}/${label}.json"
done
echo "collected into ${out}; add truth.json, then: cargo run -p oxy-experiment --example ramen_shop_judge -- ${out}"
