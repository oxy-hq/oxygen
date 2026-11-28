#!/usr/bin/env bash
# Usage: scripts/ci/test-selection/select.sh <base-rev> <out.json>
#
# Writes the test selection for the change from <base-rev> to the working tree
# (see src/lib.rs for what it means). Run from the repo root. CI passes the
# first parent of the PR merge commit; locally `origin/main` works the same way.
set -euo pipefail

base_rev=$1
out=$2
root=$(git rev-parse --show-toplevel)
tool_dir="$root/scripts/ci/test-selection"
scratch=$(mktemp -d)
cleanup() {
  git -C "$root" worktree remove --force "$scratch/base" 2>/dev/null || true
  rm -rf "$scratch"
}
trap cleanup EXIT

# Anything that goes wrong on the base side (it does not check out, its
# manifests do not load, the diff against it or the selector over it fails)
# leaves nothing to diff against. Test the whole workspace then: failing would
# also fail the PR that repairs a broken base (2026-11-28: a duplicate key on
# main failed every PR, #4569 included), and a partial answer could skip a test.
test_everything() {
  echo "::warning title=Test selection::$1; testing the whole workspace" >&2
  echo '{"mode":"all"}' >"$out"
  cat "$out"
  exit 0
}
# A failed command's `error:` / `fatal:` / `test-selection:` lines, joined, for the warning (cargo
# prints "duplicate key" and "failed to load manifest ..." as two of them).
# Without one, its last non-blank stderr line.
last_error() {
  local lines
  local plain
  plain=$(sed $'s/\x1b\\[[0-9;]*m//g' "$scratch/err" | tr -d '\r')
  lines=$(grep -E '^(error|fatal|test-selection)' <<<"$plain" || true)
  [[ -n $lines ]] || lines=$(grep -v '^[[:space:]]*$' <<<"$plain" | tail -n 1 || true)
  printf '%s' "$lines" | paste -sd ';' - | cut -c1-400
}

# The head's manifests are loaded first and on their own: a PR that breaks its
# own manifests fails here, as it should. Only base-side failures fall back.
(cd "$root" && cargo metadata --format-version 1) >"$scratch/new.json"

# Cargo's resolution of the base revision needs that revision's manifests and
# lockfile, nothing else: a worktree, no build. The head's toolchain is forced so
# a toolchain bump in the PR does not make rustup install the base's, and local
# hooks are off so a post-checkout hook does not try to seed the worktree.
# No `--locked`: resolve the way the `cargo build` that follows will.
toolchain=$(cd "$root" && rustup show active-toolchain | cut -d' ' -f1)
if ! git -C "$root" -c core.hooksPath=/dev/null worktree add --quiet --detach "$scratch/base" "$base_rev" \
  2>"$scratch/err"; then
  cat "$scratch/err" >&2
  test_everything "the base revision $base_rev does not check out ($(last_error))"
fi
if ! (cd "$scratch/base" && RUSTUP_TOOLCHAIN=$toolchain cargo metadata --format-version 1) \
  >"$scratch/old.json" 2>"$scratch/err"; then
  cat "$scratch/err" >&2
  test_everything "the base revision's manifests do not load ($(last_error))"
fi

# Tracked changes only: the merge-commit checkout in CI has no untracked files,
# and locally an untracked scratch file should not widen the run.
# `--no-renames`: with rename detection, `--name-only` prints only a moved
# file's new path, and the package that lost it (and whatever read it there)
# would never be marked.
if ! git -C "$root" diff --name-only --no-renames -z "$base_rev" >"$scratch/changed" 2>"$scratch/err"; then
  cat "$scratch/err" >&2
  test_everything "the diff against the base revision failed ($(last_error))"
fi

# Runs the selector with <old-metadata> as the base; succeeds only when it exits
# 0 and prints a selection the CI step knows how to run. `-s` so empty output is
# `[]`, not "no result" (which `-e` lets through).
run_selector() {
  cargo run --quiet --locked --manifest-path "$tool_dir/Cargo.toml" -- \
    --old-metadata "$1" \
    --new-metadata "$scratch/new.json" \
    --changed "$scratch/changed" \
    --config "$root/.config/test-selection.toml" \
    --root-manifest "$root/Cargo.toml" >"$scratch/selection.json" 2>"$scratch/err" || return 1
  jq -se 'length == 1 and (.[0] | .mode == "all" or .mode == "none"
      or (.mode == "packages" and (.packages | length) > 0))' \
    "$scratch/selection.json" >/dev/null 2>&1 || {
    echo "test-selection: printed no usable selection" >>"$scratch/err"
    return 1
  }
}

if ! run_selector "$scratch/old.json"; then
  cat "$scratch/err" >&2
  reason=$(last_error)
  # The selector also reads the head's config and root manifest, and is built
  # from the head's source. Run it again with the head as its own base: if that
  # fails too, the fault is the PR's own and the step fails. Only a failure the
  # base graph alone causes falls back.
  if ! run_selector "$scratch/new.json"; then
    cat "$scratch/err" >&2
    echo "::error title=Test selection::the selector fails on this PR's own inputs ($(last_error))" >&2
    exit 1
  fi
  test_everything "the selector failed on the base revision ($reason)"
fi
cat "$scratch/err" >&2
cp "$scratch/selection.json" "$out"
cat "$out"
