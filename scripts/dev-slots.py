#!/usr/bin/env python3
"""The machine-wide registry behind `just up`: which checkout's stack is on which ports.

Every checkout (the main clone, each `just wt-claim` slot, any `git worktree`) runs its
own backend + Vite, and they all share one data layer (Postgres, Airhouse, ClickHouse).
A *slot* N is the set of ports one checkout's stack uses:

    API 3000+10N   internal API 3001+10N   Vite 5173+10N

so slot 0 is the historical 3000/3001/5173 and a second stack never fights the first.
A checkout keeps its slot across runs (its URLs, and the browser sessions on them, stay
put); a slot is taken over only when its checkout is gone, or when every slot is held and
this one's stack is not running.

    dev-slots.py claim --repo R [--exclude N]...   -> "N api internal frontend"
    dev-slots.py show --repo R                     -> the same, or nothing when unclaimed
    dev-slots.py touch --repo R [--backend PID] [--frontend PID] [--backend-url U]
                       [--backend-from N]           record what `up` left running
    dev-slots.py release --repo R
    dev-slots.py list                              every stack, live or not
    dev-slots.py others --repo R                   the OTHER checkouts' running stacks
    dev-slots.py backends --repo R [--first]       ready backend URLs, this checkout's first
    dev-slots.py with-lock [--log F] NAME|PATH -- CMD...   run CMD holding an exclusive lock

State lives in $OXY_DEV_HOME (default ${XDG_STATE_HOME:-~/.local/state}/oxy-dev).
Pinning: OXY_DEV_SLOT=N. Size: OXY_DEV_MAX_SLOTS (default 20).
"""

import argparse
import fcntl
import json
import os
import signal
import socket
import subprocess
import sys
import urllib.request
from datetime import datetime, timezone

API_BASE, INTERNAL_BASE, FRONTEND_BASE, STRIDE = 3000, 3001, 5173, 10
# Kept in step with BACKEND_MATCH / FRONTEND_MATCH in dev-up.sh: a recorded pid counts as
# that stack's only while its command line still carries one of these.
MATCH = {
    "backend": ("serve --enterprise", "start --enterprise"),
    "frontend": ("--strictPort",),
}
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def home():
    base = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
    path = os.environ.get("OXY_DEV_HOME") or os.path.join(base, "oxy-dev")
    os.makedirs(os.path.join(path, "slots"), exist_ok=True)
    os.makedirs(os.path.join(path, "locks"), exist_ok=True)
    return path


def now(timespec="seconds"):
    return datetime.now(timezone.utc).isoformat(timespec=timespec)


def ports(n):
    return {
        "api": API_BASE + STRIDE * n,
        "internal": INTERNAL_BASE + STRIDE * n,
        "frontend": FRONTEND_BASE + STRIDE * n,
    }


def max_slots():
    return int(os.environ.get("OXY_DEV_MAX_SLOTS") or 20)


def slot_path(n):
    return os.path.join(home(), "slots", f"{n}.json")


