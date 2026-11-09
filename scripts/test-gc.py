#!/usr/bin/env python3
"""Report and remove what test runs leave behind in Docker.

A test that starts a container and is killed (Ctrl-C, a nextest timeout, a
crashed slot) never removes it. Left alone, a stopped one is worse than dead
weight: testcontainers' reuse lookup used to adopt any of them as a Postgres
(see crates/test-utils/src/containers/mod.rs), so leftovers broke the next run.

What this does, in order:

  1. Keeps the shared containers (`oxy-test-*`, labelled
     `tech.oxy.test-container`). They are meant to live across runs.
  2. Removes every other testcontainer that is not running.
  3. Removes every other testcontainer that has been running longer than
     --hours (default 2): no test run takes that long, so a killed one owns it.
     Younger ones are left, since another worktree's run may be using them.
  4. With --databases, drops every test database and template in the shared
     Postgres (marked `oxy-test-db:`). A database something is connected to is
     refused by Postgres and survives. Tests drop stale ones on their own after
     six hours (and a migration change builds a new template), so this is only
     a manual reset: reclaiming the space now, or starting from scratch.

    python3 scripts/test-gc.py                  # report only
    python3 scripts/test-gc.py --apply
    python3 scripts/test-gc.py --apply --databases
    just test-gc --apply                        # the same, via just
"""

import argparse
import datetime
import json
import subprocess
import sys

MANAGED = "org.testcontainers.managed-by=testcontainers"
SHARED_LABEL = "tech.oxy.test-container"
MARKER = "oxy-test-db:"


def docker(*args, check=True):
    return subprocess.run(
        ["docker", *args], check=check, capture_output=True, text=True
    ).stdout


def containers():
    ids = docker("ps", "-aq", "--filter", f"label={MANAGED}").split()
    return json.loads(docker("inspect", *ids)) if ids else []


def started_hours_ago(info):
    raw = info["State"]["StartedAt"]
    # Docker reports nanoseconds; fromisoformat takes at most microseconds.
    stamp, _, frac = raw.rstrip("Z").partition(".")
    started = datetime.datetime.fromisoformat(stamp + "." + (frac or "0")[:6])
    now = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)
    return (now - started).total_seconds() / 3600


def plan(infos, hours):
    keep, remove = [], []
    for info in infos:
        name = info["Name"].lstrip("/")
        image = info["Config"]["Image"]
        state = info["State"]["Status"]
        labels = info["Config"].get("Labels") or {}
        if SHARED_LABEL in labels:
            keep.append((name, image, f"shared, {state}"))
        elif state != "running":
            remove.append((info["Id"], name, image, state))
        elif (age := started_hours_ago(info)) >= hours:
            remove.append((info["Id"], name, image, f"running {age:.0f}h"))
        else:
            keep.append((name, image, f"running {age:.1f}h, may be in use"))
    return keep, remove


def shared_postgres(infos):
    for info in infos:
        labels = info["Config"].get("Labels") or {}
        if labels.get(SHARED_LABEL, "").startswith("oxy-test-postgres") and (
            info["State"]["Status"] == "running"
        ):
            return info["Name"].lstrip("/")
    return None


def psql(container, sql):
    return docker("exec", container, "psql", "-U", "postgres", "-Atc", sql)


def test_databases(container):
    rows = psql(
        container,
        "SELECT datname FROM pg_database "
        f"WHERE shobj_description(oid, 'pg_database') LIKE '{MARKER}%'",
    )
    return rows.split()


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--apply", action="store_true", help="remove, not just report")
    ap.add_argument("--hours", type=float, default=2.0,
                    help="a running container older than this is a leftover")
    ap.add_argument("--databases", action="store_true",
                    help="also drop the test databases in the shared Postgres")
    args = ap.parse_args()

    infos = containers()
    keep, remove = plan(infos, args.hours)
    for name, image, why in keep:
        print(f"keep    {name:40} {image}  ({why})")
    for _, name, image, why in remove:
        print(f"remove  {name:40} {image}  ({why})")

    pg = shared_postgres(infos)
    dbs = test_databases(pg) if pg else []
    if pg:
        print(f"{len(dbs)} test database(s) in {pg}"
              + ("" if args.databases else " (drop with --databases)"))

    if not args.apply:
        if remove or (args.databases and dbs):
            print("\nreport only; rerun with --apply")
        return 0

    if remove:
        docker("rm", "-f", *[cid for cid, *_ in remove])
        print(f"removed {len(remove)} container(s)")
    if args.databases:
        dropped = 0
        for db in dbs:
            # No FORCE: a database a live test is connected to stays.
            if subprocess.run(
                ["docker", "exec", pg, "psql", "-U", "postgres", "-qc",
                 f'DROP DATABASE IF EXISTS "{db}"'],
                capture_output=True,
            ).returncode == 0:
                dropped += 1
        print(f"dropped {dropped} of {len(dbs)} test database(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
