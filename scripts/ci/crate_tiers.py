"""Tier and depth checks for check-crate-layering.py.

Every workspace crate sits in one of five tiers (crate-layers.toml `[tiers]`),
and a dependency may only point to a lower tier. Two things hold the line until
the workspace gets there:

- `[depth] max`, the longest chain of internal dependencies in edges. A run
  fails when the chain gets longer. A shorter one prints a notice to lower the
  number; it never fails, so two PRs that shorten chains and merge together
  can't turn a later run red.
- `[tier_edges] allowed`, every sideways or upward dependency still allowed, by
  name. One that isn't listed fails, whatever the count, so a removed
  dependency can't be swapped for a new one. A listed one that no longer exists
  fails too: the PR that removed it deletes its line. Two PRs removing
  different dependencies delete different lines, so they never conflict.

A count was tried first and broke both ways (#4488 review F1, F5, F6): failing
on an improvement blames the next PR for slack it didn't create, and not
failing lets that slack hide a new dependency. Why the tiers exist:
internal-docs/crate-tiers.md.
"""

from __future__ import annotations

from typing import NamedTuple

TIER_NAMES = ("t0", "t1", "t2", "t3", "t4")
ARROW = " -> "


class TierConfig(NamedTuple):
    tier_of: dict[str, int]
    max_depth: int
    target_depth: int
    exclude: frozenset[str]
    allowed: frozenset[tuple[str, str]]


class TierConfigError(Exception):
    """The tier map or the allowed list is inconsistent."""


def _parse_edge(entry: str) -> tuple[str, str]:
    parts = [part.strip() for part in entry.split(ARROW)]
    if len(parts) != 2 or not all(parts):
        raise TierConfigError(f"[tier_edges] allowed: {entry!r} is not 'crate -> crate'")
    return parts[0], parts[1]


def load_tiers(raw: dict) -> TierConfig:
    """Read `[depth]`, `[tiers]` and `[tier_edges]` from parsed crate-layers.toml."""
    depth, tiers, edges = raw.get("depth"), raw.get("tiers"), raw.get("tier_edges")
    if not (depth and tiers and edges is not None):
        raise TierConfigError("crate-layers.toml needs [depth], [tiers] and [tier_edges]")
    tier_of: dict[str, int] = {}
    for level, name in enumerate(TIER_NAMES):
        for crate in tiers.get(name, []):
            if crate in tier_of:
                raise TierConfigError(f"{crate!r} is listed in two tiers")
            tier_of[crate] = level
    allowed = [_parse_edge(entry) for entry in edges.get("allowed", [])]
    if len(set(allowed)) != len(allowed):
        raise TierConfigError("[tier_edges] allowed lists a dependency twice")
    return TierConfig(
        tier_of=tier_of,
        max_depth=int(depth["max"]),
        target_depth=int(depth["target"]),
        exclude=frozenset(depth.get("exclude", [])),
        allowed=frozenset(allowed),
    )


def internal_graph(
    graph: dict[str, frozenset[str]], exclude: frozenset[str]
) -> dict[str, frozenset[str]]:
    """The graph restricted to workspace members, less the excluded ones."""
    members = frozenset(graph) - exclude
    return {crate: (graph[crate] & members) - {crate} for crate in members}


def longest_chain(graph: dict[str, frozenset[str]]) -> list[str]:
    """One longest dependency chain, top first. Raises on a cycle."""
    depth: dict[str, int] = {}
    visiting: set[str] = set()

    def visit(crate: str) -> int:
        if crate in depth:
            return depth[crate]
        if crate in visiting:
            raise TierConfigError(f"dependency cycle through {crate!r}")
        visiting.add(crate)
        below = [visit(dep) for dep in graph[crate]]
        visiting.discard(crate)
        depth[crate] = 1 + max(below) if below else 0
        return depth[crate]

    if not graph:
        raise TierConfigError("the internal graph is empty")
    top = max(sorted(graph), key=visit)
    chain = [top]
    while graph[chain[-1]]:
        chain.append(max(sorted(graph[chain[-1]]), key=visit))
    return chain


def unplaced(config: TierConfig, graph: dict[str, frozenset[str]]) -> list[str]:
    """Problems with the tier map: crates in no tier, and stale entries."""
    problems = [
        f"{crate!r} is in no tier — add it to [tiers] in crate-layers.toml"
        for crate in sorted(frozenset(graph) - config.tier_of.keys())
    ]
    problems += [
        f"[tiers] lists {crate!r}, which is not a workspace member"
        for crate in sorted(config.tier_of.keys() - frozenset(graph))
    ]
    return problems


def tier_violations(
    config: TierConfig, graph: dict[str, frozenset[str]]
) -> list[tuple[str, str]]:
    """Every dependency that points to the same tier or a higher one."""
    tier = config.tier_of
    return sorted(
        (source, target)
        for source, targets in graph.items()
        for target in targets
        if tier[target] >= tier[source]
    )


def edge_failures(config: TierConfig, bad: list[tuple[str, str]]) -> list[str]:
    """New sideways dependencies, and allowed entries that no longer exist."""
    tier = config.tier_of
    found = frozenset(bad)
    failures = [
        f"{a}{ARROW}{b}: T{tier[a]} may not depend on T{tier[b]}. Remove the "
        f"dependency (a driver port in a contracts crate, or a driven port with an "
        f"adapter); never add it to [tier_edges] allowed."
        for a, b in sorted(found - config.allowed)
    ]
    failures += [
        f"{a}{ARROW}{b} is listed in [tier_edges] allowed but no longer breaks the "
        f"tiers. Good — delete its line in crate-layers.toml in this PR."
        for a, b in sorted(config.allowed - found)
    ]
    return failures


