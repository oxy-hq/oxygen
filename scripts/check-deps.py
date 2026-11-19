#!/usr/bin/env python3
"""Validate inter-crate dependency rules from internal-docs/backend-architecture.md.

Rules checked:
  1. Platform crates (oxy, oxy-agent, oxy-workflow, oxy-shared) must NEVER depend on agentic-*.
  2. agentic-core must NEVER depend on any other agentic-* crate.
  3. agentic-analytics and agentic-automation must NEVER depend on each other.
  4. agentic-http must NEVER directly depend on agentic-analytics, agentic-automation,
     agentic-connector, or agentic-llm — only through agentic-pipeline.
  5. All agentic-* crates except agentic-pipeline and agentic-http must NEVER depend
     on platform crates (oxy, oxy-agent, oxy-workflow, oxy-shared).

Usage:
  python3 scripts/check-deps.py            # check all rules
  python3 scripts/check-deps.py --verbose  # show per-crate dependency summary
"""

import json
import subprocess
import sys


def get_workspace_deps() -> dict[str, dict[str, set[str]]]:
    """Return {crate_name: {"normal": {deps}, "dev": {deps}, "build": {deps}}}."""
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        capture_output=True,
        text=True,
        check=True,
    )
    meta = json.loads(result.stdout)

    crates: dict[str, dict[str, set[str]]] = {}
    for pkg in meta["packages"]:
        name = pkg["name"]
        buckets: dict[str, set[str]] = {"normal": set(), "dev": set(), "build": set()}
        for dep in pkg.get("dependencies", []):
            kind = dep.get("kind") or "normal"
            dep_name = dep["name"]
            buckets.setdefault(kind, set()).add(dep_name)
        crates[name] = buckets
    return crates


def all_deps(buckets: dict[str, set[str]]) -> set[str]:
    return buckets["normal"] | buckets.get("dev", set()) | buckets.get("build", set())


def check(
    crates: dict[str, dict[str, set[str]]],
    verbose: bool = False,
) -> list[str]:
    violations: list[str] = []

    def violation(rule: str, msg: str) -> None:
        violations.append(f"[{rule}] {msg}")

    # ── Rule 1 ──────────────────────────────────────────────────────────────────
    # Platform crates must not depend on any agentic-* crate.
    platform_crates = {"oxy", "oxy-agent", "oxy-workflow", "oxy-shared"}
    for crate in platform_crates:
        if crate not in crates:
            continue
        bad = {d for d in all_deps(crates[crate]) if d.startswith("agentic-")}
        if bad:
            violation(
                "Rule 1 — Platform → Agentic NEVER",
                f"'{crate}' must not depend on agentic crates: {sorted(bad)}",
            )

    # ── Rule 2 ──────────────────────────────────────────────────────────────────
    # agentic-core must not depend on any other agentic-* crate.
    if "agentic-core" in crates:
        bad = {d for d in all_deps(crates["agentic-core"]) if d.startswith("agentic-")}
        if bad:
            violation(
                "Rule 2 — agentic-core is pure FSM",
                f"'agentic-core' must not depend on other agentic crates: {sorted(bad)}",
            )

    # ── Rule 3 ──────────────────────────────────────────────────────────────────
    # Domain crates must not cross-depend.
    domain_pairs = [
        ("agentic-analytics", "agentic-automation"),
        ("agentic-automation", "agentic-analytics"),
    ]
    for src, forbidden in domain_pairs:
        if src in crates and forbidden in all_deps(crates[src]):
            violation(
                "Rule 3 — Domain ↔ Domain NEVER",
                f"'{src}' must not depend on '{forbidden}'",
            )

    # ── Rule 4 ──────────────────────────────────────────────────────────────────
    # agentic-http must route through agentic-pipeline, never import domain/infra directly.
    http_forbidden = {"agentic-analytics", "agentic-automation", "agentic-connector", "agentic-llm"}
    if "agentic-http" in crates:
        bad = all_deps(crates["agentic-http"]) & http_forbidden
        if bad:
            violation(
                "Rule 4 — agentic-http thin transport",
                f"'agentic-http' must not directly depend on: {sorted(bad)}",
            )

    # ── Rule 5 ──────────────────────────────────────────────────────────────────
    # Only agentic-pipeline and agentic-http may import platform crates.
    platform_names = {"oxy", "oxy-agent", "oxy-workflow", "oxy-shared"}
    allowed_upward = {"agentic-pipeline", "agentic-http"}
    for crate, buckets in crates.items():
        if not crate.startswith("agentic-") or crate in allowed_upward:
            continue
        bad = all_deps(buckets) & platform_names
        if bad:
            violation(
                "Rule 5 — Upward deps only from pipeline/http",
                f"'{crate}' must not depend on platform crates: {sorted(bad)}",
            )

    if verbose:
        print("Workspace agentic crates and their direct deps:")
        for crate, buckets in sorted(crates.items()):
            if crate.startswith("agentic-") or crate in platform_crates:
                normal = sorted(buckets["normal"])
                print(f"  {crate}: {normal}")
        print()

    return violations


def main() -> None:
    verbose = "--verbose" in sys.argv or "-v" in sys.argv

    try:
        crates = get_workspace_deps()
    except subprocess.CalledProcessError as exc:
        print(f"error: cargo metadata failed\n{exc.stderr}", file=sys.stderr)
        sys.exit(2)

    violations = check(crates, verbose=verbose)

    rules = [
        "Rule 1  {oxy, oxy-agent, oxy-workflow, oxy-shared} must not depend on agentic-*",
        "Rule 2  agentic-core must not depend on any other agentic-* crate",
        "Rule 3  agentic-analytics ↔ agentic-automation must not cross-depend",
        "Rule 4  agentic-http must not directly import agentic-{analytics,automation,connector,llm}",
        "Rule 5  agentic-* (except pipeline/http) must not depend on platform crates",
    ]

    if violations:
        print(f"❌  {len(violations)} dependency rule violation(s) found:\n")
        for v in violations:
            print(f"  • {v}")
        print()
    else:
        print(f"✅  All dependency rules pass ({len(crates)} workspace crates checked).\n")

    print("Rules checked:")
    for rule in rules:
        print(f"  {rule}")

    if violations:
        sys.exit(1)


if __name__ == "__main__":
    main()
