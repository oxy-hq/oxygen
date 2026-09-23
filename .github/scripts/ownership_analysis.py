#!/usr/bin/env python3
"""Ownership analysis for the team responsibility matrix.

Parses the full non-merge `main` history, attributes every file change to a
person (email-mapped), buckets by code area, and prints a Markdown report that
suggests a PRIMARY (highest file-touch share) and a BACKUP (next contributor)
for each area — plus bus-factor risks and newly-appeared hot areas.

Deterministic (stdlib + `git` only). The weekly `ownership-matrix-refresh`
workflow runs this and hands the report to Claude, which reconciles it with
feature ownership (from PRs) and rewrites `internal-docs/team-ownership-matrix.md`
and `.github/CODEOWNERS`.

Run locally:  python3 .github/scripts/ownership_analysis.py
Scope a window:  python3 .github/scripts/ownership_analysis.py --since 2026-01-01

KEEP IN SYNC when the team or layout changes:
  * PEOPLE  — add/remove engineers and their git emails + GitHub handles.
  * AREAS   — keep aligned with the rules in .github/CODEOWNERS.
"""
from __future__ import annotations
import argparse, re, subprocess, sys
from collections import Counter, defaultdict

# ── Team roster ──────────────────────────────────────────────────────────────
# person -> {"handle": "@gh", "emails": [...]}  (emails span company renames)
PEOPLE = {
    "Luong": {"handle": "@luong-komorebi", "emails": [
        "vo.tran.thanh.luong@gmail.com", "luong@hyperquery.ai",
        "luong@oxy.tech", "luong@onyxint.ai"]},
    "Hai":   {"handle": "@haitrr",  "emails": ["h@hai.fyi"]},
    "Tay":   {"handle": "@hotay",   "emails": ["15603942+hotay@users.noreply.github.com"]},
    "Nick":  {"handle": "@nresh",   "emails": ["nick.reshetnikov@gmail.com"]},
    "Mars":  {"handle": "@NTtanh",  "emails": ["lenhattanh95@gmail.com"]},
}
FOCUS = list(PEOPLE)
EMAIL2PERSON = {e: p for p, v in PEOPLE.items() for e in v["emails"]}

