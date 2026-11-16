#!/usr/bin/env python3
"""Replay a nextest run from its JUnit report: where the wall clock went.

Usage: nextest_timeline.py <junit.xml>... [--threads N] [--top N] [--bucket S]

`nextest-summary.py` answers "what failed, and which tests were slow". This
answers the question a slow partition actually raises — what was the run
*waiting on*:

  * utilisation: how many test slots were busy, over time. A run whose tail
    is one slot busy and the rest idle is bound by a serial chain (a
    `max-threads = 1` test group), not by the amount of work;
  * the serial stretches: every span where at most one test ran, and the
    tests that ran in it — that is the chain to shorten;
  * cost by binary and by test, so the fix goes where the seconds are.

Each JUnit file is replayed on its own (one per CI partition, from the
`nextest-junit-<n>` artifacts), then all are summed by binary and test.
nextest stamps every <testcase> with the time it started; without that
attribute only the totals are printed. `nextest-summary.py` imports `replay`
for the job summary's concurrency line.
"""

import argparse
import math
import sys
import xml.etree.ElementTree as ET
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional

# (binary id, test name, start as a unix time or None, seconds)
Case = tuple[str, str, Optional[float], float]


@dataclass
class Replay:
    wall: float
    busy: float
    # (length, offset from the first start, tests that ran in it), longest first
    lone: list[tuple[float, float, set[tuple[str, str]]]] = field(default_factory=list)
    # seconds of test time per `bucket`-second slice of the wall clock
    used: list[float] = field(default_factory=list)

    @property
    def lone_seconds(self) -> float:
        return sum(length for length, _, _ in self.lone)


def parse_ts(value: str) -> float | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def cases_of(root: ET.Element) -> list[Case]:
    return [
        (
            case.get("classname", ""),
            case.get("name", ""),
            parse_ts(case.get("timestamp", "")),
            float(case.get("time", "0") or 0),
        )
        for case in root.iter("testcase")
    ]


def replay(cases: list[Case], bucket: float = 30.0) -> Replay | None:
    """The run as a step function of time; None without per-test starts."""
    timed = [(b, n, s, d) for b, n, s, d in cases if s is not None]
    if not timed:
        return None
    t0 = min(s for _, _, s, _ in timed)
    wall = max(s + d for _, _, s, d in timed) - t0
    busy = sum(d for _, _, _, d in timed)

    # Ends sort before starts at the same instant, so back-to-back tests on
    # one slot never count as two running at once.
    events = sorted(
        [(s - t0, 1) for _, _, s, _ in timed] + [(s - t0 + d, -1) for _, _, s, d in timed]
    )
    running = 0
    spans, start = [], None
    for t, step in events:
        running += step
        if running <= 1 and start is None:
            start = t
        elif running > 1 and start is not None:
            spans.append((t - start, start))
            start = None
    if start is not None:
        spans.append((wall - start, start))
    lone = [
        (length, at, {(b, n) for b, n, s, d in timed if s - t0 < at + length and s - t0 + d > at})
        for length, at in sorted(spans, reverse=True)
        if length > 0
    ]

    used = [0.0] * max(1, math.ceil(wall / bucket))
    for _, _, s, d in timed:
        a, z = s - t0, s - t0 + d
        i = int(a // bucket)
        while a < z and i < len(used):
            edge = (i + 1) * bucket
            used[i] += min(z, edge) - a
            a, i = edge, i + 1
    return Replay(wall=wall, busy=busy, lone=lone, used=used)


def fmt(seconds: float) -> str:
    m, s = divmod(int(round(seconds)), 60)
    return f"{m}m{s:02d}s" if m else f"{seconds:.1f}s"


def print_replay(cases: list[Case], slots: int, bucket: float, top: int) -> None:
    run = replay(cases, bucket)
    if run is None:
        print("  (no per-test timestamps; timeline skipped)")
        return
    print(
        f"  wall {fmt(run.wall)}, test time {fmt(run.busy)}, mean concurrency "
        f"{run.busy / run.wall:.2f} of {slots} ({run.busy / (run.wall * slots) * 100:.0f}% of slots)"
    )
    print(f"\n  slots busy per {fmt(bucket)} (of {slots}):")
    for i, u in enumerate(run.used):
        width = min(bucket, run.wall - i * bucket)
        level = u / width if width > 0 else 0
        print(f"    {fmt(i * bucket):>7} {level:4.1f} {'#' * int(round(level / slots * 40))}")
    print(
        f"\n  at most one test running for {fmt(run.lone_seconds)} of {fmt(run.wall)} "
        f"({run.lone_seconds / run.wall * 100:.0f}%)"
    )
    for length, at, tests in run.lone[:top]:
        if length < 1:
            break
        names = ", ".join(sorted(f"{b} {n}" for b, n in tests)[:3])
        print(f"    {fmt(length):>7} from {fmt(at):>6}: {names}")


def print_totals(cases: list[Case], top: int) -> None:
    by_bin: dict[str, list[float]] = defaultdict(lambda: [0.0, 0])
    for b, _, _, d in cases:
        by_bin[b][0] += d
        by_bin[b][1] += 1
    total = sum(v[0] for v in by_bin.values()) or 1
    print(f"\nTest time by binary ({len(cases):,} tests, {fmt(total)}):")
    for b, (secs, count) in sorted(by_bin.items(), key=lambda x: -x[1][0])[:top]:
        print(f"  {secs:8.1f}s {secs / total * 100:5.1f}% {int(count):6} tests  {b}")
    print(f"\nSlowest {top} tests:")
    for b, n, _, d in sorted(cases, key=lambda c: -c[3])[:top]:
        print(f"  {d:8.1f}s  {b}  {n}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("junit", nargs="+")
    ap.add_argument("--threads", type=int, default=4, help="test slots the run had (CI partitions: 4)")
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--bucket", type=float, default=30.0, help="seconds per utilisation row")
    args = ap.parse_args()

    every: list[Case] = []
    for path in args.junit:
        try:
            cases = cases_of(ET.parse(path).getroot())
        except (OSError, ET.ParseError) as err:
            print(f"{path}: unreadable ({err})", file=sys.stderr)
            return 1
        print(f"== {path}: {len(cases):,} tests")
        print_replay(cases, args.threads, args.bucket, args.top)
        print()
        every += cases
    print_totals(every, args.top)
    return 0


if __name__ == "__main__":
    sys.exit(main())
