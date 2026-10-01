#!/usr/bin/env python3
"""Run a heavy cargo command only when a machine-wide build slot is free.

Cargo locks one target/ dir, not the machine. Eight agents in eight worktrees
each start `cargo nextest run` with -j <ncpu> and a multi-GB link at the end, so
the box thrashes and every build takes longer than if they had queued. This is
the "one build lease at a time" throttle GitHub used to port the Copilot runtime
with eight concurrent agent sessions.

    python3 scripts/build-lease.py cargo nextest run -p oxy-app --lib

Slots are flock()ed files under ~/.cache/oxy/build-lease/, so a crashed holder
releases its slot with its process. The just recipes that build or test
(`just unit`, `test-crate`, `check`, ...) go through this; plain `cargo` does not.

    OXY_BUILD_SLOTS=N    slots on this machine (default: see default_slots())
    OXY_BUILD_LEASE=off  run without a lease (CI does this implicitly: $CI set)

A command started under a lease runs its children with OXY_BUILD_LEASE_HELD=1,
so a recipe that calls another leased recipe does not wait on itself.
See internal-docs/rust-build-performance.md.
"""

import fcntl
import json
import os
import signal
import subprocess
import sys
import time

LEASE_DIR = os.path.join(os.path.expanduser("~"), ".cache", "oxy", "build-lease")
REPORT_EVERY = 30  # seconds between "still waiting" lines


def default_slots():
    """One slot per ~6 cores and ~8 GiB, never fewer than one.

    A debug link of an oxy test binary peaks at several GiB, and one cargo
    already saturates about six cores for most of a build.
    """
    cores = os.cpu_count() or 1
    try:
        mem_gib = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") / 2**30
    except (ValueError, OSError, AttributeError):
        mem_gib = 8 * cores
    return max(1, min(cores // 6, int(mem_gib // 8)))


def slot_count():
    raw = os.environ.get("OXY_BUILD_SLOTS", "").strip()
    if raw:
        try:
            return max(1, int(raw))
        except ValueError:
            sys.exit(f"build-lease: OXY_BUILD_SLOTS={raw!r} is not a number")
    return default_slots()


def try_acquire(n):
    """Lock the first free slot; return (fd, index) or None."""
    for i in range(n):
        fd = os.open(os.path.join(LEASE_DIR, f"slot-{i}.lock"), os.O_RDWR | os.O_CREAT, 0o644)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            os.close(fd)
            continue
        return fd, i
    return None


def holders(n):
    """Who holds each slot, as written by the holder (best effort)."""
    out = []
    for i in range(n):
        try:
            with open(os.path.join(LEASE_DIR, f"slot-{i}.lock")) as f:
                info = json.loads(f.read() or "{}")
        except (OSError, ValueError):
            continue
        if info:
            age = int(time.time() - info.get("since", time.time()))
            out.append(f"  slot {i}: pid {info.get('pid')} for {age}s in {info.get('cwd')}: {info.get('cmd')}")
    return out


def acquire(n):
    got = try_acquire(n)
    if got:
        return got
    print(f"build-lease: all {n} build slot(s) busy, waiting (OXY_BUILD_SLOTS to change):",
          file=sys.stderr)
    print("\n".join(holders(n)), file=sys.stderr, flush=True)
    started = last = time.monotonic()
    while True:
        time.sleep(1)
        got = try_acquire(n)
        if got:
            print(f"build-lease: got a slot after {int(time.monotonic() - started)}s", file=sys.stderr)
            return got
        if time.monotonic() - last >= REPORT_EVERY:
            last = time.monotonic()
            print(f"build-lease: still waiting ({int(last - started)}s)", file=sys.stderr, flush=True)


def record(fd, cmd):
    info = {"pid": os.getpid(), "cwd": os.getcwd(), "cmd": " ".join(cmd)[:200], "since": time.time()}
    os.ftruncate(fd, 0)
    os.pwrite(fd, json.dumps(info).encode(), 0)


def run(cmd, env):
    """Run cmd to completion, forwarding Ctrl-C/TERM; return its exit code."""
    proc = subprocess.Popen(cmd, env=env)
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda s, _f: proc.send_signal(s))
    code = proc.wait()
    return 128 - code if code < 0 else code


def main():
    cmd = sys.argv[1:]
    if cmd[:1] == ["--"]:
        cmd = cmd[1:]
    if not cmd:
        sys.exit("usage: build-lease.py <command> [args...]")
    env = dict(os.environ)
    off = env.get("OXY_BUILD_LEASE", "").lower() in ("0", "off", "false", "no")
    if off or env.get("OXY_BUILD_LEASE_HELD") or env.get("CI"):
        os.execvp(cmd[0], cmd)
    os.makedirs(LEASE_DIR, exist_ok=True)
    fd, _ = acquire(slot_count())
    record(fd, cmd)
    env["OXY_BUILD_LEASE_HELD"] = "1"
    try:
        return run(cmd, env)
    finally:
        os.ftruncate(fd, 0)
        os.close(fd)  # releases the flock


if __name__ == "__main__":
    sys.exit(main())