# ── Areas (keep aligned with .github/CODEOWNERS) ─────────────────────────────
# (path-prefix, friendly area name).  A file counts toward an area if its path
# equals the prefix or starts with prefix + "/".
# Several prefixes MAY share one friendly name (an area whose files don't sit
# under a single directory): touches aggregate and the report prints one row.
AREAS = [
    ("crates/app/src/server/api/custom_apps_serve",       "Custom Apps — serve"),
    ("crates/app/src/server/api/custom_apps_functions",   "Custom Apps — functions"),
    ("crates/app/src/server/api/custom_apps_storage",     "Custom Apps — storage"),
    ("crates/app/src/custom_app_template",                "Custom Apps — template"),
    ("sdk/create-oxy-app",                                "Custom Apps — scaffold SDK"),
    # NOTE: list a nested path BEFORE its parent — the AREAS loop takes the first match.
    ("crates/app/src/server/api/admin/workspace_health",  "Workspace health & reconcile"),
    ("crates/app/src/server/api/admin/airway_config",     "Airway admission (admin)"),
    ("crates/app/src/server/api/admin/airhouse.rs",       "Airhouse fleet console (admin)"),
    ("crates/app/src/server/api/admin/oltp.rs",           "OLTP console (admin)"),
    ("crates/app/src/server/api/admin",                   "Admin (backend)"),
    ("web-app/src/pages/admin/AdminAirhouse",             "Airhouse fleet console (UI)"),
    ("web-app/src/pages/admin/AdminOltp",                 "OLTP console (UI)"),
    ("web-app/src/pages/admin",                           "Admin (UI)"),
    ("crates/oltp",                                       "OLTP (per-org Postgres)"),
    ("crates/authz",                                      "Authorization model"),
    ("crates/app/src/server/authz",                       "Authorization (app enforce)"),
    ("crates/auth",                                       "Authentication"),
    ("crates/oxy-compile",                                "Compile boundary"),
    ("crates/project",                                    "Project domain"),
    ("crates/workspace-fs",                               "Workspace FS"),
    ("crates/app-core",                                   "App-layer core seam"),
    ("crates/app/src/server/router",                      "HTTP router / fleet routing"),
    ("crates/observability",                              "Observability (traces)"),
    ("crates/telemetry",                                  "Platform telemetry"),
    ("crates/api-partner-console",                        "Partner platform (backend)"),
    ("web-app/src/pages/partners",                        "Partner platform (UI)"),
    ("crates/infrastructure/llm",                         "LLM vendor infra"),
    ("crates/agentic/llm",                                "Agentic LLM client"),
    ("crates/semantic",                                   "Semantic model"),
    ("crates/agentic/semantic",                           "Semantic (agentic shim)"),
    ("crates/app/src/server/api/world_model_graph",       "World Model (backend)"),
    ("web-app/src/pages/ide/WorldModel",                  "World Model (UI)"),
    ("crates/metric-monitoring",                          "Metric tree & anomaly monitoring"),
    ("web-app/src/pages/ide/MetricTree",                  "Metric Tree (UI)"),
    ("web-app/src/pages/ide/SemanticLayer/AnomaliesInbox", "Anomalies Inbox (UI)"),
    ("crates/airform",                                    "Airform (modeling)"),
    ("crates/cameras",                                    "Cameras / edge / video"),
    ("crates/airhouse",                                   "Airhouse (warehouse+connector)"),
    ("crates/agentic/connector",                          "Warehouse connector"),
    ("crates/agentic/airway",                             "Airway ELT"),
    ("web-app/src/pages/airway",                          "Airway ELT (UI)"),
    ("crates/agentic/runtime",                            "Agentic runtime / task queue"),
    ("crates/agentic/core",                               "Agentic FSM core"),
    ("crates/agentic/pipeline",                           "Agentic pipeline facade"),
    ("crates/agentic/http",                               "Agentic transport"),
    ("crates/platform",                                   "Platform services"),
    ("crates/server-authz",                               "Server-authz plumbing"),
    ("web-app/src/pages/launcher",                        "Web shell / HQ launcher"),
    ("crates/api-onboarding",                              "Onboarding (backend)"),
    ("web-app/src/pages/onboarding",                      "Onboarding (UI)"),
    ("web-app/src/pages/create-workspace",               "Create workspace (UI)"),
    ("web-app/tests/agentic",                             "Agentic browser tests"),
    ("crates/agentic/builder",                            "Builder copilot"),
    ("crates/agentic/analytics",                          "Analytics agent"),
    ("crates/agentic/automation",                         "Automation/workflow engine"),
    ("web-app/src/pages/automation",                      "Automation (UI)"),
    ("crates/billing",                                    "Billing / Stripe"),
    ("crates/app/src/server/api/billing",                 "Billing (backend)"),
    ("web-app/src/pages/billing",                         "Billing (UI)"),
    ("crates/app/src/server/api/organizations",           "Multi-tenancy / orgs"),
    ("crates/app/src/server/api/secrets.rs",              "Secrets"),
    # Frontline & crew ops (new 2026-09) — one area, five prefixes.
    ("crates/app/src/server/api/frontline.rs",            "Frontline & crew ops"),
    ("crates/app/src/server/api/frontline_admin.rs",      "Frontline & crew ops"),
    ("crates/app/src/server/api/frontline_devices.rs",    "Frontline & crew ops"),
    ("crates/app/src/server/api/frontline_grants.rs",     "Frontline & crew ops"),
    ("web-app/src/components/settings/SettingsDialog/sections/organization/Crew",
                                                          "Frontline & crew ops"),
    ("crates/app/src/server/api/work",                    "Work assignment graph"),
    ("crates/app/src/server/api/documents",               "Document model (KB & compliance)"),
    ("crates/app/src/server/api/chat",                    "Chat channels"),
    ("crates/app/src/server/api/notifications",           "Notifications & push"),
    ("crates/git",                                        "Git client / IDE git"),
    ("crates/app/src/server/api/custom_apps_shell_context.rs", "Client shell context"),
    ("web-app/.agents",                                   "Client shell (.agents)"),
    ("sdk/typescript",                                    "TypeScript SDK"),
    ("sdk/cli",                                           "oxyc CLI"),
    ("web-app/src/pages/context-graph",                   "Context graph (UI)"),
    ("crates/api-github",                                 "GitHub integration (backend)"),
    ("web-app/src/pages/github",                          "GitHub import (UI)"),
    ("web-app/src/pages/Invite",                          "Invites (UI)"),
    ("web-app/src/pages/ide/observability",               "Metrics-observability IDE tab"),
    ("crates/app/src/server/api/workspaces",              "Workspaces API"),
]

