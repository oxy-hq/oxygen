#!/usr/bin/env python3
"""Summarise a nextest JUnit report as Markdown for a CI job summary.

Usage: nextest-summary.py <junit.xml> [--title T] [--slowest N]

Leads with the verdict (failed / passed / flaky counts and test time), then
each failure: the test, its binary, the panic message and where it panicked,
and the command that reruns just that test. The slowest tests follow,
collapsed: worth a look when a partition drags, noise when it doesn't.

It used to print only the slowest-15 table, so a red partition's summary
listed fifteen tests that passed and none of the ones that failed.
"""

import argparse
import re
import sys
import xml.etree.ElementTree as ET

MAX_FAILURES = 25


def panic_of(failure: ET.Element) -> tuple[str, str]:
    """(message, location) from a nextest <failure>; either may be empty."""
    text = failure.text or ""
    head = failure.get("message", "")
    where = re.search(r"panicked at ([^\s:]+:\d+:\d+)", head + "\n" + text)
    body = text.split("stack backtrace:", 1)[0]
    body = re.sub(r"^thread '.*?' (\(\d+\) )?panicked at \S+:\n?", "", body.strip(), count=1)
    body = body.split("\nnote: ", 1)[0].strip()
    lines = [l.strip() for l in body.splitlines() if l.strip()][:4]
    message = " · ".join(lines) or head or failure.get("type", "failed")
    if len(message) > 240:
        message = message[:237] + "…"
    return message, where.group(1) if where else ""


def code(text: str) -> str:
    return "`" + text.replace("`", "'").replace("|", "\\|") + "`"


def duration(seconds: float) -> str:
    m, s = divmod(int(round(seconds)), 60)
    return f"{m}m{s:02d}s" if m else f"{seconds:.1f}s"


def rerun(binary: str, name: str) -> str:
    package = binary.split("::", 1)[0]
    return f"cargo nextest run -p {package} -E 'binary_id(={binary}) and test(={name})'"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("junit")
    ap.add_argument("--title", default="Rust tests")
    ap.add_argument("--slowest", type=int, default=15)
    args = ap.parse_args()

    try:
        root = ET.parse(args.junit).getroot()
    except (OSError, ET.ParseError) as err:
        print(f"### {args.title}\n\n> [!WARNING]\n> No test report ({err}): the run ended before nextest wrote one.\n")
        return 1

    cases, failed, flaky = [], [], []
    for case in root.iter("testcase"):
        binary, name = case.get("classname", ""), case.get("name", "")
        seconds = float(case.get("time", "0") or 0)
        cases.append((seconds, binary, name))
        failure = case.find("failure")
        if failure is None:
            failure = case.find("error")
        if failure is not None:
            failed.append((binary, name, seconds, failure))
        elif case.find("flakyFailure") is not None or case.find("flakyError") is not None:
            flaky.append((binary, name))

    total = sum(s for s, _, _ in cases)
    passed = len(cases) - len(failed)
    verdict = []
    if failed:
        verdict.append(f"❌ **{len(failed)} failed**")
    verdict.append(f"✅ {passed:,} passed")
    if flaky:
        verdict.append(f"🔁 {len(flaky)} flaky (passed on retry)")
    verdict.append(f"{duration(total)} of test time")
    out = [f"### {args.title}", "", " · ".join(verdict), ""]

    if failed:
        out += ["| Failed test | Binary | Panic |", "| --- | --- | --- |"]
        for binary, name, _, failure in failed[:MAX_FAILURES]:
            message, where = panic_of(failure)
            at = f" at {code(where)}" if where else ""
            out.append(f"| {code(name)} | {code(binary)} | {code(message)}{at} |")
        if len(failed) > MAX_FAILURES:
            out.append(f"\n…and {len(failed) - MAX_FAILURES} more; the job log has every one.")
        out += ["", "<details><summary>Rerun locally</summary>", "", "```bash"]
        out += [rerun(b, n) for b, n, _, _ in failed[:MAX_FAILURES]]
        out += ["```", "", "</details>", ""]

    if flaky:
        out += ["Flaky: " + ", ".join(f"{code(n)} ({code(b)})" for b, n in flaky), ""]

    if cases and args.slowest > 0:
        cases.sort(reverse=True)
        top = cases[: args.slowest]
        share = sum(s for s, _, _ in top) / total * 100 if total else 0
        out += [
            f"<details><summary>Slowest {len(top)} of {len(cases):,} tests ({share:.0f}% of test time)</summary>",
            "",
            "| seconds | binary | test |",
            "| ---: | --- | --- |",
        ]
        out += [f"| {s:.1f} | {code(b)} | {code(n)} |" for s, b, n in top]
        out += ["", "</details>", ""]

    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
