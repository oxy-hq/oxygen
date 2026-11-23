#!/usr/bin/env python3
"""Rank the functions this repo should simplify next, worst first.

Three inputs, none of which knows about the other two:

- `bca check` says which functions breach a metric threshold, and by how much.
  It has no idea which of them anyone touches, or whether a test would notice if
  a refactor broke them.
- `bca vcs` scores each *file* by change history: churn, authors, bug-fix
  commits. It is file-level and metric-blind.
- the `code-review-graph` SQLite database holds `TESTED_BY` and `CALLS` edges per
  function, which is the part no general-purpose metric tool can have.

The join is the whole point. A tangled function nobody touches costs nothing, so
ranking is `file risk x threshold excess` — the churn-times-complexity shape, not
complexity alone. And a tangled function with no test is not a refactor
candidate at all: it is a *write a test first* candidate, which is what
`kind` says. An agent handed the first list without the second is a regression
factory.

Run from anywhere in the repo:

    python3 scripts/codehealth/worklist.py                  # the top 10, as text
    python3 scripts/codehealth/worklist.py --format json    # the same, machine-readable
    python3 scripts/codehealth/worklist.py --format brief --top 1
    python3 scripts/codehealth/worklist.py --self-test      # check the ranking logic

Exit codes: 0 with a worklist, 3 when every input was fine but nothing ranked
(the loop's "nothing to do" — not a failure), 1 on a tool error.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import os
import shutil
import sqlite3
import subprocess
import tomllib
import sys
from pathlib import Path
from typing import NamedTuple

sys.path.insert(0, str(Path(__file__).parent))

# `bca check` names a whole-file offender this; a file over 600 lines is real
# debt but it is not one function, and this loop ships one function per PR.
FILE_SCOPE = "<file>"

# A baseline entry matches within 50 lines by default, and a function drifts by
# a few lines between the graph build and the metric run. Same tolerance here.
LINE_TOLERANCE = 50

# Only these rank. The other gated metrics — `abc`, `nargs`, `loc.*` — measure
# size, and the measured distribution already settled that size is the wrong
# proxy for "a reviewer cannot hold this". The proof arrived on the first real
# run of this script: `abc` put `buildProgram` in `sdk/cli/src/main.ts` at the
# top of the worklist, the 1,889-line flat registration table whose own
# cognitive complexity clears the limit, and which the design doc names as the
# exact change nobody should make. Cognitive and cyclomatic charge for nesting
# and branching, which is the tangle this loop exists to unpick. The others
# still gate — a *new* 716-ABC function deserves the question — they just rank
# nothing.
RANKING_METRICS = frozenset({"cognitive", "cyclomatic"})

# The loop may not change what verifies it — the ranking script, the installer,
# a workflow, a manifest — which the weekly workflow enforces on a clean
# checkout. So this must not *assign* those either: a work item the agent is
# forbidden to act on wastes a whole week and reads as the loop being broken.
# `bca` still measures and gates them; they just never become work.
LOOP_OFF_LIMITS = ("scripts/", ".github/", ".claude/")

# Test code is excluded, not ranked low. Simplifying a test for its cognitive
# score is churn with no upside, and `served_router_tests.rs` is otherwise the
# single highest-churn file in the repo — it would sit at the top of every run.
TEST_MARKERS = ("/tests/", "tests/", "/test/", ".test.", ".spec.", "_tests.rs", "_test.rs")


class Offender(NamedTuple):
    """One metric breach on one space, from `bca check`'s SARIF document."""

    metric: str
    value: float
    limit: float
    path: str
    name: str
    line_start: int
    line_end: int

    @property
    def excess(self) -> float:
        """How far past the limit, as a ratio. 1.0 means exactly at the limit."""
        return self.value / self.limit if self.limit else self.value


