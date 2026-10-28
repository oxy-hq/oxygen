---
name: accept-agentic-healing
description: Promote staged Tier-2 healing recordings for a flow — wraps `pnpm test:agentic --accept-healing <flow>` and shows the cache diff
activeForm: Promoting staged Tier-2 healing recordings
argument-hint: "<flow-stem>"
allowed-tools:
  - Bash
  - Read
---

# Accept staged healing recordings for a flow

Thin wrapper around the runner's `--accept-healing` subcommand. Promotes
staged Tier-2 healing recordings from `.cache/healing-staging.json` into
the main `.cache/bespoke-actions.json`, shows the resulting diff, and
reminds the dev to commit it.

`$ARGUMENTS` is the flow stem (e.g. `chat-ask`, `ide-save`).

**Use this only when `/fix-agentic-test` classified the failure as
Tier-2 (staged heal).** Running `--accept-healing` against an empty
staging file is a no-op; running it against a staged recording the dev
hasn't reviewed is a foot-gun (the new selectors become the test's
ground truth without human review).

This command must be run from the root of an `oxy-hq/oxygen-internal`
checkout.

## Steps

### 1. Sanity-check

```bash
test -f web-app/tests/agentic/README.md || { echo "Not in an oxy-hq/oxygen-internal checkout"; exit 2; }
```

### 2. Confirm there's something to promote

```bash
STAGING=web-app/tests/agentic/.cache/healing-staging.json
test -s "$STAGING" || {
  echo "No staged healing entries at $STAGING. Nothing to promote."
  echo "If you expected staged entries, the previous run probably didn't trigger a Tier-2 heal — re-run the flow with /run-agentic-tests first."
  exit 0
}
```

### 3. Show what's about to be promoted

```bash
echo "=== Staged healing entries ==="
cat "$STAGING" | jq '.entries[] | {flow, case, step_index, staged_at, action_count: (.actions | length)}'
```

If the staged set targets a different flow than `$ARGUMENTS`, surface
that. The `--accept-healing <flow>` filter only promotes entries whose
`flow` field matches; others stay staged.

### 4. (Optional) Open the trace for visual confirmation

If the dev wants to confirm the new selectors hit the right element, the
trace from the redrive run is at:

```bash
ls -t web-app/tests/agentic/.traces/<flow-stem>-*.zip 2>/dev/null | head -1
```

```bash
pnpm exec playwright show-trace web-app/tests/agentic/.traces/<flow>-<case>.zip
```

Frame-by-frame DOM, network, screenshots — same debugging tools as a
hand-written Playwright suite.

### 5. Promote

```bash
cd web-app
pnpm test:agentic --accept-healing $ARGUMENTS
```

The runner's `promoteStaging()` moves each staged entry into the main
cache (keyed by its `cache_key`), then drops the promoted entries from
staging. If multiple flows are staged and only one matches the filter,
only that flow's entries move.

### 6. Show the cache diff and remind the dev to commit

```bash
git diff web-app/tests/agentic/.cache/bespoke-actions.json | head -100
```

If the diff is long, suggest `git diff --stat` for a summary.

**Tell the dev:**

> The new selectors are now the test's ground truth. Review the diff
> above — confirm the `selector_strategies[]` for each updated action
> point at the elements you'd expect for the new product behaviour. When
> it looks right, commit:
>
> ```bash
> git add web-app/tests/agentic/.cache/bespoke-actions.json
> git commit -m "chore: agentic — accept Tier-2 healing for <flow-stem>"
> ```
>
> The next CI run will replay against the promoted recording. Per-step
> cost drops back to the warm-replay floor (~$0.002).

## Error handling

- **`no staged healing entries`** (runner output). The filter matched
  nothing in `healing-staging.json`. Possible reasons:
  1. The staging file is empty — no Tier-2 heal happened. The failure
     is probably bucket A (Tier-1 silent re-rank) or bucket C
     (behavioral). Run `/fix-agentic-test <flow>` for classification.
  2. The filter doesn't match. Try without the filter to see what's
     staged for which flow.
- **Promotion runs but `git diff` is empty.** The cache file existed
  before promotion but with the same content. Either the promotion
  overwrote entries with byte-identical recordings (rare), or the diff
  is in another file (`healing-staging.json` would now be smaller — that
  diff is the operational trail, not the substantive change).
- **`bespoke-actions.json` is gitignored.** Check `.gitignore`. The
  current convention commits the cache file so CI restores share local
  baselines; if your local copy diverged, the next CI run will record
  cold from the shared baseline. If gitignored, the cache lives only in
  the CI cache layer (`actions/cache`); promotion is then only relevant
  for the next local run.

## Notes

- **Don't promote without reviewing the diff.** The Tier-2 staging
  mechanism exists specifically so a human reads the new selectors
  before they become ground truth. Skipping the review is the foot-gun
  the staging file was designed to prevent.
- **Promotion is per-flow.** `--accept-healing <flow>` filters by flow
  name. To promote everything staged, run without the filter — but
  that's almost never what you want; usually only one flow's drift
  matters.
- The staging file's entries are appended on each Tier-2 redrive — if
  the redrive happens during multiple consecutive runs (e.g. the new
  recording itself drifts before promotion), multiple entries may stack
  up. The latest entry per `cache_key` is what gets promoted; earlier
  ones are dropped from staging at promotion time.
- The cache-invalidation taxonomy + v2 durability mechanism design
  rationale both live in `internal-docs/agentic-browser-testing-spec.md`
  (Tier-2 is row `C2` in the cache-invalidation taxonomy section).