def read_slot(n):
    try:
        with open(slot_path(n), encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def write_slot(rec):
    path = slot_path(rec["slot"])
    with open(f"{path}.tmp", "w", encoding="utf-8") as f:
        json.dump(rec, f, indent=2)
        f.write("\n")
    os.replace(f"{path}.tmp", path)


def all_slots():
    names = os.listdir(os.path.join(home(), "slots"))
    recs = (
        read_slot(int(n[:-5]))
        for n in names
        if n.endswith(".json") and n[:-5].isdigit()
    )
    return sorted((r for r in recs if r), key=lambda r: r["slot"])


def ps_command(pid):
    out = subprocess.run(
        ["ps", "-o", "command=", "-p", str(pid)],
        capture_output=True,
        text=True,
        check=False,
    )
    return out.stdout.strip() if out.returncode == 0 else None


def proc_alive(rec, role):
    """The recorded process group is still that stack's `role` (see MATCH)."""
    pid = (rec.get(role) or {}).get("pid")
    if not pid:
        return False
    cmd = ps_command(pid)
    if cmd is not None:
        return any(m in cmd for m in MATCH[role])
    try:  # leader gone, members lingering: still holding its ports
        os.killpg(pid, 0)
        return True
    except (ProcessLookupError, PermissionError):
        return False


def live(rec):
    return proc_alive(rec, "backend") or proc_alive(rec, "frontend")


def port_in_use(port):
    """Anything listening, on either loopback family, or holding the wildcard bind."""
    for host, family in (("127.0.0.1", socket.AF_INET), ("::1", socket.AF_INET6)):
        try:
            with socket.socket(family, socket.SOCK_STREAM) as s:
                s.settimeout(0.3)
                if s.connect_ex((host, port)) == 0:
                    return True
        except OSError:
            pass
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        # Linux: ignore TIME_WAIT, a listener still blocks.
        if sys.platform.startswith("linux"):
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            s.bind(("0.0.0.0", port))
        except OSError:
            return True
    return False


def ports_free(n):
    return not any(port_in_use(p) for p in ports(n).values())


def http_ok(url):
    try:
        with OPENER.open(url, timeout=5) as resp:
            return 200 <= resp.status < 400
    except (OSError, ValueError):
        return False


class Locked:
    """An exclusive flock, released by the kernel if the holder dies."""

    def __init__(self, path, label=None):
        self.path, self.label, self.fd = path, label, None

    def __enter__(self):
        self.fd = os.open(self.path, os.O_RDWR | os.O_CREAT, 0o644)
        try:
            fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            if self.label:
                holder = os.pread(self.fd, 400, 0).decode("utf-8", "replace").strip()
                print(
                    f"    waiting for {self.label} lock, held by {holder or 'another run'}",
                    file=sys.stderr,
                )
            fcntl.flock(self.fd, fcntl.LOCK_EX)
        os.ftruncate(self.fd, 0)
        holder = (
            f"pid {os.getppid()} in {os.environ.get('OXY_DEV_REPO') or os.getcwd()}"
        )
        os.pwrite(self.fd, f"{holder} since {now()}\n".encode(), 0)
        return self

    def __exit__(self, *exc):
        os.close(self.fd)


def registry_lock():
    return Locked(os.path.join(home(), "locks", "registry.lock"))


def mine(repo):
    return next((r for r in all_slots() if r.get("repo") == repo), None)


def take(n, repo, previous):
    rec = read_slot(n) if previous is None else previous
    keep = rec if rec and rec.get("repo") == repo else {}
    rec = {
        "slot": n,
        "repo": repo,
        "claimed_at": keep.get("claimed_at") or now(),
        "last_used": now("microseconds"),
        "ports": ports(n),
        "backend": keep.get("backend"),
        "frontend": keep.get("frontend"),
        "backend_from": keep.get("backend_from"),
    }
    write_slot(rec)
    for other in all_slots():  # one checkout, one slot
        if other["slot"] != n and other.get("repo") == repo:
            os.remove(slot_path(other["slot"]))
    return rec


def pinned(repo):
    n = int(os.environ["OXY_DEV_SLOT"])
    if not 0 <= n < max_slots():
        sys.exit(
            f"OXY_DEV_SLOT={n} is outside 0..{max_slots() - 1} (OXY_DEV_MAX_SLOTS)"
        )
    held = read_slot(n)
    if held and held.get("repo") != repo and os.path.isdir(held["repo"]) and live(held):
        sys.exit(f"OXY_DEV_SLOT={n} is held by {held['repo']}, whose stack is running")
    return take(n, repo, None)


def choose(repo, exclude):
    """Sticky claim, else a free or abandoned slot, else the stalest idle one."""
    own = mine(repo)
    if own and own["slot"] not in exclude:
        return take(own["slot"], repo, own)
    idle = []
    for n in range(max_slots()):
        if n in exclude:
            continue
        held = read_slot(n)
        if held is None or not os.path.isdir(held.get("repo", "")):
            if ports_free(n):
                return take(n, repo, None)
        elif not live(held):
            idle.append(held)
    for held in sorted(idle, key=lambda r: r.get("last_used") or ""):
        if ports_free(held["slot"]):
            print(
                f"    slot {held['slot']} was {held['repo']}'s (not running); it gets another next time",
                file=sys.stderr,
            )
            return take(held["slot"], repo, None)
    sys.exit(
        f"no free dev slot: all {max_slots()} are running or their ports are taken (`just stacks`)"
    )


def emit(rec):
    p = rec["ports"]
    print(rec["slot"], p["api"], p["internal"], p["frontend"])


def cmd_claim(args):
    with registry_lock():
        if os.environ.get("OXY_DEV_SLOT", "").strip():
            emit(pinned(args.repo))
        else:
            emit(choose(args.repo, set(args.exclude)))


def cmd_show(args):
    rec = mine(args.repo)
    if rec:
        emit(rec)


def cmd_touch(args):
    with registry_lock():
        rec = mine(args.repo)
        if rec is None:
            return
        p = rec["ports"]
        backend_url = args.backend_url or f"http://127.0.0.1:{p['api']}"
        rec["backend"] = (
            {"pid": args.backend, "url": backend_url} if args.backend else None
        )
        rec["frontend"] = (
            {"pid": args.frontend, "url": f"http://127.0.0.1:{p['frontend']}"}
            if args.frontend
            else None
        )
        rec["backend_from"] = args.backend_from
        rec["last_used"] = now("microseconds")
        write_slot(rec)


def cmd_release(args):
    with registry_lock():
        rec = mine(args.repo)
        if rec:
            os.remove(slot_path(rec["slot"]))


def branch(repo):
    out = subprocess.run(
        ["git", "-C", repo, "branch", "--show-current"],
        capture_output=True,
        text=True,
        check=False,
    )
    if not os.path.isdir(repo):
        return "gone"
    return out.stdout.strip() or ("detached" if out.returncode == 0 else "-")


def describe(rec, role, health_path):
    if not proc_alive(rec, role):
        return "-"
    url = rec[role]["url"]
    return f"pid {rec[role]['pid']} " + (
        "ok" if http_ok(url + health_path) else "NOT answering"
    )


def cmd_list(args):
    recs = all_slots()
    if not recs:
        print("    no stacks registered yet (`just up` claims one)")
        return
    homedir = os.path.expanduser("~")
    print(
        f"    {'slot':<4} {'api':<5} {'vite':<5} {'backend':<20} {'frontend':<20} checkout (branch)"
    )
    for rec in recs:
        p = rec["ports"]
        backend = describe(rec, "backend", "/api/ready")
        if rec.get("backend_from") is not None and proc_alive(rec, "frontend"):
            backend = f"slot {rec['backend_from']}'s"
        where = rec["repo"].replace(homedir, "~", 1)
        mark = "*" if rec["repo"] == args.repo else " "
        print(
            f"   {mark}{rec['slot']:<4} {p['api']:<5} {p['frontend']:<5} {backend:<20} "
            f"{describe(rec, 'frontend', '/'):<20} {where} ({branch(rec['repo'])})"
        )


def cmd_others(args):
    for rec in all_slots():
        if (
            rec.get("repo") != args.repo
            and os.path.isdir(rec.get("repo", ""))
            and live(rec)
        ):
            print(f"slot {rec['slot']}  {rec['repo']}")


def cmd_backends(args):
    recs = sorted(all_slots(), key=lambda r: (r.get("repo") != args.repo, r["slot"]))
    for rec in recs:
        if proc_alive(rec, "backend") and http_ok(rec["backend"]["url"] + "/api/ready"):
            print(rec["slot"], rec["backend"]["url"])
            if args.first:
                return


def cmd_with_lock(args):
    cmd = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
    if not cmd:
        sys.exit("with-lock: no command")
    name = args.name
    path = name if "/" in name else os.path.join(home(), "locks", f"{name}.lock")
    label = os.path.basename(name).removesuffix(".lock")
    with Locked(path, label):
        if not args.log:
            sys.exit(subprocess.run(cmd, check=False).returncode)
        with open(args.log, "ab") as out:
            run = subprocess.run(cmd, stdout=out, stderr=subprocess.STDOUT, check=False)
        sys.exit(run.returncode)


def main():
    signal.signal(
        signal.SIGPIPE, signal.SIG_DFL
    )  # `… | head` ends us quietly, like any CLI
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("claim", "show", "touch", "release", "list", "others", "backends"):
        p = sub.add_parser(name)
        p.add_argument("--repo", default=os.getcwd())
        if name == "claim":
            p.add_argument("--exclude", type=int, action="append", default=[])
        if name == "backends":
            p.add_argument("--first", action="store_true")
        if name == "touch":
            p.add_argument("--backend", type=int)
            p.add_argument("--frontend", type=int)
            p.add_argument("--backend-url")
            p.add_argument("--backend-from", type=int)
    lock = sub.add_parser("with-lock")
    lock.add_argument(
        "--log", help="send CMD's output here (the wait notice stays on stderr)"
    )
    lock.add_argument("name")
    lock.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if hasattr(args, "repo"):
        args.repo = os.path.realpath(args.repo)
    globals()[f"cmd_{args.command.replace('-', '_')}"](args)


if __name__ == "__main__":
    main()