class Facts(NamedTuple):
    """What the graph knows about a function that no metric tool can.

    `indexed` is the difference between "no test reaches this" and "the graph has
    never seen this file", and conflating them is the one mistake that would make
    this whole loop unsafe. Measured 2026-11-23: the graph holds nodes for 1,186
    of 7,878 tracked `.rs` / `.ts` / `.tsx` files, so the second case is the
    common one, not an edge case.
    """

    tested_by: int
    callers: int
    indexed: bool = False


class Sources(NamedTuple):
    """The three measurements the ranking joins.

    One struct rather than three parameters because they are produced together,
    consumed together, and the repo's own rule is to group past five arguments —
    which `rank` had just crossed, and its own metric gate said so.
    """

    offenders: list[Offender]
    risk: dict[str, float]
    facts: dict[tuple[str, str], Facts]


class Item(NamedTuple):
    path: str
    name: str
    line_start: int
    line_end: int
    metrics: tuple[Offender, ...]
    risk: float
    tested_by: int
    callers: int
    indexed: bool
    score: float

    @property
    def kind(self) -> str:
        """What the agent is allowed to do with this item.

        The hard rule is "no refactor that no test would catch", and the graph
        can only answer it for a file it has indexed. So there are three answers,
        not two, and the third is the most common one here:

        - `refactor`: the graph names tests that reach this function.
        - `write-tests`: the graph indexes this file and finds no test. Write
          tests, change nothing else.
        - `prove-coverage`: the graph has never seen this file, so it knows
          nothing. The agent must demonstrate coverage — break the function,
          watch a test fail — before reshaping it, and write a test instead when
          it cannot. Stronger evidence than a `TESTED_BY` edge, and it does not
          depend on what the index happens to cover.
        """
        if not self.indexed:
            return "prove-coverage"
        return "refactor" if self.tested_by else "write-tests"


class ToolError(Exception):
    """An input could not be produced or read. Distinct from "nothing ranked"."""


def repo_root(start: Path) -> Path:
    """The worktree root, so the script runs from anywhere under it."""
    out = run(["git", "rev-parse", "--show-toplevel"], cwd=start)
    return Path(out.strip())


def run(cmd: list[str], cwd: Path | None = None) -> str:
    try:
        proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, check=False)
    except FileNotFoundError as exc:
        raise ToolError(f"{cmd[0]} is not installed") from exc
    if proc.returncode != 0:
        raise ToolError(f"{' '.join(cmd[:2])} exited {proc.returncode}: {proc.stderr.strip()[:400]}")
    return proc.stdout


def bca_binary(root: Path) -> str:
    """`.tools/bin/bca` is where `scripts/ci/install-bca.sh` puts it; PATH otherwise."""
    pinned = root / ".tools" / "bin" / "bca"
    if pinned.is_file():
        return str(pinned)
    found = shutil.which("bca")
    if not found:
        raise ToolError("bca not found: run scripts/ci/install-bca.sh")
    return found


def is_test_code(path: str) -> bool:
    return any(marker in path for marker in TEST_MARKERS)


def is_off_limits(path: str) -> bool:
    return path.startswith(LOOP_OFF_LIMITS)


def slug_of(path: str, name: str) -> str:
    """The branch-name form of one work item, and the loop's identity for it.

    The weekly workflow names its branch `codehealth/<slug>--<run id>`, so this
    is how a run recognises an item it already has an open PR for. It lives here
    rather than in a `jq` expression in the workflow because it is the key two
    runs a week apart have to agree on, and the two spellings had already
    disagreed once.

    **The path is part of it.** A slug built from the name alone collides on
    every ordinary name — `handler`, `render`, `main`, the same `Type::method`
    in two crates — so an open PR for one `handler` would silently drop every
    other `handler` from the worklist, and the loop would look like it had run
    out of work. The extension is dropped because it adds nothing: one path is
    one file.

    The separator before the run id is a *double* dash, and that is load-bearing:
    runs of non-alphanumerics collapse to a single dash here, so a slug can never
    contain `--`, which makes `codehealth/<slug>--<run id>` unambiguous to split
    back apart. With a single dash, a function named `step_2` produces
    `...-step-2` and the run-id strip eats the `2`.
    """
    stem = path.rsplit(".", 1)[0] if "." in path.rsplit("/", 1)[-1] else path
    return re.sub(r"[^a-z0-9]+", "-", f"{stem}-{name}".lower()).strip("-")


