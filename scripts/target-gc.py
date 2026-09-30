#!/usr/bin/env python3
"""Report and reclaim the disk that per-worktree target/ dirs accumulate.

Cargo never deletes anything from target/ on its own. Two things grow without
bound when many agents work in parallel worktrees:

  1. target/<profile>/incremental/<crate>-<hash>/ -- rustc keeps one incremental
     cache per (crate, feature set, profile). When the hash changes (a feature
     flip, a toolchain bump, a profile edit) the old directory is orphaned and
     never read again, but it stays. Other repos have measured this reaching
     hundreds of GB (rjwalters/loom#8453: 213 GB across 6,402 session dirs).
  2. whole target/ dirs of worktrees nobody builds in any more -- the agent
     finished, the branch merged, the worktree was left behind.

Both are safe to delete: the worst case is a rebuild of what was removed.

    python3 scripts/target-gc.py                 # report only
    python3 scripts/target-gc.py --days 7        # show what gc would delete
    python3 scripts/target-gc.py --days 7 --apply
    just target-gc --days 7 --apply             # the same, via just

Sizes are measured with one `du` over every worktree at once, so hardlinked
artifacts (scripts/seed-target.py) are counted once, not once per worktree.
The main checkout's target/ is never removed wholesale, only its stale
incremental dirs. See internal-docs/rust-build-performance.md.
"""

import argparse
import os
import shutil
import subprocess
import sys
import time

def worktrees(root):
    out = subprocess.run(
        ["git", "-C", root, "worktree", "list", "--porcelain"],
        check=True, capture_output=True, text=True,
    ).stdout
    trees, cur = [], {}
    for line in out.splitlines() + [""]:
        if not line:
            if cur:
                trees.append(cur)
            cur = {}
            continue
        key, _, val = line.partition(" ")
        cur[key] = val or True
    return trees


def du_kib(paths):
    """Unique KiB across `paths` (hardlinks counted once), and per path."""
    paths = [p for p in paths if os.path.exists(p)]
    if not paths:
        return 0, {}
    out = subprocess.run(
        ["du", "-s", "-k", "-c", "--", *paths], capture_output=True, text=True
    ).stdout.splitlines()
    per = {}
    total = 0
    for line in out:
        size, _, path = line.partition("\t")
        if path == "total":
            total = int(size)
        else:
            per[path] = int(size)
    return total, per


def newest_mtime(path):
    newest = 0.0
    for dirpath, _dirs, files in os.walk(path):
        for name in files:
            try:
                newest = max(newest, os.lstat(os.path.join(dirpath, name)).st_mtime)
            except OSError:
                pass
    return newest or os.lstat(path).st_mtime


def nested_roots(target, leaf):
    """Every `<profile>/<leaf>` under target/ (`incremental`, `.fingerprint`).

    Not just debug/ and release/: rust-analyzer builds into
    target/rust-analyzer/debug/ (.vscode/settings.json), and custom profiles or
    `--target <triple>` builds nest the same way.
    """
    for dirpath, dirs, _files in os.walk(target):
        if os.path.basename(dirpath) == leaf:
            dirs[:] = []
            yield dirpath
            continue
        # Artifact dirs hold thousands of files and never a root.
        dirs[:] = [d for d in dirs if d == leaf or d not in ("deps", "build", ".fingerprint", "incremental", "examples")]


def last_build(target):
    """Newest fingerprint write under target/, rust-analyzer's included; None if never built."""
    stamps = [newest_mtime(fp) for fp in nested_roots(target, ".fingerprint")]
    return max(stamps, default=None)


def stale_incremental(target, cutoff):
    """Incremental dirs not written since `cutoff`, never the newest per crate."""
    stale = []
    for inc in nested_roots(target, "incremental"):
        by_crate = {}
        for name in os.listdir(inc):
            path = os.path.join(inc, name)
            if os.path.isdir(path):
                crate = name.rsplit("-", 1)[0]
                by_crate.setdefault(crate, []).append((newest_mtime(path), path))
        for dirs in by_crate.values():
            dirs.sort(reverse=True)
            stale += [p for m, p in dirs[1:] if m < cutoff]
    return stale


def gib(kib):
    return f"{kib / 1024 / 1024:6.1f} GiB"


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--days", type=float, help="gc anything untouched this long")
    ap.add_argument("--apply", action="store_true", help="delete (default: dry run)")
    args = ap.parse_args()

    root = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], check=True, capture_output=True, text=True
    ).stdout.strip()
    trees = worktrees(root)
    main_tree = trees[0]["worktree"]
    targets = [os.path.join(t["worktree"], "target") for t in trees]

    total, per = du_kib(targets)
    print(f"{'target/ size':>12}  {'last build':>10}  worktree")
    for t, target in zip(trees, targets):
        if target not in per:
            continue
        built = last_build(target)
        age = (time.time() - built) / 86400 if built is not None else float("nan")
        branch = str(t.get("branch", "(detached)")).removeprefix("refs/heads/")
        print(f"{gib(per[target])}  {age:8.1f}d  {t['worktree']}  [{branch}]")
    print(f"{gib(total)}  unique across all worktrees (hardlinks counted once)")

    if args.days is None:
        return 0
    cutoff = time.time() - args.days * 86400
    doomed = []
    for t, target in zip(trees, targets):
        if not os.path.isdir(target):
            continue
        # Idle only if NO profile under it was built lately: a worktree kept open
        # in the editor has a fresh target/rust-analyzer/ even when cargo is cold.
        built = last_build(target)
        idle = built is not None and built < cutoff
        if idle and t["worktree"] != main_tree:
            doomed.append(target)
        else:
            doomed += stale_incremental(target, cutoff)

    freed, _ = du_kib(doomed)
    verb = "deleting" if args.apply else "would delete"
    for path in doomed:
        print(f"{verb} {path}")
        if args.apply:
            shutil.rmtree(path, ignore_errors=True)
    print(f"{verb} {len(doomed)} dirs, ~{gib(freed).strip()} (less if hardlinked elsewhere)")
    if doomed and not args.apply:
        print("re-run with --apply to delete")
    return 0


if __name__ == "__main__":
    sys.exit(main())