NOISE = re.compile(r"(^|/)(Cargo\.lock|pnpm-lock\.yaml|package-lock\.json|yarn\.lock)$"
                   r"|(^|/)dist/|(^|/)\.snap$|(^|/)node_modules/")

BACKUP_MIN_TOUCHES = 10   # a credible backup needs at least this many touches …
BACKUP_MIN_SHARE   = 8    # … and this % share of the area
BUS_FACTOR_SHARE   = 75   # primary this dominant …
BUS_FACTOR_BACKUP  = 12   # … with backup below this % = single point of failure


def person_of(email: str, name: str) -> str:
    if email in EMAIL2PERSON:
        return EMAIL2PERSON[email]
    if "[bot]" in name or "[bot]" in email or name in ("Copilot", "Claude"):
        return "__bot__"
    return "__other__"


def generic_area(path: str) -> str:
    p = path.split("/")
    if p[0] == "crates" and len(p) > 1:
        if p[1] in ("agentic", "infrastructure", "integration") and len(p) > 2:
            return f"crates/{p[1]}/{p[2]}"
        return f"crates/{p[1]}"
    if p[0] == "web-app" and len(p) > 3 and p[1] == "src" and p[2] == "pages":
        return f"web-app/src/pages/{p[3]}"
    if p[0] == "web-app" and len(p) > 2 and p[1] == "src":
        return f"web-app/src/{p[2]}"
    if p[0] == "sdk" and len(p) > 1:
        return f"sdk/{p[1]}"
    return p[0] if len(p) == 1 else f"{p[0]}/{p[1]}"


def collect(since: str | None):
    fmt = "\x01%ae\x02%an"
    cmd = ["git", "log", "--no-merges", f"--pretty=format:{fmt}", "--name-only"]
    if since:
        cmd.insert(2, f"--since={since}")
    raw = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout

    area_touch = defaultdict(Counter)      # AREAS friendly name -> person -> touches
    generic_touch = defaultdict(Counter)   # generic area -> person -> touches
    per_person_commits = Counter()
    per_person_files = Counter()
    cur = None
    for line in raw.split("\n"):
        if line.startswith("\x01"):
            email, name = line[1:].split("\x02")
            cur = person_of(email, name)
            per_person_commits[cur] += 1
            continue
        path = line.strip()
        if not path or cur is None or NOISE.search(path):
            continue
        if cur in FOCUS:
            per_person_files[cur] += 1
        for prefix, friendly in AREAS:
            if path == prefix or path.startswith(prefix + "/"):
                area_touch[friendly][cur] += 1
                break
        generic_touch[generic_area(path)][cur] += 1
    return area_touch, generic_touch, per_person_commits, per_person_files