def normalize_name(name: str) -> str:
    """`bca` spells a method `Type::method`; the graph spells it `Type.method`."""
    return name.replace("::", ".")


def parse_sarif(doc: dict) -> list[Offender]:
    """Every current offender, baselined debt included, from one SARIF run."""
    offenders = []
    for run_doc in doc.get("runs", []):
        for result in run_doc.get("results", []):
            parsed = _parse_result(result)
            if parsed is not None:
                offenders.append(parsed)
    return offenders


def _parse_result(result: dict) -> Offender | None:
    """One SARIF result, or None when it names a file rather than a function."""
    locations = result.get("locations") or []
    if not locations:
        return None
    physical = locations[0].get("physicalLocation", {})
    uri = physical.get("artifactLocation", {}).get("uri")
    region = physical.get("region", {})
    logical = locations[0].get("logicalLocations") or [{}]
    name = logical[0].get("fullyQualifiedName")
    if not uri or not name or name == FILE_SCOPE:
        return None
    value, limit = _parse_message(result.get("message", {}).get("text", ""))
    if value is None or limit is None:
        return None
    return Offender(
        metric=result.get("ruleId", "?"),
        value=value,
        limit=limit,
        path=uri,
        name=normalize_name(name),
        line_start=int(region.get("startLine", 0)),
        line_end=int(region.get("endLine", region.get("startLine", 0))),
    )


def _parse_message(text: str) -> tuple[float | None, float | None]:
    """`"cognitive 23 exceeds limit 15"` -> (23.0, 15.0).

    The value is only in the message; SARIF has no field for it, and a future
    `bca` could reword this. A shape it cannot parse is dropped rather than
    guessed, which costs one worklist row and never invents a number.
    """
    parts = text.split()
    if len(parts) < 5 or parts[2] != "exceeds" or parts[3] != "limit":
        return None, None
    try:
        return float(parts[1]), float(parts[4])
    except ValueError:
        return None, None


def parse_risk(doc: dict) -> dict[str, float]:
    """Path -> change-history risk score, from `bca vcs`'s JSON."""
    risk = {}
    for entry in doc.get("files", []):
        path = entry.get("path")
        score = entry.get("vcs", {}).get("risk_score")
        if path and score is not None:
            risk[path] = float(score)
    return risk


def relativize(absolute: str, tracked: frozenset[str]) -> str | None:
    """Turn the graph's absolute `file_path` into a repo-relative one.

    The graph stores absolute paths, and this database has rows under *two*
    prefixes — the repo moved and the stale rows were never pruned. Stripping
    one known prefix would silently drop 4,256 nodes, and hardcoding a prefix
    would break in CI, where the checkout is somewhere else entirely. So the
    answer comes from the repo itself: the longest path suffix that git tracks.
    A suffix that matches nothing is a file that no longer exists, which is
    exactly the row to drop.
    """
    parts = absolute.replace("\\", "/").split("/")
    for start in range(len(parts)):
        candidate = "/".join(parts[start:])
        if candidate in tracked:
            return candidate
    return None


def relativize_qualified(qualified: str, tracked: frozenset[str]) -> str | None:
    """`<abs path>::<symbol>` -> `<rel path>::<symbol>`, so edges dedupe."""
    path, sep, symbol = qualified.partition("::")
    if not sep:
        return None
    rel = relativize(path, tracked)
    return f"{rel}::{symbol}" if rel else None


