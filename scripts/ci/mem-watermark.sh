#!/usr/bin/env bash
# Print the runner's memory high-water mark (RAM + swap in use) each time it
# rises by STEP MiB, so a job the runner kills mid-build still leaves its last
# reading in the log. Run it in the background of the step you want covered;
# the step's own output is the only place the readings land.
#
#   scripts/ci/mem-watermark.sh [STEP_MIB=512] [INTERVAL_S=10] &
#
# With MEM_WATERMARK_FILE set, the latest high-water mark is also kept in that
# file, one line, for the job summary to quote after the step.
set -u

step=${1:-512}
interval=${2:-10}
peak=0

while sleep "$interval"; do
  read -r total avail swap_total swap_free < <(awk '
    /^MemTotal:/     { t = $2 }
    /^MemAvailable:/ { a = $2 }
    /^SwapTotal:/    { st = $2 }
    /^SwapFree:/     { sf = $2 }
    END { print t, a, st, sf }' /proc/meminfo)
  ram=$(((total - avail) / 1024))
  swap=$(((swap_total - swap_free) / 1024))
  used=$((ram + swap))
  if ((used >= peak + step)); then
    peak=$used
    echo "mem-watermark: ${used} MiB in use (RAM ${ram}/$((total / 1024)) MiB, swap ${swap}/$((swap_total / 1024)) MiB)"
    if [[ -n "${MEM_WATERMARK_FILE:-}" ]]; then
      awk -v u="$used" -v r="$ram" -v rt="$((total / 1024))" -v s="$swap" -v st="$((swap_total / 1024))" \
        'BEGIN { printf "%.1f GiB (RAM %.1f of %.1f GiB, swap %.1f of %.1f GiB)\n", u/1024, r/1024, rt/1024, s/1024, st/1024 }' \
        >"$MEM_WATERMARK_FILE"
    fi
  fi
done
