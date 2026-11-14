#!/usr/bin/env python3
"""Render a Trivy JSON report as a job-summary Markdown section.

Usage: trivy-summary.py REPORT.json --kind vuln|secret --title TITLE [--limit N]

The verdict comes first (counts by severity, how many have a fix), then the
findings someone can act on: fixable vulnerabilities, worst first, each with
the version that fixes it. Findings with no fix are collapsed, since nothing in
this repo can change them. Secrets are listed by file, line and rule, never by
the matched text, which Trivy masks only partially.

Replaces dumping Trivy's `table` output into the summary, which printed every
row of every target inside one fenced block: unreadable at 40 findings, and it
gave no count to read at a glance.
"""

import argparse
import json
import sys

ORDER = {"CRITICAL": 0, "HIGH": 1, "MEDIUM": 2, "LOW": 3, "UNKNOWN": 4}
# Rows past this go unlisted: a job summary is capped at 1 MiB per step, and an
# OS image can carry hundreds of unfixed CVEs.
CAP = 300
ICON = {"CRITICAL": "🟥", "HIGH": "🟧", "MEDIUM": "🟨", "LOW": "⬜", "UNKNOWN": "⬜"}


def cell(text) -> str:
    return str(text or "").replace("|", "\\|").replace("\n", " ").strip()


def short(text, n: int = 70) -> str:
    text = cell(text)
    return text if len(text) <= n else text[: n - 1] + "…"


def link(vuln: dict) -> str:
    vid = cell(vuln.get("VulnerabilityID"))
    url = vuln.get("PrimaryURL")
    return f"[{vid}]({url})" if url else vid


def counts_line(by_sev: dict) -> str:
    parts = [f"{ICON[s]} **{by_sev[s]} {s}**" for s in sorted(by_sev, key=ORDER.get) if by_sev[s]]
    return " · ".join(parts)


def subject(report: dict) -> str:
    name = report.get("ArtifactName", "")
    meta = report.get("Metadata") or {}
    os_info = meta.get("OS") or {}
    distro = f"{os_info.get('Family', '')} {os_info.get('Name', '')}".strip()
    if report.get("ArtifactType") == "container_image":
        return f"`{name}`" + (f" ({distro})" if distro else "")
    return "the repository" if name in (".", "") else f"`{name}`"


def fixable_table(rows: list, many: bool) -> list[str]:
    head = "| Severity | ID | Package | Installed | Fixed in |" + (" Where |" if many else "") + " Title |"
    out = [head, "| --- | --- | --- | --- | --- |" + (" --- |" if many else "") + " --- |"]
    for target, v in rows:
        out.append(
            f"| {ICON.get(v.get('Severity'), '')} {cell(v.get('Severity'))} | {link(v)} "
            f"| `{cell(v.get('PkgName'))}` | {cell(v.get('InstalledVersion'))} "
            f"| **{short(v.get('FixedVersion'), 40)}** |" + (f" `{cell(target)}` |" if many else "")
            + f" {short(v.get('Title'))} |"
        )
    return out + [""]


def stuck_table(stuck: list) -> list[str]:
    """No fix to apply, so one row per package: which ones carry the risk."""
    groups: dict = {}
    for target, v in stuck:
        key = (target, v.get("PkgName", ""), v.get("InstalledVersion", ""))
        groups.setdefault(key, []).append(v)
    ranked = sorted(
        groups.items(),
        key=lambda kv: (-sum(v.get("Severity") == "CRITICAL" for v in kv[1]), -len(kv[1]), kv[0][1]),
    )
    out = [f"<details><summary>{len(stuck)} with no fix available, in {len(groups)} package(s)</summary>", ""]
    out += ["| Package | Installed | Critical | High | e.g. |", "| --- | --- | ---: | ---: | --- |"]
    for (_, pkg, installed), vs in ranked[:CAP]:
        crit = sum(v.get("Severity") == "CRITICAL" for v in vs)
        high = sum(v.get("Severity") == "HIGH" for v in vs)
        worst = sorted(vs, key=lambda v: ORDER.get(v.get("Severity"), 9))[:3]
        out.append(
            f"| `{cell(pkg)}` | {short(installed, 40)} | {crit or ''} | {high or ''} "
            f"| {', '.join(link(v) for v in worst)} |"
        )
    return out + ["", "</details>", ""]


