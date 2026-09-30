#!/usr/bin/env python3
"""Reuse a fixed pool of worktrees instead of creating a new one per task.

`git worktree add` gives each task a new path, and a new path is the one thing
Cargo cannot carry over: rustc's incremental cache is bound to the absolute path
it was built at, so a seeded worktree still pays ~60s on the first edit to
oxy-app, and again for every other crate it touches
(internal-docs/rust-build-performance.md, "The one cost seeding cannot remove").
A slot is a worktree at a path that never changes. Claiming one switches it to
your branch, so Cargo recompiles only what the branch changed, with incremental
caches intact, and disk stays capped at one target/ per slot.

    python3 scripts/worktree-slot.py claim <branch> [--base origin/main]
    python3 scripts/worktree-slot.py release [<slot-path>]
    python3 scripts/worktree-slot.py list

Slots live in $OXY_WORKTREE_SLOTS (default: <main-checkout>-slots/slot-N).
A slot is free when it has no claim file and a clean working tree; `claim`
creates a new slot only when none is free (the post-checkout hook seeds its
target/). A claim is a file in the slot's git dir, so it never shows up in
`git status`. `release` refuses a dirty slot unless --force.
"""

import argparse
import datetime
import os
import subprocess
import sys

CLAIM = "oxy-slot-claim"


def git(*args, cwd=None, check=True):
    return subprocess.run(
        ["git", *args], cwd=cwd, check=check, capture_output=True, text=True
    ).stdout.strip()


def main_checkout():
    common = git("rev-parse", "--path-format=absolute", "--git-common-dir")
    return os.path.dirname(common)


def slots_dir(main):
    return os.environ.get("OXY_WORKTREE_SLOTS") or f"{main}-slots"


def slot_index(path):
    tail = os.path.basename(path).split("-", 1)[-1]
    return int(tail) if tail.isdigit() else 0


def slots(main):
    base = os.path.realpath(slots_dir(main))
    if not os.path.isdir(base):
        return []
    names = sorted((n for n in os.listdir(base) if n.startswith("slot-")), key=slot_index)
    return [os.path.join(base, n) for n in names if os.path.exists(os.path.join(base, n, ".git"))]


def claim_file(slot):
    return os.path.join(git("rev-parse", "--absolute-git-dir", cwd=slot), CLAIM)


def claimed_by(slot):
    try:
        with open(claim_file(slot)) as f:
            return f.read().strip()
    except FileNotFoundError:
        return None


def is_clean(slot):
    return git("status", "--porcelain", cwd=slot) == ""


def take_claim(slot):
    """The claim file IS the lock: O_EXCL means exactly one claimer wins."""
    try:
        return os.open(claim_file(slot), os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o644)
    except FileExistsError:
        return None


def new_slot(main, base):
    """Create the next slot-N. mkdir is atomic, so racing creators pick distinct N."""
    os.makedirs(slots_dir(main), exist_ok=True)
    root = os.path.realpath(slots_dir(main))
    existing = [slot_index(n) for n in os.listdir(root) if n.startswith("slot-")]
    n = max(existing, default=0) + 1
    while True:
        path = os.path.join(root, f"slot-{n}")
        try:
            os.mkdir(path)
            break
        except FileExistsError:
            n += 1
    git("worktree", "add", "--detach", path, base, cwd=main)  # accepts the empty dir
    return path


def cmd_claim(args, main):
    slot, fd = None, None
    while fd is None:
        for cand in slots(main):
            if claimed_by(cand) is None and is_clean(cand):
                fd = take_claim(cand)
                if fd is not None:
                    slot = cand
                    break
        if fd is None:
            # A slot is visible to other claimers as soon as it exists, so the
            # creator can lose it too; the loop just tries again.
            slot = new_slot(main, args.base)
            fd = take_claim(slot)
    try:
        exists = git("rev-parse", "--verify", "--quiet", f"refs/heads/{args.branch}",
                     cwd=slot, check=False)
        if exists:
            git("switch", args.branch, cwd=slot)
        else:
            git("switch", "-c", args.branch, args.base, cwd=slot)
    except BaseException:
        os.close(fd)
        os.remove(claim_file(slot))  # give the slot back untouched
        raise
    stamp = datetime.datetime.now().isoformat(timespec="seconds")
    with os.fdopen(fd, "w") as f:
        f.write(f"{args.branch} {stamp} pid={os.getppid()}\n")
    print(slot)
    return 0


def cmd_release(args, main):
    slot = os.path.abspath(args.slot or os.getcwd())
    slot = os.path.realpath(git("rev-parse", "--show-toplevel", cwd=slot))
    if slot not in slots(main):
        print(f"{slot} is not a slot under {slots_dir(main)}", file=sys.stderr)
        return 1
    if not is_clean(slot) and not args.force:
        print(f"{slot} has uncommitted changes; commit, stash, or --force", file=sys.stderr)
        return 1
    if not is_clean(slot):
        git("stash", "push", "--include-untracked", "-m", f"slot release {slot}", cwd=slot)
        print("stashed uncommitted changes (git stash list)", file=sys.stderr)
    # Detach so the branch can be checked out elsewhere; target/ is kept warm.
    git("switch", "--detach", cwd=slot)
    try:
        os.remove(claim_file(slot))
    except FileNotFoundError:
        pass
    print(f"released {slot}")
    return 0


def cmd_list(_args, main):
    for slot in slots(main):
        who = claimed_by(slot) or ("free" if is_clean(slot) else "unclaimed but dirty")
        print(f"{slot}\t{who}")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("claim")
    c.add_argument("branch")
    c.add_argument("--base", default="origin/main")
    r = sub.add_parser("release")
    r.add_argument("slot", nargs="?")
    r.add_argument("--force", action="store_true")
    sub.add_parser("list")
    args = ap.parse_args()
    try:
        main_tree = main_checkout()
        return {"claim": cmd_claim, "release": cmd_release, "list": cmd_list}[args.cmd](args, main_tree)
    except subprocess.CalledProcessError as e:
        # git's own reason (e.g. "branch is already used by worktree at …").
        print((e.stderr or "").strip() or e, file=sys.stderr)
        return e.returncode


if __name__ == "__main__":
    sys.exit(main())