def depth_message(depth: int, config: TierConfig) -> str | None:
    if depth > config.max_depth:
        return (
            f"The longest dependency chain is {depth}, above the recorded "
            f"{config.max_depth}. Remove the dependency that lengthened it instead "
            f"of raising [depth] max."
        )
    if depth < config.max_depth:
        return (
            f"The longest dependency chain is {depth}, below the recorded "
            f"{config.max_depth}: lower [depth] max to {depth} in crate-layers.toml."
        )
    return None


def check_tiers(
    config: TierConfig, graph: dict[str, frozenset[str]]
) -> tuple[list[str], str]:
    """(failures, summary line) for the whole tier and depth check."""
    internal = internal_graph(graph, config.exclude)
    failures = unplaced(config, internal)
    if failures:
        return failures, ""
    chain = longest_chain(internal)
    depth = len(chain) - 1
    bad = tier_violations(config, internal)
    failures = edge_failures(config, bad)
    message = depth_message(depth, config)
    summary = (
        f"depth {depth} (target {config.target_depth}), "
        f"{len(bad)} dependencies against the tiers"
    )
    if message and depth > config.max_depth:
        failures.append(message)
        failures.append("Longest chain: " + ARROW.join(chain))
    elif message:
        summary += ". Notice: " + message
    return failures, summary


# --- self-test -------------------------------------------------------------


def _config(
    tiers: dict[str, int], max_depth: int, allowed: set[tuple[str, str]]
) -> TierConfig:
    return TierConfig(tiers, max_depth, 4, frozenset({"workspace-hack"}), frozenset(allowed))


_GRAPH = {
    "server": frozenset({"api", "workspace-hack", "serde"}),
    "api": frozenset({"domain"}),
    "domain": frozenset({"kernel", "other-domain"}),
    "other-domain": frozenset({"kernel"}),
    "kernel": frozenset(),
    "workspace-hack": frozenset(),
}
_TIERS = {"server": 4, "api": 3, "domain": 2, "other-domain": 2, "kernel": 0}
_KNOWN = {("domain", "other-domain")}


def self_test() -> int:
    """Synthetic graphs: each way the tier and depth checks can pass or fail."""
    return _self_test_graph() + _self_test_edges() + _self_test_config_errors()


def _self_test_graph() -> int:
    passed = 0
    internal = internal_graph(_GRAPH, frozenset({"workspace-hack"}))
    assert "workspace-hack" not in internal and "serde" not in internal["server"]
    passed += 1
    assert longest_chain(internal) == ["server", "api", "domain", "other-domain", "kernel"]
    passed += 1
    assert tier_violations(_config(_TIERS, 4, _KNOWN), internal) == sorted(_KNOWN)
    passed += 1
    assert check_tiers(_config(_TIERS, 4, _KNOWN), _GRAPH)[0] == []
    passed += 1
    worse = check_tiers(_config(_TIERS, 3, _KNOWN), _GRAPH)[0]
    assert worse and "above the recorded 3" in worse[0], worse
    passed += 1
    # A shorter chain never fails: it is a notice naming the number to lower.
    failures, summary = check_tiers(_config(_TIERS, 5, _KNOWN), _GRAPH)
    assert not failures and "lower [depth] max to 4" in summary, (failures, summary)
    passed += 1
    return passed


def _self_test_edges() -> int:
    passed = 0
    # A sideways dependency that isn't listed fails, however short the list is.
    unlisted = check_tiers(_config(_TIERS, 4, set()), _GRAPH)[0]
    assert unlisted and "may not depend on" in unlisted[0], unlisted
    passed += 1
    # A listed one that is gone fails until its line is deleted.
    stale = check_tiers(_config(_TIERS, 4, _KNOWN | {("api", "server")}), _GRAPH)[0]
    assert stale and "no longer breaks the tiers" in stale[0], stale
    passed += 1
    # Swapping one sideways dependency for another is caught both ways.
    swapped = dict(_GRAPH, domain=frozenset({"kernel"}),
                   **{"other-domain": frozenset({"kernel", "domain"})})
    found = check_tiers(_config(_TIERS, 4, _KNOWN), swapped)[0]
    assert any("other-domain -> domain: " in f for f in found), found
    assert any("domain -> other-domain" in f for f in found), found
    passed += 1
    return passed


def _self_test_config_errors() -> int:
    passed = 0
    missing = dict(_TIERS)
    del missing["kernel"]
    assert "is in no tier" in check_tiers(_config(missing, 4, _KNOWN), _GRAPH)[0][0]
    passed += 1
    gone = dict(_TIERS, gone=1)
    found = check_tiers(_config(gone, 4, _KNOWN), _GRAPH)[0]
    assert any("not a workspace member" in p for p in found), found
    passed += 1
    for bad, why in (
        ({"tiers": {"t0": ["a"], "t1": ["a"]}, "tier_edges": {}}, "a crate in two tiers"),
        ({"tiers": {"t0": ["a"]}, "tier_edges": {"allowed": ["a->"]}}, "a malformed edge"),
        ({"tiers": {"t0": ["a"]}, "tier_edges": {"allowed": ["a -> b", "a -> b"]}},
         "a duplicate edge"),
    ):
        try:
            load_tiers({"depth": {"max": 1, "target": 1}, **bad})
        except TierConfigError:
            passed += 1
        else:  # pragma: no cover
            raise AssertionError(f"{why} must raise")
    try:
        longest_chain({"a": frozenset({"b"}), "b": frozenset({"a"})})
    except TierConfigError:
        passed += 1
    else:  # pragma: no cover
        raise AssertionError("a cycle must raise")
    return passed