def vulns(report: dict, title: str, limit: int) -> list[str]:
    rows, seen = [], set()
    for result in report.get("Results") or []:
        for v in result.get("Vulnerabilities") or []:
            key = (v.get("VulnerabilityID"), v.get("PkgName"), v.get("InstalledVersion"), result.get("Target"))
            if key in seen:
                continue
            seen.add(key)
            rows.append((result.get("Target", ""), v))
    out = [f"### {title}", "", f"Scanned {subject(report)}.", ""]
    if not rows:
        return out + ["✅ No CRITICAL or HIGH vulnerabilities.", ""]

    by_sev: dict = {}
    for _, v in rows:
        by_sev[v.get("Severity", "UNKNOWN")] = by_sev.get(v.get("Severity", "UNKNOWN"), 0) + 1
    fixable = [(t, v) for t, v in rows if v.get("FixedVersion")]
    stuck = [(t, v) for t, v in rows if not v.get("FixedVersion")]
    out.append(f"{counts_line(by_sev)} — **{len(fixable)}** fixable by an upgrade, {len(stuck)} with no fix yet.")
    out.append("")

    targets = sorted({t for t, _ in rows})
    many = len(targets) > 1
    if many:
        out += ["| Where | Findings | Fixable |", "| --- | ---: | ---: |"]
        for t in targets:
            n = sum(1 for tt, _ in rows if tt == t)
            f = sum(1 for tt, _ in fixable if tt == t)
            out.append(f"| `{cell(t)}` | {n} | {f} |")
        out.append("")

    rank = lambda tv: (ORDER.get(tv[1].get("Severity"), 9), tv[1].get("PkgName", ""))
    if fixable:
        ranked = sorted(fixable, key=rank)
        out += ["**Fixable**, worst first", ""]
        out += fixable_table(ranked[:limit], many)
        if len(ranked) > limit:
            out += [f"<details><summary>{len(ranked) - limit} more fixable</summary>", ""]
            out += fixable_table(ranked[limit:CAP], many)
            out += ["</details>", ""]
    if stuck:
        out += stuck_table(stuck)
    return out


def secrets(report: dict, title: str, limit: int) -> list[str]:
    found = [
        (r.get("Target", ""), s) for r in report.get("Results") or [] for s in r.get("Secrets") or []
    ]
    out = [f"### {title}", ""]
    if not found:
        return out + ["✅ No secrets detected.", ""]
    out += [
        f"❌ **{len(found)} secret(s)** detected. Rotate any that are real; add a false positive to `.trivyignore.yaml`.",
        "",
        "| Severity | File | Line | Rule |",
        "| --- | --- | ---: | --- |",
    ]
    for target, s in sorted(found, key=lambda ts: (ORDER.get(ts[1].get("Severity"), 9), ts[0]))[:limit]:
        out.append(
            f"| {cell(s.get('Severity'))} | `{cell(target)}` | {cell(s.get('StartLine'))} "
            f"| {cell(s.get('Title') or s.get('RuleID'))} (`{cell(s.get('RuleID'))}`) |"
        )
    if len(found) > limit:
        out.append(f"\n…and {len(found) - limit} more.")
    return out + [""]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("report")
    ap.add_argument("--kind", choices=("vuln", "secret"), required=True, help="the scanner that wrote REPORT")
    ap.add_argument("--title", required=True)
    ap.add_argument("--limit", type=int, default=30, help="rows per table")
    args = ap.parse_args()
    try:
        with open(args.report, encoding="utf-8") as f:
            report = json.load(f)
    except (OSError, ValueError) as err:
        print(f"### {args.title}\n\n> [!WARNING]\n> No Trivy report to read ({err}); the scan step failed before writing one.\n")
        return 1
    render = secrets if args.kind == "secret" else vulns
    print("\n".join(render(report, args.title, args.limit)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