def load_graph_facts(db_path: Path, tracked: frozenset[str]) -> dict[tuple[str, str], Facts]:
    """Per-function `TESTED_BY` and `CALLS` counts, keyed by (rel path, name).

    Read-only, and the schema version is asserted: when upstream moves, this
    fails loudly rather than returning zero tests for everything — which would
    read as "nothing is tested" and send the agent to write tests that exist.
    """
    if not db_path.is_file():
        raise ToolError(f"no graph database at {db_path}: run `code-review-graph update`")
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        _assert_schema(conn)
        tested, called = _edge_counts(conn, tracked)
        return _facts_by_function(conn, tracked, tested, called)
    finally:
        conn.close()


def _assert_schema(conn: sqlite3.Connection) -> None:
    row = conn.execute("select value from metadata where key = 'schema_version'").fetchone()
    if row is None:
        raise ToolError("graph database has no schema_version")
    if row[0] != "9":
        raise ToolError(
            f"graph schema is {row[0]}, this script reads 9 "
            "(check .github/actions/claude-search-tools graph-version)"
        )


def _edge_counts(
    conn: sqlite3.Connection, tracked: frozenset[str]
) -> tuple[dict[str, int], dict[str, int]]:
    """Distinct `TESTED_BY` targets and `CALLS` sources per relative symbol.

    Distinct, because the same test calling a function twice is one test, and
    the stale-prefix duplicates would otherwise double every count.
    """
    tested: dict[str, set[str]] = {}
    called: dict[str, set[str]] = {}
    query = "select kind, source_qualified, target_qualified from edges where kind in ('TESTED_BY','CALLS')"
    for kind, source, target in conn.execute(query):
        if kind == "TESTED_BY":
            _add_edge(tested, source, target, tracked)
        else:
            _add_edge(called, target, source, tracked)
    return (
        {key: len(peers) for key, peers in tested.items()},
        {key: len(peers) for key, peers in called.items()},
    )


def _add_edge(
    into: dict[str, set[str]], subject: str, peer: str, tracked: frozenset[str]
) -> None:
    key = relativize_qualified(subject, tracked)
    if key is None:
        return
    peer_key = relativize_qualified(peer, tracked) or peer
    into.setdefault(key, set()).add(peer_key)


def _facts_by_function(
    conn: sqlite3.Connection,
    tracked: frozenset[str],
    tested: dict[str, int],
    called: dict[str, int],
) -> dict[tuple[str, str], Facts]:
    facts: dict[tuple[str, str], Facts] = {}
    query = (
        "select qualified_name, file_path, name, parent_name from nodes "
        "where kind = 'Function' and is_test = 0"
    )
    for qualified, file_path, name, parent in conn.execute(query):
        rel = relativize(file_path, tracked)
        if rel is None:
            continue
        key_symbol = relativize_qualified(qualified, tracked)
        if key_symbol is None:
            continue
        display = f"{parent}.{name}" if parent else name
        previous = facts.get((rel, display), Facts(0, 0, True))
        facts[(rel, display)] = Facts(
            tested_by=max(previous.tested_by, tested.get(key_symbol, 0)),
            callers=max(previous.callers, called.get(key_symbol, 0)),
            indexed=True,
        )
    return facts


def score_of(risk: float, offenders: tuple[Offender, ...]) -> float:
    """File risk times the worst threshold excess.

    Multiplicative on purpose: a complex function in code nobody touches scores
    near zero, which is the ranking the design argues for. The excess is the
    worst single metric rather than a sum, so a function breaching four
    correlated metrics does not outrank one that is genuinely worse on the one
    that matters.
    """
    worst = max(offender.excess for offender in offenders)
    return risk * worst


