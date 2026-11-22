#!/usr/bin/env python3
"""Fail if a crate depends across a layer boundary it may not cross.

The dependency-direction rules in internal-docs/backend-architecture.md are
marked HARD and were enforced by nothing but review. This is the check that
turns them into a red build. Membership and the rules live beside this file in
`crate-layers.toml`; what fails a run is decided there, not here.

Dev-dependencies are exempt by rule, not by oversight: backend-architecture.md
argues the oxy-oltp -> agentic-connector case explicitly, and a dev-dep does not
propagate to consumers, so the lean shipped graph the rule protects is unchanged.

Run from anywhere in the repo:

    python3 scripts/ci/check-crate-layering.py              # check the workspace
    python3 scripts/ci/check-crate-layering.py --self-test  # check the checker
"""

from __future__ import annotations

import json
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import NamedTuple

RULES_PATH = Path(__file__).with_name("crate-layers.toml")
AGENTIC_DIR = "crates/agentic/"


class Rule(NamedTuple):
    name: str
    from_layers: frozenset[str]  # layer names, or {"*"} for every crate
    except_layers: frozenset[str]
    forbid_layers: frozenset[str]
    forbid_crates: frozenset[str]
    why: str


class Violation(NamedTuple):
    rule: str
    source: str
    target: str
    why: str


class ConfigError(Exception):
    """The layer map or the rules are internally inconsistent."""


def load_rules(path: Path) -> tuple[dict[str, frozenset[str]], list[Rule]]:
    """Parse crate-layers.toml into a layer map and a rule list."""
    with open(path, "rb") as f:
        raw = tomllib.load(f)
    layers = {name: frozenset(crates) for name, crates in raw.get("layers", {}).items()}
    rules = []
    for entry in raw.get("rules", []):
        origin = entry["from"]
        rule = Rule(
            name=entry["name"],
            from_layers=frozenset([origin] if isinstance(origin, str) else origin),
            except_layers=frozenset(entry.get("except", [])),
            forbid_layers=frozenset(entry.get("forbid", [])),
            forbid_crates=frozenset(entry.get("forbid_crates", [])),
            why=entry["why"],
        )
        _validate_rule(rule, layers)
        rules.append(rule)
    if not rules:
        raise ConfigError(f"{path} defines no rules")
    return layers, rules


def _validate_rule(rule: Rule, layers: dict[str, frozenset[str]]) -> None:
    """A rule naming a layer that does not exist matches nothing, silently."""
    named = set(rule.except_layers) | {
        layer for layer in rule.forbid_layers if layer != "*agentic"
    }
    named |= {layer for layer in rule.from_layers if layer != "*"}
    unknown = sorted(named - layers.keys())
    if unknown:
        raise ConfigError(f"rule {rule.name!r} names undefined layer(s): {unknown}")


def _expand(rule: Rule, layers: dict[str, frozenset[str]]) -> frozenset[str]:
    """The set of crates this rule forbids depending on."""
    names = set(rule.forbid_layers)
    if "*agentic" in names:
        names.discard("*agentic")
        names |= {name for name in layers if name.startswith("agentic_")}
    forbidden = set(rule.forbid_crates)
    for layer in names:
        forbidden |= layers[layer]
    return frozenset(forbidden)


def check(
    layers: dict[str, frozenset[str]],
    rules: list[Rule],
    graph: dict[str, frozenset[str]],
) -> list[Violation]:
    """Every forbidden edge in `graph`, which maps a crate to its dependencies."""
    violations = []
    for rule in rules:
        if "*" in rule.from_layers:
            sources = set(graph)
        else:
            sources = set().union(*(layers[layer] for layer in rule.from_layers))
        for layer in rule.except_layers:
            sources -= layers[layer]
        forbidden = _expand(rule, layers)
        for source in sorted(sources & graph.keys()):
            for target in sorted(graph[source] & forbidden):
                if target != source:
                    violations.append(Violation(rule.name, source, target, rule.why))
    return violations


def validate_membership(
    layers: dict[str, frozenset[str]],
    known: frozenset[str],
    agentic: frozenset[str],
) -> list[str]:
    """Problems with the map itself: stale entries, and unplaced agentic crates."""
    problems = []
    for layer, crates in sorted(layers.items()):
        for crate in sorted(crates - known):
            problems.append(
                f"layer {layer!r} lists {crate!r}, which is not a workspace member"
            )
    placed = frozenset().union(*layers.values()) if layers else frozenset()
    for crate in sorted(agentic - placed):
        problems.append(
            f"{crate!r} lives under {AGENTIC_DIR} but is in no layer — "
            f"add it to a layer in {RULES_PATH.name}"
        )
    return problems