def rank_focus(ctr: Counter):
    return [(p, c) for p, c in ctr.most_common() if p in FOCUS]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--since", help="git --since date to scope the window (default: all history)")
    args = ap.parse_args()

    area_touch, generic_touch, commits, files = collect(args.since)
    total = sum(commits.values())
    scope = f"since {args.since}" if args.since else "full main history"

    out = []
    out.append(f"# Ownership analysis — {total} non-merge commits ({scope})\n")
    out.append("_Data-derived suggestion. PRIMARY = highest file-touch share; BACKUP = next "
               "contributor. Reconcile with PR feature-ownership before writing the matrix — "
               "the file-touch leader is not always the feature owner (e.g. onboarding)._\n")

    out.append("## Per-person totals\n")
    out.append("| Person | Handle | Commits | File-touches |")
    out.append("| --- | --- | ---: | ---: |")
    for p in FOCUS:
        out.append(f"| {p} | {PEOPLE[p]['handle']} | {commits.get(p,0)} | {files.get(p,0)} |")
    out.append(f"| _(others)_ | | {commits.get('__other__',0)} | |")
    out.append(f"| _(bots)_ | | {commits.get('__bot__',0)} | |\n")

    bus = []
    out.append("## Per-area primary / backup (by file-touch share)\n")
    out.append("| Area | Primary | Backup | Other focus touches |")
    out.append("| --- | --- | --- | --- |")
    seen_areas = set()
    for _prefix, friendly in AREAS:
        if friendly in seen_areas:        # multi-prefix area — one row, counts aggregated
            continue
        seen_areas.add(friendly)
        ranked = rank_focus(area_touch.get(friendly, Counter()))
        tot = sum(c for _, c in ranked)
        if tot == 0:
            out.append(f"| {friendly} | _(no history)_ | | |")
            continue
        pri_p, pri_c = ranked[0]
        pri_sh = round(100 * pri_c / tot)
        primary = f"{PEOPLE[pri_p]['handle']} ({pri_sh}%)"
        backup, flag = "_(none — bus factor)_", ""
        if len(ranked) > 1:
            b_p, b_c = ranked[1]
            b_sh = round(100 * b_c / tot)
            if b_c >= BACKUP_MIN_TOUCHES and b_sh >= BACKUP_MIN_SHARE:
                backup = f"{PEOPLE[b_p]['handle']} ({b_sh}%)"
            else:
                backup = f"~{PEOPLE[b_p]['handle']} ({b_sh}%, thin)"
        others = ", ".join(f"{PEOPLE[p]['handle'].lstrip('@')} {c}" for p, c in ranked[2:5])
        if pri_sh >= BUS_FACTOR_SHARE and (len(ranked) < 2 or round(100*ranked[1][1]/tot) < BUS_FACTOR_BACKUP):
            flag = " ⚠️"
            bus.append((friendly, primary))
        out.append(f"| {friendly}{flag} | {primary} | {backup} | {others} |")

    out.append("\n## ⚠️ Bus-factor risks (one dominant owner, thin/absent backup)\n")
    out.append("_Cross-training candidates — the backup below is a senior generalist, not a "
               "second specialist._\n" if bus else "_None._\n")
    for friendly, primary in bus:
        out.append(f"- **{friendly}** — only {primary}")

    # New / unlisted hot areas not covered by AREAS (so coverage keeps up)
    listed_prefixes = {generic_area(pfx) for pfx, _ in AREAS}
    unlisted = []
    for a, ctr in generic_touch.items():
        ft = sum(c for p, c in ctr.items() if p in FOCUS)
        if ft >= 40 and a not in listed_prefixes:
            top = rank_focus(ctr)[:2]
            if top:
                lead = ", ".join(f"{PEOPLE[p]['handle']} {c}" for p, c in top)
                unlisted.append((ft, a, lead))
    unlisted.sort(reverse=True)
    out.append("\n## Unlisted hot areas (candidates to add to AREAS + CODEOWNERS)\n")
    if unlisted:
        out.append("| Area | Focus touches | Leaders |")
        out.append("| --- | ---: | --- |")
        for ft, a, lead in unlisted[:20]:
            out.append(f"| `{a}` | {ft} | {lead} |")
    else:
        out.append("_None above threshold._")

    print("\n".join(out))


if __name__ == "__main__":
    sys.exit(main())