def rank(
    sources: Sources,
    top: int,
    exclude_slugs: frozenset[str] = frozenset(),
) -> list[Item]:
    """The worklist: offenders grouped per function, scored, worst first.

    `exclude_slugs` drops functions that already have an open loop PR. Without
    it the loop re-proposes the same function every week: the ranking is
    computed from `main`, last week's PR has not merged so `main` is unchanged,
    and position 1 is the same item — a duplicate PR on a fresh branch, two of
    them competing for the same review. It also means a failing PR is not
    retried while it is open, which is the stop condition the workflow's own PR
    body used to claim and nothing implemented.
    """
    grouped: dict[tuple[str, str], list[Offender]] = {}
    for offender in sources.offenders:
        if (
            is_test_code(offender.path)
            or is_off_limits(offender.path)
            or offender.metric not in RANKING_METRICS
        ):
            continue
        grouped.setdefault((offender.path, offender.name), []).append(offender)
    items = [_item(key, group, sources) for key, group in grouped.items()]
    items = [
        item
        for item in items
        if item.score > 0 and slug_of(item.path, item.name) not in exclude_slugs
    ]
    items.sort(key=lambda item: (-item.score, -item.callers, item.path, item.name))
    return items[:top] if top > 0 else items


def _item(key: tuple[str, str], group: list[Offender], sources: Sources) -> Item:
    path, name = key
    group.sort(key=lambda offender: -offender.excess)
    metrics = tuple(group)
    known = _lookup_facts(sources.facts, path, name)
    return Item(
        path=path,
        name=name,
        line_start=metrics[0].line_start,
        line_end=max(offender.line_end for offender in metrics),
        metrics=metrics,
        risk=sources.risk.get(path, 0.0),
        tested_by=known.tested_by,
        callers=known.callers,
        indexed=known.indexed,
        score=score_of(sources.risk.get(path, 0.0), metrics),
    )


def _lookup_facts(
    facts: dict[tuple[str, str], Facts], path: str, name: str
) -> Facts:
    """Exact match, then the bare method name, then "the graph does not know".

    The second try matters: `bca` names a trait method `Type::method` while the
    graph may hold only `method` when it could not resolve the `impl` block.

    The third is the safety property. A total miss returns `indexed=False`, which
    becomes `prove-coverage`, not `write-tests` — because the graph not knowing a
    function and a function having no tests are different facts, and only one of
    them licenses writing tests for it.
    """
    direct = facts.get((path, name))
    if direct is not None:
        return direct
    if "." in name:
        bare = facts.get((path, name.rsplit(".", 1)[1]))
        if bare is not None:
            return bare
    return Facts(0, 0, indexed=False)


def collect(root: Path, top: int, exclude_slugs: frozenset[str] = frozenset()) -> list[Item]:
    """Run the three inputs and join them."""
    bca = bca_binary(root)
    sarif_path = root / ".tools" / "codehealth-offenders.sarif"
    sarif_path.parent.mkdir(parents=True, exist_ok=True)
    run(
        [
            bca,
            "check",
            "--no-fail",
            "--report-suppressed",
            "--exclude-tests",
            "--report-format",
            "sarif",
            "--output",
            str(sarif_path),
        ],
        cwd=root,
    )
    with open(sarif_path, encoding="utf-8") as handle:
        offenders = parse_sarif(json.load(handle))
    risk = parse_risk(json.loads(run([bca, "vcs", "--format", "json", "--top", "0"], cwd=root)))
    tracked = frozenset(run(["git", "ls-files"], cwd=root).splitlines())
    db_path = graph_db(root)
    warn_if_stale(db_path, root)
    facts = load_graph_facts(db_path, tracked)
    return rank(Sources(offenders, risk, facts), top, exclude_slugs)


def graph_db(root: Path) -> Path:
    """Where to read the graph from. `CODEHEALTH_GRAPH_DB` always wins.

    In CI the graph is built at the workspace root, so the first candidate hits.
    Locally this repo is worked in `just wt-claim` worktrees, which have no graph
    of their own — the one database lives in the main checkout, ~370 MB and
    gitignored, and building a second per worktree is not worth the disk. So the
    main checkout is the fallback, and `warn_if_stale` says out loud when that
    database describes a different commit.
    """
    override = os.environ.get("CODEHEALTH_GRAPH_DB")
    if override:
        return Path(override)
    local = root / ".code-review-graph" / "graph.db"
    if local.is_file():
        return local
    shared = _main_checkout(root) / ".code-review-graph" / "graph.db"
    return shared if shared.is_file() else local