def graph_from_metadata(metadata: dict) -> dict[str, frozenset[str]]:
    """Crate -> its normal and build dependencies. Dev-dependencies are dropped."""
    packages = metadata.get("packages") or []
    if not packages:
        raise ConfigError("cargo metadata returned no packages")
    graph = {}
    for package in packages:
        graph[package["name"]] = frozenset(
            dep["name"]
            for dep in package.get("dependencies", [])
            if (dep.get("kind") or "normal") != "dev"
        )
    return graph


def agentic_members(metadata: dict) -> frozenset[str]:
    """Workspace members whose manifest sits under crates/agentic/."""
    root = metadata["workspace_root"].rstrip("/") + "/"
    return frozenset(
        package["name"]
        for package in metadata["packages"]
        if package["manifest_path"].startswith(root + AGENTIC_DIR)
    )


def workspace_metadata() -> dict:
    # No `--offline`: `--no-deps` resolves nothing, so this needs no network, and
    # verify-workspace-lints.py beside it already calls cargo the same way.
    out = subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"]
    )
    return json.loads(out)


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()

    layers, rules = load_rules(RULES_PATH)
    metadata = workspace_metadata()
    graph = graph_from_metadata(metadata)

    problems = validate_membership(
        layers, frozenset(graph), agentic_members(metadata)
    )
    if problems:
        print("The crate layer map is out of date:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    violations = check(layers, rules, graph)
    if violations:
        print("Forbidden dependency edges:", file=sys.stderr)
        for violation in violations:
            print(f"  {violation.source} -> {violation.target}", file=sys.stderr)
            print(f"      rule: {violation.rule}", file=sys.stderr)
            print(f"      {violation.why}", file=sys.stderr)
        print(
            "\nThese rules come from internal-docs/backend-architecture.md and "
            f"crates/agentic/CLAUDE.md. If one is wrong, change it there and in "
            f"{RULES_PATH.name} — in the same PR, with the reason.",
            file=sys.stderr,
        )
        return 1

    print(f"{len(graph)} crates, {len(rules)} rules, no violations")
    return 0


# --- self-test -------------------------------------------------------------
#
# The rule engine is pure, so it is checked against synthetic graphs rather than
# against the workspace: a run that passes only because the real repo is clean
# would prove nothing about the rules.

_LAYERS = {
    "platform": frozenset({"oxy", "oxy-shared"}),
    "platform_upper": frozenset({"oxy", "oxy-project"}),
    "agentic_core": frozenset({"agentic-core"}),
    "agentic_runtime": frozenset({"agentic-runtime"}),
    "agentic_infra": frozenset({"agentic-connector", "agentic-llm"}),
    "agentic_domain": frozenset({"agentic-analytics", "agentic-automation"}),
    "agentic_facade": frozenset({"agentic-pipeline"}),
    "agentic_transport": frozenset({"agentic-http"}),
    "airlayer_owner": frozenset({"oxy-airlayer-compat"}),
}


def _rules() -> list[Rule]:
    return load_rules(RULES_PATH)[1]


def _edge(source: str, target: str) -> dict[str, frozenset[str]]:
    return {source: frozenset({target})}


def _self_test() -> int:
    rules = _rules()
    passed = 0

    def case(name: str, graph: dict[str, frozenset[str]], expect: str | None) -> None:
        nonlocal passed
        found = [v.rule for v in check(_LAYERS, rules, graph)]
        if expect is None:
            assert not found, f"{name}: expected no violation, got {found}"
        else:
            assert expect in found, f"{name}: expected {expect}, got {found}"
        passed += 1

    case("platform importing agentic", _edge("oxy-shared", "agentic-llm"),
         "platform-never-agentic")
    case("core importing anything agentic", _edge("agentic-core", "agentic-runtime"),
         "core-is-pure")
    case("domain importing its sibling",
         _edge("agentic-analytics", "agentic-automation"), "domains-are-siblings")
    case("domain importing the facade",
         _edge("agentic-automation", "agentic-pipeline"), "domains-are-siblings")
    case("transport importing a domain",
         _edge("agentic-http", "agentic-analytics"),
         "transport-enters-through-the-facade")
    case("transport importing infrastructure",
         _edge("agentic-http", "agentic-llm"), "transport-enters-through-the-facade")
    case("infrastructure importing the runtime",
         _edge("agentic-connector", "agentic-runtime"), "infra-never-upward")
    case("runtime importing a domain",
         _edge("agentic-runtime", "agentic-analytics"), "runtime-above-core-only")
    case("airlayer outside its owner", _edge("oxy", "airlayer"),
         "airlayer-has-one-owner")
    # The only allowed upward dependency is pipeline/http -> oxy. Without a rule
    # for it, any other agentic crate could take `oxy` and the step stayed green.
    case("core importing oxy", _edge("agentic-core", "oxy"),
         "one-upward-dependency")
    case("runtime importing oxy-project", _edge("agentic-runtime", "oxy-project"),
         "one-upward-dependency")
    case("a domain importing oxy", _edge("agentic-analytics", "oxy"),
         "one-upward-dependency")
    case("infrastructure importing oxy", _edge("agentic-llm", "oxy"),
         "one-upward-dependency")

    # Allowed edges. Each of these is a real edge in the workspace today, so a
    # rule that flags one would fail the build on correct code.
    case("facade importing a domain",
         _edge("agentic-pipeline", "agentic-analytics"), None)
    case("facade importing the platform", _edge("agentic-pipeline", "oxy"), None)
    case("transport importing the platform", _edge("agentic-http", "oxy"), None)
    # oxy-shared is below everything, so no layer may be stopped from taking it.
    case("core importing oxy-shared", _edge("agentic-core", "oxy-shared"), None)
    case("a domain importing oxy-shared",
         _edge("agentic-automation", "oxy-shared"), None)
    case("transport importing the facade",
         _edge("agentic-http", "agentic-pipeline"), None)
    case("transport importing the runtime",
         _edge("agentic-http", "agentic-runtime"), None)
    case("infrastructure importing core and oxy-shared",
         {"agentic-connector": frozenset({"agentic-core", "oxy-shared"})}, None)
    case("domain importing infrastructure",
         _edge("agentic-analytics", "agentic-connector"), None)
    case("airlayer in its owner", _edge("oxy-airlayer-compat", "airlayer"), None)
    case("an unmapped crate", _edge("oxy-cameras", "oxy-shared"), None)

    # Dev-dependencies never reach the graph, so the documented and allowed
    # oxy-oltp -> agentic-connector edge cannot fail the build.
    dev_only = {
        "packages": [
            {
                "name": "oxy-oltp",
                "dependencies": [
                    {"name": "agentic-connector", "kind": "dev"},
                    {"name": "oxy-shared", "kind": None},
                ],
            }
        ]
    }
    graph = graph_from_metadata(dev_only)
    assert graph == {"oxy-oltp": frozenset({"oxy-shared"})}, graph
    passed += 1

    # A build-dependency does reach it.
    build_dep = {
        "packages": [
            {"name": "a", "dependencies": [{"name": "agentic-llm", "kind": "build"}]}
        ]
    }
    assert graph_from_metadata(build_dep) == {"a": frozenset({"agentic-llm"})}
    passed += 1

    # An empty graph satisfies every rule vacuously, which must not read as green.
    try:
        graph_from_metadata({"packages": []})
    except ConfigError:
        passed += 1
    else:  # pragma: no cover
        raise AssertionError("empty metadata must raise")

    # A rule naming a layer that does not exist matches nothing, silently.
    try:
        _validate_rule(
            Rule(
                "typo", frozenset({"platfrom"}), frozenset(), frozenset(),
                frozenset(), "",
            ),
            _LAYERS,
        )
    except ConfigError:
        passed += 1
    else:  # pragma: no cover
        raise AssertionError("an undefined layer in a rule must raise")

    # A layer entry naming a crate that no longer exists is a stale map.
    problems = validate_membership(
        {"platform": frozenset({"oxy-gone"})}, frozenset({"oxy"}), frozenset()
    )
    assert problems and "not a workspace member" in problems[0], problems
    passed += 1

    # An agentic crate in no layer is unchecked, which is the failure the rule
    # exists to prevent.
    problems = validate_membership(
        {"platform": frozenset({"oxy"})},
        frozenset({"oxy", "agentic-new"}),
        frozenset({"agentic-new"}),
    )
    assert problems and "is in no layer" in problems[0], problems
    passed += 1

    # The shipped rule file must parse and cover every layer it names.
    layers, shipped = load_rules(RULES_PATH)
    assert len(shipped) >= 7, shipped
    assert "agentic_domain" in layers
    passed += 1

    print(f"self-test: {passed} cases passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
