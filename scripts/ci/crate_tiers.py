"""Tier and depth checks for check-crate-layering.py.

Every workspace crate sits in one of five tiers (crate-layers.toml `[tiers]`),
and a dependency may only point to a lower tier. Two things hold the line until
the workspace gets there:

- `[depth] max`, the longest chain of internal dependencies in edges. A run
  fails when the chain gets longer. A shorter one prints a notice to lower the
  number; it never fails, so two PRs that shorten chains and merge together
  can't turn a later run red.
- `[depth] non_agentic_max`, the same ratchet over the chains that contain no
  agentic crate: the graph with every crate in `[depth] agentic` removed. A
  chain with any agentic crate in it is held to `max` only. The agentic list is
  pinned: it must equal the crates under crates/agentic/, so moving a crate in
  or out is a reviewed change to the toml, not a silent relabelling.
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
    non_agentic_max: int
    non_agentic_target: int
    agentic: frozenset[str]


class DepthCaps(NamedTuple):
    max_depth: int
    target_depth: int
    non_agentic_max: int
    non_agentic_target: int
    agentic: frozenset[str]


class TierConfigError(Exception):
    """The tier map or the allowed list is inconsistent."""


def _parse_edge(entry: str) -> tuple[str, str]:
    parts = [part.strip() for part in entry.split(ARROW)]
    if len(parts) != 2 or not all(parts):
        raise TierConfigError(f"[tier_edges] allowed: {entry!r} is not 'crate -> crate'")
    return parts[0], parts[1]


def _load_depth(depth: dict) -> DepthCaps:
    """`[depth]`: the two caps, their targets and the pinned agentic list."""
    keys = ("max", "target", "non_agentic_max", "non_agentic_target", "agentic")
    missing = [key for key in keys if key not in depth]
    if missing:
        raise TierConfigError(f"[depth] is missing {', '.join(missing)}")
    caps = DepthCaps(
        max_depth=int(depth["max"]),
        target_depth=int(depth["target"]),
        non_agentic_max=int(depth["non_agentic_max"]),
        non_agentic_target=int(depth["non_agentic_target"]),
        agentic=frozenset(depth["agentic"]),
    )
    if caps.non_agentic_max > caps.max_depth:
        raise TierConfigError("[depth] non_agentic_max is above max")
    if caps.target_depth > caps.max_depth:
        raise TierConfigError("[depth] target is above max")
    if caps.non_agentic_target > caps.non_agentic_max:
        raise TierConfigError("[depth] non_agentic_target is above non_agentic_max")
    return caps


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
    caps = _load_depth(depth)
    return TierConfig(
        tier_of=tier_of,
        max_depth=caps.max_depth,
        target_depth=caps.target_depth,
        exclude=frozenset(depth.get("exclude", [])),
        allowed=frozenset(allowed),
        non_agentic_max=caps.non_agentic_max,
        non_agentic_target=caps.non_agentic_target,
        agentic=caps.agentic,
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


class DepthCap(NamedTuple):
    key: str  # the [depth] key that records the cap
    what: str  # what the chain is, in a sentence
    recorded: int


def depth_message(depth: int, cap: DepthCap) -> str | None:
    if depth > cap.recorded:
        return (
            f"The {cap.what} is {depth}, above the recorded {cap.recorded}. Remove "
            f"the dependency that lengthened it instead of raising [depth] {cap.key}."
        )
    if depth < cap.recorded:
        return (
            f"The {cap.what} is {depth}, below the recorded {cap.recorded}: lower "
            f"[depth] {cap.key} to {depth} in crate-layers.toml."
        )
    return None


def non_agentic_graph(
    internal: dict[str, frozenset[str]], agentic: frozenset[str]
) -> dict[str, frozenset[str]]:
    """The graph with every agentic crate and every edge into one removed.

    A chain in it contains no agentic crate. One that passes through an agentic
    crate is not here at all, so a crate below the agentic stack is counted
    only from itself down, never from the top of the chain that reached it.
    """
    return {c: deps - agentic for c, deps in internal.items() if c not in agentic}


def agentic_pin_failures(config: TierConfig, found: frozenset[str]) -> list[str]:
    """`[depth] agentic` must name exactly the crates under crates/agentic/."""
    fix = (
        "Update [depth] agentic in crate-layers.toml in the same PR; moving a crate "
        "across crates/agentic/ relabels its chains, it shortens none."
    )
    failures = [
        f"{crate!r} lives under crates/agentic/ but is not in [depth] agentic. {fix}"
        for crate in sorted(found - config.agentic)
    ]
    failures += [
        f"[depth] agentic lists {crate!r}, which does not live under "
        f"crates/agentic/. {fix}"
        for crate in sorted(config.agentic - found)
    ]
    return failures


def _depth_check(
    chain: list[str], cap: DepthCap, failures: list[str]
) -> str | None:
    """Append a failure when the chain is above its cap; else return a notice."""
    message = depth_message(len(chain) - 1, cap)
    if message and len(chain) - 1 > cap.recorded:
        failures.append(message)
        failures.append(f"{cap.what[0].upper()}{cap.what[1:]}: " + ARROW.join(chain))
        return None
    return message


def check_tiers(
    config: TierConfig, graph: dict[str, frozenset[str]], agentic_found: frozenset[str]
) -> tuple[list[str], str]:
    """(failures, summary line) for the whole tier and depth check."""
    internal = internal_graph(graph, config.exclude)
    failures = agentic_pin_failures(config, agentic_found) + unplaced(config, internal)
    if failures:
        return failures, ""
    chain = longest_chain(internal)
    plain = longest_chain(non_agentic_graph(internal, config.agentic))
    bad = tier_violations(config, internal)
    failures = edge_failures(config, bad)
    notices = [
        _depth_check(chain, DepthCap("max", "longest dependency chain",
                                     config.max_depth), failures),
        _depth_check(plain, DepthCap("non_agentic_max",
                                     "longest chain with no agentic crate",
                                     config.non_agentic_max), failures),
    ]
    summary = (
        f"depth {len(chain) - 1} (target {config.target_depth}), non-agentic "
        f"{len(plain) - 1} (target {config.non_agentic_target}), "
        f"{len(bad)} dependencies against the tiers"
    )
    summary += "".join(f". Notice: {notice}" for notice in notices if notice)
    return failures, summary


# --- self-test -------------------------------------------------------------


def _config(
    tiers: dict[str, int], max_depth: int, allowed: set[tuple[str, str]]
) -> TierConfig:
    """No agentic crates, so the non-agentic cap is the full one."""
    return TierConfig(
        tiers, max_depth, 4, frozenset({"workspace-hack"}), frozenset(allowed),
        non_agentic_max=max_depth, non_agentic_target=4, agentic=frozenset(),
    )


def _check(config: TierConfig, graph: dict[str, frozenset[str]]) -> tuple[list[str], str]:
    """check_tiers with the pin satisfied: the agentic crates found are the listed ones."""
    return check_tiers(config, graph, config.agentic)


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
    return (
        _self_test_graph() + _self_test_split_depth() + _self_test_split_ratchet()
        + _self_test_agentic_pin()
        + _self_test_edges() + _self_test_config_errors() + _self_test_depth_errors()
    )


def _self_test_graph() -> int:
    passed = 0
    internal = internal_graph(_GRAPH, frozenset({"workspace-hack"}))
    assert "workspace-hack" not in internal and "serde" not in internal["server"]
    passed += 1
    assert longest_chain(internal) == ["server", "api", "domain", "other-domain", "kernel"]
    passed += 1
    assert tier_violations(_config(_TIERS, 4, _KNOWN), internal) == sorted(_KNOWN)
    passed += 1
    assert _check(_config(_TIERS, 4, _KNOWN), _GRAPH)[0] == []
    passed += 1
    worse = _check(_config(_TIERS, 3, _KNOWN), _GRAPH)[0]
    assert worse and "above the recorded 3" in worse[0], worse
    passed += 1
    # A shorter chain never fails: it is a notice naming the number to lower.
    failures, summary = _check(_config(_TIERS, 5, _KNOWN), _GRAPH)
    assert not failures and "lower [depth] max to 4" in summary, (failures, summary)
    passed += 1
    return passed


# Three readings of "non-agentic depth" give three numbers on this graph: the
# induced subgraph (the rule) gives 3, from `a` down; dropping every crate below
# an agentic one gives 2 (server -> api -> kernel); stitching through the
# agentic crate gives 5.
_SPLIT_GRAPH = {
    "server": frozenset({"agentic-x", "api"}),
    "agentic-x": frozenset({"a"}),
    "a": frozenset({"b"}),
    "b": frozenset({"c"}),
    "c": frozenset({"kernel"}),
    "api": frozenset({"kernel"}),
    "kernel": frozenset(),
}
_SPLIT_TIERS = {"server": 4, "agentic-x": 3, "a": 2, "b": 1, "c": 0, "api": 3, "kernel": 0}
# Five edges need one sideways step in five tiers; it is listed, so only depth fails.
_SPLIT_ALLOWED = frozenset({("c", "kernel")})
_AGENTIC = frozenset({"agentic-x"})


def _split(max_depth: int, non_agentic_max: int, agentic=_AGENTIC) -> TierConfig:
    return TierConfig(
        _SPLIT_TIERS, max_depth, 4, frozenset(), _SPLIT_ALLOWED,
        non_agentic_max=non_agentic_max, non_agentic_target=2, agentic=agentic,
    )


def _self_test_split_depth() -> int:
    passed = 0
    plain = longest_chain(non_agentic_graph(_SPLIT_GRAPH, _AGENTIC))
    assert plain == ["a", "b", "c", "kernel"], plain
    passed += 1
    # The chain through the agentic crate (5) is held to max only: within max,
    # and above the non-agentic cap, it passes.
    failures, summary = check_tiers(_split(5, 3), _SPLIT_GRAPH, _AGENTIC)
    assert failures == [], failures
    assert "depth 5 (target 4), non-agentic 3 (target 2)" in summary, summary
    passed += 1
    # A non-agentic chain over its cap fails, with the chain, though the full
    # depth is within max.
    failures = check_tiers(_split(5, 2), _SPLIT_GRAPH, _AGENTIC)[0]
    assert len(failures) == 2, failures
    assert "no agentic crate is 3, above the recorded 2" in failures[0], failures
    assert "[depth] non_agentic_max" in failures[0], failures
    assert failures[1].endswith("a -> b -> c -> kernel"), failures
    passed += 1
    return passed


def _self_test_split_ratchet() -> int:
    passed = 0
    # The full cap still holds the agentic chain.
    failures = check_tiers(_split(4, 3), _SPLIT_GRAPH, _AGENTIC)[0]
    assert len(failures) == 2 and "chain is 5, above the recorded 4" in failures[0]
    assert failures[1].endswith("server -> agentic-x -> a -> b -> c -> kernel")
    passed += 1
    # A shorter non-agentic chain is a notice naming the number, never a failure.
    failures, summary = check_tiers(_split(5, 4), _SPLIT_GRAPH, _AGENTIC)
    assert not failures and "lower [depth] non_agentic_max to 3" in summary, summary
    assert "lower [depth] max" not in summary, summary
    passed += 1
    # Unlabelled, the same chain is non-agentic and fails the tighter cap.
    failures = check_tiers(_split(5, 3, frozenset()), _SPLIT_GRAPH, frozenset())[0]
    assert any("no agentic crate is 5, above the recorded 3" in f for f in failures)
    passed += 1
    return passed


def _self_test_agentic_pin() -> int:
    passed = 0
    # A crate under crates/agentic/ that the list misses fails, by name.
    failures = check_tiers(_split(5, 3, frozenset()), _SPLIT_GRAPH, _AGENTIC)[0]
    assert len(failures) == 1, failures
    assert "'agentic-x' lives under crates/agentic/ but is not in [depth] agentic" \
        in failures[0], failures
    passed += 1
    # A listed crate that is not under crates/agentic/ fails, by name.
    listed = _AGENTIC | {"a"}
    failures = check_tiers(_split(5, 3, listed), _SPLIT_GRAPH, _AGENTIC)[0]
    assert len(failures) == 1, failures
    assert "[depth] agentic lists 'a', which does not live under" in failures[0], failures
    passed += 1
    # The pin is reported even when the tier map is also out of date.
    gone = dict(_SPLIT_TIERS)
    del gone["kernel"]
    config = _split(5, 3, frozenset())._replace(tier_of=gone)
    failures = check_tiers(config, _SPLIT_GRAPH, _AGENTIC)[0]
    assert any("not in [depth] agentic" in f for f in failures), failures
    assert any("is in no tier" in f for f in failures), failures
    passed += 1
    return passed


def _self_test_edges() -> int:
    passed = 0
    # A sideways dependency that isn't listed fails, however short the list is.
    unlisted = _check(_config(_TIERS, 4, set()), _GRAPH)[0]
    assert unlisted and "may not depend on" in unlisted[0], unlisted
    passed += 1
    # A listed one that is gone fails until its line is deleted.
    stale = _check(_config(_TIERS, 4, _KNOWN | {("api", "server")}), _GRAPH)[0]
    assert stale and "no longer breaks the tiers" in stale[0], stale
    passed += 1
    # Swapping one sideways dependency for another is caught both ways.
    swapped = dict(_GRAPH, domain=frozenset({"kernel"}),
                   **{"other-domain": frozenset({"kernel", "domain"})})
    found = _check(_config(_TIERS, 4, _KNOWN), swapped)[0]
    assert any("other-domain -> domain: " in f for f in found), found
    assert any("domain -> other-domain" in f for f in found), found
    passed += 1
    return passed


def _self_test_config_errors() -> int:
    passed = 0
    missing = dict(_TIERS)
    del missing["kernel"]
    assert "is in no tier" in _check(_config(missing, 4, _KNOWN), _GRAPH)[0][0]
    passed += 1
    gone = dict(_TIERS, gone=1)
    found = _check(_config(gone, 4, _KNOWN), _GRAPH)[0]
    assert any("not a workspace member" in p for p in found), found
    passed += 1
    for bad, why in (
        ({"tiers": {"t0": ["a"], "t1": ["a"]}, "tier_edges": {}}, "listed in two tiers"),
        ({"tiers": {"t0": ["a"]}, "tier_edges": {"allowed": ["a->"]}},
         "is not 'crate -> crate'"),
        ({"tiers": {"t0": ["a"]}, "tier_edges": {"allowed": ["a -> b", "a -> b"]}},
         "lists a dependency twice"),
    ):
        _expect_error({"depth": dict(_DEPTH), **bad}, why)
        passed += 1
    load_tiers({"depth": dict(_DEPTH), "tiers": {"t0": ["a"]}, "tier_edges": {}})
    passed += 1  # the baseline the cases above and below each break one way
    try:
        longest_chain({"a": frozenset({"b"}), "b": frozenset({"a"})})
    except TierConfigError:
        passed += 1
    else:  # pragma: no cover
        raise AssertionError("a cycle must raise")
    return passed


_DEPTH = {"max": 2, "target": 1, "non_agentic_max": 2, "non_agentic_target": 1,
          "agentic": ["agentic-core"]}


def _expect_error(raw: dict, expect: str) -> None:
    try:
        load_tiers(raw)
    except TierConfigError as error:
        assert expect in str(error), (expect, str(error))
    else:  # pragma: no cover
        raise AssertionError(f"{raw} must raise {expect!r}")


def _self_test_depth_errors() -> int:
    passed = 0
    for change, expect in (
        ({"non_agentic_max": 3}, "non_agentic_max is above max"),
        ({"target": 3}, "target is above max"),
        ({"non_agentic_target": 2, "non_agentic_max": 1},
         "non_agentic_target is above non_agentic_max"),
        ({"agentic": None}, "missing agentic"),
        ({"non_agentic_max": None}, "missing non_agentic_max"),
    ):
        depth = {k: v for k, v in dict(_DEPTH, **change).items() if v is not None}
        _expect_error({"depth": depth, "tiers": {"t0": ["a"]}, "tier_edges": {}}, expect)
        passed += 1
    return passed