def _main_checkout(root: Path) -> Path:
    """The main checkout of this repo, which is `root` unless this is a worktree.

    `--git-common-dir` is the shared `.git` directory: the main checkout's
    `.git` even when called from a worktree, where `--git-dir` points into
    `.git/worktrees/<name>` instead.
    """
    try:
        common = Path(run(["git", "rev-parse", "--git-common-dir"], cwd=root).strip())
    except ToolError:
        return root
    if not common.is_absolute():
        common = (root / common).resolve()
    return common.parent


def warn_if_stale(db_path: Path, root: Path) -> None:
    """Say so when the graph was built at a different commit than HEAD.

    Never fatal. `TESTED_BY` and `CALLS` for a function that has not moved are
    the same at both commits, which is what this script asks of the graph — but
    an item ranked from a graph two days old can name a function that no longer
    exists, and that is worth one line on stderr rather than a surprise.
    """
    try:
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        try:
            row = conn.execute("select value from metadata where key = 'git_head_sha'").fetchone()
        finally:
            conn.close()
        head = run(["git", "rev-parse", "HEAD"], cwd=root).strip()
    except (ToolError, sqlite3.Error):
        return
    if row and row[0] != head:
        print(
            f"note: graph was built at {row[0][:12]}, HEAD is {head[:12]} — "
            "run `code-review-graph update` for current line numbers",
            file=sys.stderr,
        )


def cargo_package(root: Path, path: str) -> str | None:
    """The cargo package a repo-relative file belongs to, or None.

    Walks up to the nearest `Cargo.toml` with a `[package]` name, which is how
    cargo itself resolves it. Needed because the directory is not the package:
    `crates/server/` is `oxy-server`, `crates/core/` is plain `oxy`, and the test
    table in `CLAUDE.md` is explicit that a run must be scoped to the package —
    a bare `cargo nextest run` links every test target in 84 crates.
    """
    current = (root / path).parent
    while True:
        manifest = current / "Cargo.toml"
        if manifest.is_file():
            name = _package_name(manifest)
            if name:
                return name
        if current == root or root not in current.parents:
            return None
        current = current.parent


def _package_name(manifest: Path) -> str | None:
    try:
        with open(manifest, "rb") as handle:
            data = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError):
        return None
    name = data.get("package", {}).get("name")
    return name if isinstance(name, str) else None


def affected(root: Path, changed: list[str]) -> list[str]:
    """What a diff earns, as data: `cargo-package <name>` and `web-app` lines.

    This is the gate's half of "one function per PR, with the affected crate's
    tests actually run". The agent reports what it ran; this derives what it
    *should* have run from the diff itself, and the workflow runs that. A report
    is not evidence.

    Data, not shell. An earlier cut printed ready-to-run command lines and the
    caller `eval`-ed them, which handed arbitrary code execution to anything
    that could change this file — and this file sits under `scripts/`, inside
    the trees the loop's own agent may edit. The caller now matches a fixed verb
    and validates the name, so the worst a tampered line can do is fail to
    match.
    """
    packages = set()
    web = False
    for path in changed:
        if path.startswith("web-app/"):
            web = True
        elif path.endswith(".rs"):
            name = cargo_package(root, path)
            if name:
                packages.add(name)
    earned = [f"cargo-package {name}" for name in sorted(packages)]
    if web:
        earned.append("web-app")
    return earned


def as_text(items: list[Item]) -> str:
    rows = [
        f"{item.score:7.1f}  {item.kind:14}  {_metric_summary(item):28}  "
        f"tests {item.tested_by if item.indexed else '?':>3}  callers {item.callers:4}  "
        f"{item.path}:{item.line_start} {item.name}"
        for item in items
    ]
    return "\n".join(rows)


