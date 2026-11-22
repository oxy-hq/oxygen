#!/usr/bin/env python3
"""PostToolUse hook: tell the agent when its edit made a function too complex.

Nothing in an agent's loop says "that function is now too tangled to maintain".
This closes the loop at the moment the edit lands, while the author still has
the context: it runs the same gate as `just metrics` on the one file that
changed and hands the offenders back as advisory context.

Advisory only. It never blocks an edit and never fails a tool call: every
failure path here exits 0 silently, because a measurement tool that can break
editing is worse than no measurement tool.

Thresholds, exclusions and the baseline all come from bca.toml, so this agrees
with `just metrics` and with CI by construction. The baseline means an edit to
an already-complex function says nothing unless the edit made it *worse*.
"""

import json
import os
import shutil
import subprocess
import sys
import tomllib

MEASURED = {".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".kt", ".swift", ".sh"}
TIMEOUT = 20

GUIDANCE = (
    "Simplify the code, do not game the number: extracting a block into a "
    "helper that is called once, or splitting at an arbitrary line, moves the "
    "metric without making anything easier to read. Prefer removing a "
    "condition, inverting a guard, replacing a nested match with an early "
    "return, or pulling out a named concept that stands on its own. If the "
    "complexity is irreducible and the shape is right, say so and leave it — "
    "`bca: suppress` with a reason is the honest escape hatch."
)


def _root() -> str:
    """This repo, located from this file — never from the event.

    The hook lives at <root>/.claude/hooks/, so its own path is the one anchor
    the session cannot move. Taking the root from the event's `cwd` instead
    would mean resolving `.tools/bin/bca` relative to wherever the session
    happens to be sitting, and running whatever is there: a `.tools/bin/bca`
    inside a cloned repo or an unpacked archive would execute on the next edit.
    The same mistake also breaks correctness quietly — `bca` discovers
    `bca.toml` by climbing from its working directory, so a wrong root measures
    against the wrong thresholds, or none.
    """
    return os.path.realpath(os.path.join(os.path.dirname(__file__), "..", ".."))


def _inside(root: str, path: str) -> bool:
    try:
        return os.path.commonpath([root, path]) == root
    except ValueError:  # different drives
        return False


def _measured(root: str) -> list[str]:
    """The trees bca.toml actually scans, read from it rather than hardcoded.

    Naming a file on the command line bypasses `paths`, and the baseline only
    holds entries for files inside them. So without this check an edit to, say,
    `.github/scripts/deploy-metrics.ts` — 1,600 lines, never scanned, nothing
    baselined — would come back with every pre-existing offender in it,
    announced as "new or worsened". That is false, and it would push an agent
    into restructuring a file it had merely touched: the exact churn the
    guidance below tells it not to do.

    Unreadable manifest means an empty list means silence. An advisory tool
    that cannot tell what is in scope should say nothing.
    """
    try:
        with open(os.path.join(root, "bca.toml"), "rb") as handle:
            paths = tomllib.load(handle).get("paths") or []
    except Exception:
        return []
    return [
        os.path.realpath(os.path.join(root, entry))
        for entry in paths
        if isinstance(entry, str)
    ]


def main() -> int:
    try:
        event = json.load(sys.stdin)
    except Exception:
        return 0

    if event.get("hook_event_name") != "PostToolUse":
        return 0
    if event.get("tool_name") not in ("Edit", "Write"):
        return 0

    path = (event.get("tool_input") or {}).get("file_path")
    if not path or not os.path.isabs(path):
        return 0
    root = _root()
    path = os.path.realpath(path)
    if not os.path.isfile(path) or not _inside(root, path):
        return 0
    if not any(_inside(tree, path) for tree in _measured(root)):
        return 0
    if os.path.splitext(path)[1] not in MEASURED:
        return 0

    # Never install from a hook: a network fetch in the edit loop is not worth
    # the latency, and an agent that has not run `just metrics` once simply gets
    # no feedback. The repo-local copy first; PATH second, which is the user's
    # own shell configuration rather than anything this event can influence.
    local = os.path.join(root, ".tools", "bin", "bca")
    bca = local if os.access(local, os.X_OK) else shutil.which("bca")
    if not bca:
        return 0

    try:
        done = subprocess.run(
            [bca, "check", "--no-summary", "--no-remediation", path],
            capture_output=True,
            text=True,
            timeout=TIMEOUT,
            cwd=root,
        )
    except Exception:
        return 0

    # bca's contract: 0 clean, 1 tool error, 2-5 threshold findings. A tool
    # error is ours to swallow — the agent did nothing wrong.
    if done.returncode not in (2, 3, 4, 5):
        return 0

    rows = [
        line
        for line in done.stdout.splitlines()
        if line.strip() and not line.startswith("bca:")
    ]
    if not rows:
        return 0

    report = "\n".join(rows[:20])
    text = (
        f"`bca check` on the file you just edited reports complexity over this "
        f"repo's limits (bca.toml; the existing offenders in "
        f".bca-baseline.toml are filtered out, so these are new or worsened):\n\n"
        f"{report}\n\n{GUIDANCE}"
    )
    json.dump(
        {
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": text,
            }
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        # Belt and braces: this hook may never be the reason an edit fails.
        sys.exit(0)
