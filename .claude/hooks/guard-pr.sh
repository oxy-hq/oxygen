#!/usr/bin/env bash
# .claude/hooks/guard-pr.sh — PreToolUse guard for the `roadmap` merge status
# (internal-docs/code-review.md, Merge signals). A PR created from a checkout of this
# repo, through the GitHub MCP tool or `gh pr create`, gets its title and body run through
# `scripts/roadmap/roadmap.ts check` first, against the checkout the command runs in; the
# status's own verdict refuses the creation until the body carries the trailer the branch
# owes. Writing it before the PR opens is one line; fixing it after the status fails is a
# second push.
#
# Silence = allow. Fails open when the body cannot be read from the text (`--fill`, a
# variable, a form scripts/roadmap/pr-command.ts does not read), when the checkout or its
# base branch cannot be found, or when node/jq are missing: this is the status brought
# forward, not a boundary; the status still runs on the PR.
set -uo pipefail

command -v jq >/dev/null 2>&1 || exit 0
command -v node >/dev/null 2>&1 || exit 0
input=$(cat 2>/dev/null) || exit 0
tool=$(jq -r '.tool_name // empty' <<<"$input") || exit 0
here=$(cd "$(dirname "$0")/../.." 2>/dev/null && pwd) || exit 0
reader="$here/scripts/roadmap/pr-command.ts"

deny() {
  jq -nc --arg r "$1" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}'
  exit 0
}

# The checkout the PR is opened from: the command's own `cd`, else the tool's cwd, else
# the session's project. A wt-claim slot is a different checkout from the session root.
checkout_from() {
  local dir=$1
  [ -n "$dir" ] && git -C "$dir" rev-parse --show-toplevel 2>/dev/null
}

title="" body="" base="main" repo="" root=""
case "$tool" in
mcp__github__create_pull_request)
  repo=$(jq -r '"\(.tool_input.owner // "")/\(.tool_input.repo // "")"' <<<"$input")
  [ "$repo" = "/" ] && repo=""
  title=$(jq -r '.tool_input.title // empty' <<<"$input")
  body=$(jq -r '.tool_input.body // empty' <<<"$input")
  base=$(jq -r '.tool_input.base // "main"' <<<"$input")
  root=$(checkout_from "$(jq -r '.cwd // empty' <<<"$input")")
  ;;
Bash)
  cmd=$(jq -r '.tool_input.command // empty' <<<"$input")
  [[ $cmd == *"gh pr create"* ]] || exit 0
  [ -f "$reader" ] || exit 0
  parsed=$(printf '%s' "$cmd" | node "$reader" 2>/dev/null) || exit 0
  [ "$(jq -r '.found' <<<"$parsed")" = "true" ] || exit 0
  [ "$(jq -r '.dynamic' <<<"$parsed")" = "false" ] || exit 0
  # A `cd` the text does not settle (`cd "$(just wt-claim x)"`) means the checkout is
  # unknown: judging the one the shell started in would be the wrong branch's verdict.
  [ "$(jq -r '.cwdUnresolved' <<<"$parsed")" = "false" ] || exit 0
  title=$(jq -r '.title // empty' <<<"$parsed")
  base=$(jq -r '.base // "main"' <<<"$parsed")
  repo=$(jq -r '.repo // empty' <<<"$parsed")
  # The command's own `cd`, relative to where the tool runs it, else that directory.
  start=$(jq -r '.cwd // empty' <<<"$input")
  cwd=$(jq -r '.cwd // empty' <<<"$parsed")
  if [ -n "$cwd" ]; then
    cwd=$(cd "${start:-.}" 2>/dev/null && cd "$cwd" 2>/dev/null && pwd) || exit 0
  else
    cwd=$start
  fi
  root=$(checkout_from "$cwd")
  file=$(jq -r '.bodyFile // empty' <<<"$parsed")
  if [ -n "$file" ]; then
    [[ $file = /* ]] || file="${cwd:-.}/$file"
    [ -f "$file" ] || exit 0
    body=$(cat "$file")
  else
    body=$(jq -r '.body // empty' <<<"$parsed")
  fi
  ;;
*) exit 0 ;;
esac

[ -n "$root" ] || root=$(checkout_from "${CLAUDE_PROJECT_DIR:-}")
[ -n "$root" ] || exit 0
# Only this repository's PRs: a `--repo` naming another, or a checkout of another, is not ours.
case "$repo" in "" | oxy-hq/oxygen-internal) ;; *) exit 0 ;; esac
git -C "$root" remote get-url origin 2>/dev/null |
  grep -Eq '^(https://github\.com/|git@github\.com:|ssh://git@github\.com/)oxy-hq/oxygen-internal(\.git)?/?$' || exit 0
git -C "$root" rev-parse --verify -q "origin/$base" >/dev/null 2>&1 || exit 0

# The checkout named by the command is data to judge, never code to run: the check is
# this repo's own copy of the script, pointed at that checkout with --dir.
checker="$here/scripts/roadmap/roadmap.ts"
[ -f "$checker" ] || exit 0
tmp=$(mktemp "${TMPDIR:-/tmp}/guard-pr.XXXXXX") || exit 0
trap 'rm -f "$tmp"' EXIT
printf '%s\n' "$body" >"$tmp"
out=$(node "$checker" check --dir "$root" --body-file "$tmp" --title "$title" --base "origin/$base" 2>&1)
status=$?
# Only the status's own verdict refuses; a crash (no merge base, a parse error) allows.
grep -q '^roadmap: ' <<<"$out" || exit 0
[ "$status" -eq 0 ] && exit 0
deny "The roadmap status would fail this PR as written. $(grep '^roadmap: ' <<<"$out" | head -1 | sed 's/^roadmap: //'). Draft the trailer before opening it: node scripts/roadmap/roadmap.ts draft --title \"<PR title>\" (skill: oxy-roadmap), then end the body with Roadmap: <line-id> or Roadmap: none -- <why>."