def _metric_summary(item: Item) -> str:
    return ", ".join(f"{m.metric} {m.value:g}/{m.limit:g}" for m in item.metrics[:2])


def as_json(items: list[Item]) -> str:
    payload = [
        {
            "score": round(item.score, 3),
            "kind": item.kind,
            "path": item.path,
            "name": item.name,
            "slug": slug_of(item.path, item.name),
            "line_start": item.line_start,
            "line_end": item.line_end,
            "risk": round(item.risk, 3),
            "tested_by": item.tested_by if item.indexed else None,
            "callers": item.callers if item.indexed else None,
            "indexed": item.indexed,
            "metrics": [
                {"metric": m.metric, "value": m.value, "limit": m.limit} for m in item.metrics
            ],
        }
        for item in items
    ]
    return json.dumps(payload, indent=2)


def as_brief(items: list[Item]) -> str:
    """The agent's brief: one item, what it may do, and what it may not."""
    if not items:
        return "No work item ranked.\n"
    return "\n\n".join(_brief_one(item) for item in items) + "\n"


# The third outcome, and the one an agent will never guess. Both of the first
# real worklist's top Rust items — `authz::allows` (cyclomatic 90) and
# `Action::as_str` (61) — are deliberately exhaustive matches that `CLAUDE.md`
# mandates, so the correct PR for them changes no code at all.
DECLINE = [
    "### If the right answer is to change nothing",
    "",
    "Some of this debt is deliberate. One exhaustive `match` that states every",
    "case is the clearest possible code and scores terribly; `crates/authz`'s",
    "`allows()` is required by `CLAUDE.md` to be exactly that. Splitting it would",
    "make the authority model harder to read and every number better.",
    "",
    "When that is the situation, do not refactor and do not leave it unmarked.",
    "Put the reason at the site and let the PR be that one line:",
    "",
    "```rust",
    "// bca: suppress(cyclomatic) -- one exhaustive match per authority ring, by design",
    "```",
    "",
    "`bca exemptions` lists every such marker, so this is a visible backlog with",
    "reasons attached, not a hidden exemption list. A PR that declines with a",
    "reason is a successful run of this loop, not a failed one.",
]


def _brief_one(item: Item) -> str:
    lines = [
        f"## {item.name} — `{item.path}:{item.line_start}-{item.line_end}`",
        "",
        f"- Work kind: **{item.kind}**",
        f"- Threshold breaches: {', '.join(f'`{m.metric}` {m.value:g} (limit {m.limit:g})' for m in item.metrics)}",
        f"- Change-history risk of this file: {item.risk:.2f}"
        f" (ranked {_risk_words(item.risk)})",
        f"- Tests that reach it: **{item.tested_by}**"
        if item.indexed
        else "- Tests that reach it: **unknown — this file is not in the graph**",
        f"- Callers in the graph: **{item.callers}**"
        if item.indexed
        else "- Callers: **unknown**",
        "",
    ]
    lines.extend(_brief_rules(item))
    if item.kind != "write-tests":
        lines.extend(["", *DECLINE])
    return "\n".join(lines)


BRIEF_RULES = {
    "write-tests": [
        "**The graph indexes this file and finds no test reaching this function,**",
        "**so you may not restructure it.** Write tests for its current behaviour",
        "instead, and change nothing else. Each test must fail if the behaviour",
        "changes — break the function on purpose, watch it go red, put it back. A",
        "test that passes against any implementation is worse than no test. Leave",
        "the complexity alone; a later run ranks it again, with tests, and then it",
        "can be reshaped.",
    ],
    "prove-coverage": [
        "**The graph has never indexed this file, so it knows nothing about this**",
        "**function's tests — and 0 is not the same answer as 'no tests'.** Before",
        "you change a line, establish coverage yourself: find the tests that reach",
        "this function, break it on purpose, and confirm one of them fails. Only",
        "then is a behaviour-preserving refactor in scope, under the same rules as",
        "a tested item.",
        "",
        "If nothing fails when you break it, stop refactoring and write that test",
        "instead — that becomes the whole PR, and say in the body that the refactor",
        "is left for a later run.",
    ],
}


