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

# Cargo's resolution of the base revision needs that revision's manifests and
# lockfile, nothing else: a worktree, no build. The head's toolchain is forced so
# a toolchain bump in the PR does not make rustup install the base's, and local
# hooks are off so a post-checkout hook does not try to seed the worktree.
# No `--locked`: resolve the way the `cargo build` that follows will.
toolchain=$(cd "$root" && rustup show active-toolchain | cut -d' ' -f1)
git -C "$root" -c core.hooksPath=/dev/null worktree add --quiet --detach "$scratch/base" "$base_rev"
(cd "$scratch/base" && RUSTUP_TOOLCHAIN=$toolchain cargo metadata --format-version 1) \
  >"$scratch/old.json"
(cd "$root" && cargo metadata --format-version 1) >"$scratch/new.json"

# Tracked changes only: the merge-commit checkout in CI has no untracked files,
# and locally an untracked scratch file should not widen the run.
# `--no-renames`: with rename detection, `--name-only` prints only a moved
# file's new path, and the package that lost it (and whatever read it there)
# would never be marked.
git -C "$root" diff --name-only --no-renames -z "$base_rev" >"$scratch/changed"

cargo run --quiet --locked --manifest-path "$tool_dir/Cargo.toml" -- \
  --old-metadata "$scratch/old.json" \
  --new-metadata "$scratch/new.json" \
  --changed "$scratch/changed" \
  --config "$root/.config/test-selection.toml" \
  --root-manifest "$root/Cargo.toml" >"$out"
cat "$out"
