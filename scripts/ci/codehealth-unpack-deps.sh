#!/usr/bin/env bash
# Unpack the private cargo dependencies the weekly-code-health `deps` job packed.
#
# Two independent guards, because this tar is unpacked into $HOME in a job that
# either runs the agent's code or runs after a job that did, and `~/.cargo/bin`
# is first on PATH:
#
#   1. The digest. `deps` records it as a job output the moment it packs, and a
#      job output is fixed once its step ends. Agent-written test code can reach
#      the run-scoped ACTIONS_RUNTIME_TOKEN out of the runner's process memory,
#      so "this is the artifact our own job produced" is an assumption, not a
#      fact, until it is checked.
#   2. The member list. Only `.cargo/registry` and `.cargo/git` are extracted,
#      by name. So even a tar that somehow passed the first guard cannot plant
#      `~/.cargo/bin/git` or `~/.cargo/bin/python3` — which the next steps in
#      `verify` would then run, before any of its own agent code, as the metric
#      gate and the test derivation.
#
# Either guard alone would do. Both, because the first is a check and the second
# is a limit, and a limit does not depend on getting the check right.
set -euo pipefail

TAR=${TAR:-cargo-deps.tar}

if [ -z "${TAR_SHA256:-}" ]; then
  echo "::error::no digest was recorded for $TAR; refusing to unpack it"
  exit 1
fi
if [ ! -f "$TAR" ]; then
  echo "::error::$TAR is missing"
  exit 1
fi

echo "$TAR_SHA256  $TAR" | sha256sum -c -

# `-C "$HOME"` with the two members named. GNU tar already strips a leading `/`
# and refuses a `..` component, and naming the members makes that moot anyway.
# `--no-same-owner` because nothing here should carry ownership from the job
# that packed it.
tar -xf "$TAR" -C "$HOME" --no-same-owner .cargo/registry .cargo/git
rm -f "$TAR"

# Fail loudly rather than letting cargo discover it: without this the first
# cargo invocation reports a network error while offline, which reads as a
# toolchain problem rather than a missing dependency set.
shopt -s nullglob
dbs=("$HOME"/.cargo/git/db/*)
if [ ${#dbs[@]} -eq 0 ]; then
  echo "::error::unpacked, but no private git database is present"
  exit 1
fi
echo "unpacked ${#dbs[@]} private git database(s)"