def _brief_rules(item: Item) -> list[str]:
    preset = BRIEF_RULES.get(item.kind)
    if preset is not None:
        return preset
    return [
        f"{item.tested_by} test(s) reach this function, so a behaviour-preserving",
        "refactor is in scope. Behaviour-preserving means: no new feature, no new",
        "dependency, no public API change, no change to what any caller observes.",
        "Run those tests before and after. Splitting one tangled function into four",
        "that are only ever called in sequence improves every number and no one's",
        "life — if that is the only available move, say so and change nothing.",
    ]


def _risk_words(risk: float) -> str:
    if risk >= 8:
        return "among the very worst in the repo"
    if risk >= 5:
        return "high"
    if risk >= 2:
        return "moderate"
    return "low"


FORMATS = {"text": as_text, "json": as_json, "brief": as_brief}


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--top", type=int, default=10, help="how many items (0 = all)")
    parser.add_argument("--format", choices=sorted(FORMATS), default="text")
    parser.add_argument(
        "--pick",
        type=int,
        metavar="N",
        help="render only the Nth item (1-based); exit 3 when there is no such item",
    )
    parser.add_argument(
        "--exclude-slug",
        action="append",
        default=[],
        metavar="SLUG",
        help="skip the function with this branch slug; repeatable (open loop PRs)",
    )
    parser.add_argument(
        "--json-out",
        metavar="PATH",
        help="also write the whole ranked list here, so one run serves both outputs",
    )
    parser.add_argument("--self-test", action="store_true", help="check the ranking logic")
    parser.add_argument(
        "--affected",
        metavar="BASE",
        help="print the test commands the diff against BASE earns, then exit",
    )
    args = parser.parse_args(argv)
    if args.self_test:
        from worklist_selftest import self_test

        return self_test()
    if args.affected:
        return _print_affected(args.affected)
    try:
        items = collect(
            repo_root(Path.cwd()), args.top, frozenset(s for s in args.exclude_slug if s)
        )
    except ToolError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    if not items:
        print("nothing ranked: no untested-or-complex function in churning code", file=sys.stderr)
        return 3
    if args.json_out:
        # Ranking costs a full `bca vcs` history walk — ~60s on this repo cold.
        # The caller usually wants the machine-readable list *and* one item's
        # brief, and paying that twice for the same answer is the kind of waste
        # that makes a weekly job feel expensive enough to turn off.
        Path(args.json_out).write_text(as_json(items) + "\n", encoding="utf-8")
    if args.pick is not None:
        if args.pick < 1 or args.pick > len(items):
            print(f"no item at position {args.pick} of {len(items)}", file=sys.stderr)
            return 3
        items = [items[args.pick - 1]]
    rendered = FORMATS[args.format](items)
    print(rendered if rendered.endswith("\n") else rendered + "\n", end="")
    print(_mix(items), file=sys.stderr)
    return 0


def _print_affected(base: str) -> int:
    root = repo_root(Path.cwd())
    try:
        changed = run(["git", "diff", "--name-only", base], cwd=root).splitlines()
    except ToolError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    commands = affected(root, changed)
    if not commands:
        print("no package-scoped tests for this diff", file=sys.stderr)
        return 3
    print("\n".join(commands))
    return 0


def _mix(items: list[Item]) -> str:
    counts: dict[str, int] = {}
    for item in items:
        counts[item.kind] = counts.get(item.kind, 0) + 1
    parts = ", ".join(f"{count} {kind}" for kind, count in sorted(counts.items()))
    return f"{len(items)} item(s): {parts}"


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
