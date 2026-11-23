#!/usr/bin/env python3
"""The self-test suite for `worklist.py`, run by `worklist.py --self-test`.

Its own file because the gate the loop enforces applies to the loop: together
the two were 822 physical lines against a 600 limit, and splitting production
logic from its tests is the split that was actually there.

Every case here was written against a mutation that breaks it. The ones worth
knowing about, because each is a silent failure rather than a crash:

- a graph read that returns nothing reads as "nothing in this repo is tested",
  which routes every work item to `write-tests` forever;
- the graph not knowing a function and a function having no tests are different
  facts, and only one of them licenses writing tests for it;
- `abc` and `nargs` measure size, and ranking on them puts a flat registration
  table at the top of the worklist — which is what the first real run did.
"""

from __future__ import annotations

import contextlib
import io
import json
import sqlite3
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from worklist import (  # noqa: E402
    BRIEF_RULES,
    DECLINE,
    FILE_SCOPE,
    Facts,
    Item,
    LINE_TOLERANCE,
    Offender,
    RANKING_METRICS,
    TEST_MARKERS,
    ToolError,
    _add_edge,
    _edge_counts,
    _facts_by_function,
    _lookup_facts,
    _mix,
    _package_name,
    _parse_message,
    _parse_result,
    affected,
    as_brief,
    as_json,
    as_text,
    cargo_package,
    load_graph_facts,
    parse_risk,
    parse_sarif,
    rank,
    relativize,
    relativize_qualified,
    slug_of,
    repo_root,
    Sources,
    score_of,
)


def _offender(metric: str, value: float, limit: float, path: str, name: str) -> Offender:
    return Offender(metric, value, limit, path, name, 10, 20)


def _self_test_parsing() -> int:
    """SARIF and risk parsing, on documents copied from real `bca` output."""
    doc = {
        "runs": [
            {
                "results": [
                    {
                        "ruleId": "cognitive",
                        "message": {"text": "cognitive 23 exceeds limit 15"},
                        "locations": [
                            {
                                "physicalLocation": {
                                    "artifactLocation": {"uri": "crates/a/src/b.rs"},
                                    "region": {"startLine": 244, "endLine": 431},
                                },
                                "logicalLocations": [
                                    {"fullyQualifiedName": "AirwayEvent::from_engine"}
                                ],
                            }
                        ],
                    },
                    {
                        "ruleId": "loc.ploc",
                        "message": {"text": "loc.ploc 606 exceeds limit 600"},
                        "locations": [
                            {
                                "physicalLocation": {
                                    "artifactLocation": {"uri": "crates/a/src/c.rs"},
                                    "region": {"startLine": 1},
                                },
                                "logicalLocations": [{"fullyQualifiedName": "<file>"}],
                            }
                        ],
                    },
                ]
            }
        ]
    }
    cases: list[tuple[str, bool]] = []
    parsed = parse_sarif(doc)
    cases.append(("file-scope offender dropped", len(parsed) == 1))
    cases.append(("method name normalized", parsed[0].name == "AirwayEvent.from_engine"))
    cases.append(("value and limit read", (parsed[0].value, parsed[0].limit) == (23.0, 15.0)))
    cases.append(("excess computed", abs(parsed[0].excess - 23 / 15) < 1e-9))
    cases.append(("line range kept", (parsed[0].line_start, parsed[0].line_end) == (244, 431)))
    cases.append(("reworded message dropped", _parse_message("cognitive is far too high") == (None, None)))
    cases.append(("non-numeric message dropped", _parse_message("x y exceeds limit z") == (None, None)))
    risk = parse_risk(
        {"files": [{"path": "crates/a/src/b.rs", "vcs": {"risk_score": 9.67}}, {"path": "x"}]}
    )
    cases.append(("risk parsed", risk == {"crates/a/src/b.rs": 9.67}))
    return _report("parsing", cases)


def _self_test_relativize() -> int:
    """The two-prefix problem this database actually has."""
    tracked = frozenset({"crates/a/src/b.rs", "web-app/src/x.ts"})
    cases = [
        (
            "current prefix stripped",
            relativize("/Volumes/luong-ext/oxy-internal-2/crates/a/src/b.rs", tracked)
            == "crates/a/src/b.rs",
        ),
        (
            "stale prefix stripped too",
            relativize("/Volumes/luong-ext/oxy-internal/crates/a/src/b.rs", tracked)
            == "crates/a/src/b.rs",
        ),
        (
            "a CI checkout elsewhere works",
            relativize("/home/runner/work/oxygen-internal/oxygen-internal/web-app/src/x.ts", tracked)
            == "web-app/src/x.ts",
        ),
        ("an untracked path is dropped", relativize("/a/b/gone.rs", tracked) is None),
        (
            "qualified name relativized",
            relativize_qualified("/any/where/crates/a/src/b.rs::T.m", tracked)
            == "crates/a/src/b.rs::T.m",
        ),
        ("a bare path is not a qualified name", relativize_qualified("crates/a/src/b.rs", tracked) is None),
    ]
    return _report("relativize", cases)


def _self_test_ranking() -> int:
    """The ranking and, more importantly, the untested-code rule."""
    # bca: suppress(abc) -- a case table; the score tracks coverage, not tangle
    churning = "crates/hot/src/a.rs"
    quiet = "crates/cold/src/b.rs"
    offenders = [
        _offender("cognitive", 30, 15, churning, "tangled"),
        _offender("cyclomatic", 20, 15, churning, "tangled"),
        _offender("cognitive", 60, 15, quiet, "worse_but_untouched"),
        _offender("cognitive", 30, 15, churning, "untested"),
        _offender("cognitive", 99, 15, "crates/hot/tests/it.rs", "in_a_test"),
        _offender("abc", 716, 40, churning, "flat_registration_table"),
        _offender("nargs", 9, 5, churning, "many_args"),
        _offender("cognitive", 80, 15, "scripts/codehealth/worklist.py", "the_ranker_itself"),
        _offender("cognitive", 80, 15, ".github/scripts/x.ts", "a_ci_script"),
    ]
    risk = {
        churning: 9.0,
        quiet: 0.1,
        "crates/hot/tests/it.rs": 9.9,
        "scripts/codehealth/worklist.py": 9.9,
        ".github/scripts/x.ts": 9.9,
    }
    facts = {
        (churning, "tangled"): Facts(tested_by=4, callers=12, indexed=True),
        (quiet, "worse_but_untouched"): Facts(tested_by=1, callers=1, indexed=True),
        (churning, "untested"): Facts(tested_by=0, callers=3, indexed=True),
    }
    items = rank(Sources(offenders, risk, facts), top=0)
    by_name = {item.name: item for item in items}
    cases = [
        ("test code excluded", "in_a_test" not in by_name),
        ("abc does not rank anything", "flat_registration_table" not in by_name),
        ("nargs does not rank anything", "many_args" not in by_name),
        (
            "the loop never assigns itself its own tooling",
            "the_ranker_itself" not in by_name,
        ),
        ("nor a CI script it may not change", "a_ci_script" not in by_name),
        (
            "a function with an open loop PR is skipped",
            "tangled" not in {
                item.name
                for item in rank(
                    Sources(offenders, risk, facts),
                    0,
                    frozenset({slug_of(churning, "tangled")}),
                )
            },
        ),
        (
            "but the rest of the list still ranks",
            rank(
                Sources(offenders, risk, facts), 0, frozenset({slug_of(churning, "tangled")})
            )[0].name
            == "untested",
        ),
        (
            "excluding nothing changes nothing",
            rank(Sources(offenders, risk, facts), 0, frozenset()) == rank(Sources(offenders, risk, facts), 0),
        ),
        ("cognitive and cyclomatic do rank", "tangled" in by_name),
        ("churning beats worse-but-quiet", items[0].name == "tangled"),
        ("metrics grouped per function", len(by_name["tangled"].metrics) == 2),
        ("worst metric leads the group", by_name["tangled"].metrics[0].metric == "cognitive"),
        ("score is risk times worst excess", abs(by_name["tangled"].score - 9.0 * 2.0) < 1e-9),
        ("tested function is a refactor", by_name["tangled"].kind == "refactor"),
        ("indexed function with no test gets write-tests", by_name["untested"].kind == "write-tests"),
        ("untested function has no tests", by_name["untested"].tested_by == 0),
        (
            "a function the graph never saw is prove-coverage, not write-tests",
            rank(Sources([_offender("cognitive", 30, 15, churning, "unseen")], risk, facts), 0)[0].kind
            == "prove-coverage",
        ),
        ("zero-risk files drop out", all(item.score > 0 for item in items)),
        ("top clamps the list", len(rank(Sources(offenders, risk, facts), top=1)) == 1),
    ]
    unranked = rank(Sources([_offender("cognitive", 30, 15, "crates/new/src/a.rs", "fresh")], {}, {}), top=0
    )
    cases.append(("a file with no history ranks nothing", unranked == []))
    return _report("ranking", cases)


def _self_test_facts_lookup() -> int:
    """The fallback that decides whether an item is `refactor` or `write-tests`."""
    facts = {
        ("a.rs", "T.method"): Facts(3, 7, True),
        ("a.rs", "loose"): Facts(1, 2, True),
        ("a.rs", "bare"): Facts(0, 0, True),
    }
    cases = [
        ("exact match", _lookup_facts(facts, "a.rs", "T.method") == Facts(3, 7, True)),
        ("bare-name fallback", _lookup_facts(facts, "a.rs", "Other.loose") == Facts(1, 2, True)),
        ("an indexed zero stays indexed", _lookup_facts(facts, "a.rs", "bare").indexed),
        ("a real miss is not indexed", not _lookup_facts(facts, "a.rs", "absent").indexed),
        ("a real miss reports no tests", _lookup_facts(facts, "a.rs", "absent").tested_by == 0),
        ("wrong file does not match", not _lookup_facts(facts, "b.rs", "T.method").indexed),
    ]
    return _report("facts lookup", cases)


def _self_test_briefs() -> int:
    """Both briefs must state their rule; the untested one is the load-bearing case."""
    tested = Item(
        "a.rs", "f", 1, 9, (_offender("cognitive", 30, 15, "a.rs", "f"),), 9.0, 4, 2, True, 18.0
    )
    untested = tested._replace(tested_by=0)
    unknown = tested._replace(tested_by=0, callers=0, indexed=False)
    tested_brief = as_brief([tested])
    untested_brief = as_brief([untested])
    unknown_brief = as_brief([unknown])
    cases = [
        ("tested brief allows a refactor", "behaviour-preserving" in tested_brief),
        ("tested brief warns about pointless splits", "no one's" in tested_brief),
        ("tested brief states the test count", "**4**" in tested_brief),
        ("untested brief forbids restructuring", "may not restructure" in untested_brief),
        ("untested brief asks for tests", "Write tests" in untested_brief),
        ("untested brief does not invite a refactor", "behaviour-preserving" not in untested_brief),
        ("unknown brief says the graph does not know", "never indexed" in unknown_brief),
        ("unknown brief demands proof first", "break it on purpose" in unknown_brief),
        ("unknown brief does not claim zero tests", "**0**" not in unknown_brief),
        ("a refactor brief offers the decline path", "bca: suppress" in tested_brief),
        ("a prove-coverage brief offers it too", "bca: suppress" in unknown_brief),
        (
            "a write-tests brief does not — there is nothing to decline yet",
            "bca: suppress" not in untested_brief,
        ),
        ("empty worklist says so", as_brief([]).startswith("No work item")),
        ("json round-trips", json.loads(as_json([tested]))[0]["kind"] == "refactor"),
        ("json hides counts it does not know", json.loads(as_json([unknown]))[0]["tested_by"] is None),
        ("text names the function", "a.rs:1 f" in as_text([tested])),
        ("text marks an unknown count", "tests   ?" in as_text([unknown])),
        ("mix counts by kind", _mix([tested, unknown]) == "2 item(s): 1 prove-coverage, 1 refactor"),
    ]
    return _report("briefs", cases)


def _fake_graph(path: Path, schema: str = "9") -> None:
    """A graph database with this one's real shape, including its stale rows.

    The columns and edge kinds are copied from the live schema (version 9), and
    the two absolute prefixes are the ones actually in it: the repo moved and
    nothing pruned the old rows.
    """
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        create table nodes (
            id integer primary key autoincrement, kind text not null, name text not null,
            qualified_name text not null unique, file_path text not null,
            line_start integer, line_end integer, language text, parent_name text,
            params text, return_type text, modifiers text, is_test integer default 0,
            file_hash text, extra text default '{}', updated_at real not null,
            signature text, community_id integer);
        create table edges (
            id integer primary key autoincrement, kind text not null,
            source_qualified text not null, target_qualified text not null,
            file_path text not null, line integer default 0, extra text default '{}',
            confidence real default 1.0, confidence_tier text default 'EXTRACTED',
            updated_at real not null);
        create table metadata (key text primary key, value text not null);
        """
    )
    now = "/now/crates/a/src/b.rs"
    stale = "/gone/crates/a/src/b.rs"
    nodes = [
        ("Function", "from_engine", f"{now}::T.from_engine", now, "T", 0),
        ("Function", "from_engine", f"{stale}::T.from_engine", stale, "T", 0),
        ("Function", "untested", f"{now}::untested", now, None, 0),
        ("Test", "it_works", f"{now}::it_works", now, None, 1),
        ("Function", "a_test_fn", f"{now}::a_test_fn", now, None, 1),
        # `Class` and `File` nodes also carry is_test = 0, so filtering on that
        # column alone would pull 5,613 non-functions into the join.
        ("Class", "T", f"{now}::T", now, None, 0),
        ("File", "b.rs", now, now, None, 0),
    ]
    conn.executemany(
        "insert into nodes (kind, name, qualified_name, file_path, parent_name, is_test, "
        "updated_at) values (?,?,?,?,?,?,0)",
        nodes,
    )
    edges = [
        # The same test, recorded under both prefixes: one test, not two.
        ("TESTED_BY", f"{now}::T.from_engine", f"{now}::it_works"),
        ("TESTED_BY", f"{stale}::T.from_engine", f"{stale}::it_works"),
        ("TESTED_BY", f"{now}::T.from_engine", f"{now}::other_test"),
        # The same caller twice is one caller.
        ("CALLS", f"{now}::caller_one", f"{now}::T.from_engine"),
        ("CALLS", f"{now}::caller_one", f"{now}::T.from_engine"),
        ("CALLS", f"{now}::caller_two", f"{now}::T.from_engine"),
        ("CALLS", f"{now}::caller_one", f"{now}::untested"),
        # An edge on a file git no longer tracks must not resurrect it.
        ("TESTED_BY", "/now/crates/a/src/deleted.rs::x", f"{now}::it_works"),
    ]
    conn.executemany(
        "insert into edges (kind, source_qualified, target_qualified, file_path, updated_at) "
        "values (?,?,?,'',0)",
        edges,
    )
    conn.execute("insert into metadata values ('schema_version', ?)", (schema,))
    conn.commit()
    conn.close()


def _self_test_graph() -> int:
    """The graph read, against a database with this one's schema.

    Without this the whole join can return zeros and every self-test still
    passes — and a zero here does not look like a bug, it looks like "nothing in
    this repo is tested", which routes every item to `write-tests` forever.
    """
    tracked = frozenset({"crates/a/src/b.rs"})
    with tempfile.TemporaryDirectory() as tmp:
        good = Path(tmp) / "graph.db"
        _fake_graph(good)
        facts = load_graph_facts(good, tracked)
        cases = [
            ("a tested method is found", ("crates/a/src/b.rs", "T.from_engine") in facts),
            (
                "duplicate prefixes count once",
                facts[("crates/a/src/b.rs", "T.from_engine")].tested_by == 2,
            ),
            (
                "a repeated caller counts once",
                facts[("crates/a/src/b.rs", "T.from_engine")].callers == 2,
            ),
            (
                "an untested function reads zero tests",
                facts[("crates/a/src/b.rs", "untested")] == Facts(0, 1, True),
            ),
            (
                "every function the graph holds is marked indexed",
                all(fact.indexed for fact in facts.values()),
            ),
            ("test nodes are skipped", ("crates/a/src/b.rs", "it_works") not in facts),
            ("is_test functions are skipped", ("crates/a/src/b.rs", "a_test_fn") not in facts),
            ("class nodes are skipped", ("crates/a/src/b.rs", "T") not in facts),
            ("file nodes are skipped", ("crates/a/src/b.rs", "b.rs") not in facts),
            ("deleted files are dropped", not any("deleted" in key[0] for key in facts)),
        ]
        wrong = Path(tmp) / "wrong.db"
        _fake_graph(wrong, schema="10")
        cases.append(("a schema bump fails loudly", _raises(lambda: load_graph_facts(wrong, tracked))))
        missing = Path(tmp) / "absent.db"
        cases.append(("a missing database fails loudly", _raises(lambda: load_graph_facts(missing, tracked))))
    return _report("graph", cases)


def _raises(call) -> bool:
    try:
        call()
    except ToolError:
        return True
    return False


def _self_test_affected() -> int:
    """Package resolution against this repo's own real layout."""
    root = repo_root(Path.cwd())
    cases = [
        ("crates/server is oxy-server", cargo_package(root, "crates/server/src/main.rs") == "oxy-server"),
        ("crates/core is plain oxy", cargo_package(root, "crates/core/src/lib.rs") == "oxy"),
        ("a nested module resolves to its crate", cargo_package(root, "crates/authz/src/lib.rs") == "oxy-authz"),
        ("a non-crate path has no package", cargo_package(root, "internal-docs/README.md") is None),
        (
            "a rust diff names its package",
            affected(root, ["crates/authz/src/lib.rs"]) == ["cargo-package oxy-authz"],
        ),
        (
            "two crates earn two runs, never --workspace",
            affected(root, ["crates/authz/src/lib.rs", "crates/core/src/lib.rs"])
            == ["cargo-package oxy", "cargo-package oxy-authz"],
        ),
        (
            "a web-app diff earns the web suite",
            affected(root, ["web-app/src/x.tsx"]) == ["web-app"],
        ),
        (
            "nothing it emits is a shell command",
            all(
                " " not in line.split(" ", 1)[-1]
                for line in affected(root, ["crates/authz/src/lib.rs", "web-app/src/x.tsx"])
            ),
        ),
        (
            "every package name is a bare identifier the caller can validate",
            all(
                line.removeprefix("cargo-package ").replace("-", "").replace("_", "").isalnum()
                for line in affected(root, ["crates/authz/src/lib.rs", "crates/core/src/lib.rs"])
            ),
        ),
        ("a docs-only diff earns nothing", affected(root, ["internal-docs/x.md"]) == []),
        (
            "a mixed diff earns both",
            len(affected(root, ["crates/authz/src/lib.rs", "web-app/src/x.tsx"])) == 2,
        ),
    ]
    return _report("affected", cases)


def _self_test_slug() -> int:
    """The branch slug, which is the key two runs a week apart agree on.

    It has to match what `codehealth/<slug>-<run id>` produces in the workflow,
    because that is where the next run reads it back from.
    """
    cases = [
        (
            "path and name together",
            slug_of("web-app/src/a.tsx", "getToolDisplay") == "web-app-src-a-gettooldisplay",
        ),
        (
            "a method",
            slug_of("crates/agentic/llm/src/c.rs", "LlmClient.run_with_tools")
            == "crates-agentic-llm-src-c-llmclient-run-with-tools",
        ),
        (
            "an anonymous space",
            slug_of("sdk/cli/src/main.ts", "buildProgram::<anon@L347>")
            == "sdk-cli-src-main-buildprogram-anon-l347",
        ),
        ("no leading or trailing dash", slug_of("a/b.rs", "<file>") == "a-b-file"),
        ("it is stable", slug_of("a/b.rs", "f") == slug_of("a/b.rs", "f")),
        # The bug this shape exists to prevent: a slug from the name alone would
        # make these equal, so one open PR would hide the other function.
        (
            "the same name in two files is two items",
            slug_of("web-app/src/a.ts", "handler") != slug_of("sdk/cli/src/b.ts", "handler"),
        ),
        (
            "the same method on two crates is two items",
            slug_of("crates/a/src/lib.rs", "T.go") != slug_of("crates/b/src/lib.rs", "T.go"),
        ),
        # No `--` anywhere, which is what lets the workflow split the run id back
        # off `codehealth/<slug>--<run id>`.
        ("never contains a double dash", "--" not in slug_of("a/b..c.rs", "f::<anon@L1>")),
        (
            "a name ending in a number keeps it",
            slug_of("a/b.rs", "step_2").endswith("step-2"),
        ),
        ("a dotless path keeps its last component", slug_of("a/justfile", "f") == "a-justfile-f"),
    ]
    return _report("slug", cases)


def _self_test_cli() -> int:
    """`--pick` and `--json-out`, against a stubbed ranking.

    Mechanical, but the workflow depends on both: `--pick` out of range must be
    exit 3 (a clean week) and not exit 0 with the wrong item, and `--json-out`
    has to land even when `--pick` narrows what is printed.
    """
    import worklist

    items = [
        Item("a.rs", "first", 1, 9, (_offender("cognitive", 30, 15, "a.rs", "first"),), 9.0, 1, 1, True, 18.0),
        Item("b.rs", "second", 1, 9, (_offender("cognitive", 20, 15, "b.rs", "second"),), 1.0, 1, 1, True, 1.3),
    ]
    original = worklist.collect
    worklist.collect = lambda root, top, exclude_slugs=frozenset(): items
    cases = []
    try:
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stdout(io.StringIO()) as out_text:
            out = Path(tmp) / "w.json"
            code = worklist.main(["--pick", "2", "--format", "json", "--json-out", str(out)])
            written = json.loads(out.read_text())
            printed = json.loads(out_text.getvalue())
            cases.append(("pick 2 succeeds", code == 0))
            cases.append(("pick 2 prints only that item", [i["name"] for i in printed] == ["second"]))
            cases.append(("json-out holds the whole list", len(written) == 2))
            cases.append(("json-out is ranked", written[0]["name"] == "first"))
            cases.append(("pick out of range is exit 3", worklist.main(["--pick", "3"]) == 3))
            cases.append(("pick 0 is exit 3, not the last item", worklist.main(["--pick", "0"]) == 3))
            cases.append(("no pick renders everything", worklist.main(["--format", "json"]) == 0))
    finally:
        worklist.collect = original
    return _report("cli", cases)


def _report(group: str, cases: list[tuple[str, bool]]) -> int:
    failed = [name for name, ok in cases if not ok]
    for name in failed:
        print(f"FAIL [{group}] {name}", file=sys.stderr)
    print(f"{group}: {len(cases) - len(failed)}/{len(cases)} passed", file=sys.stderr)
    return len(failed)


def self_test() -> int:
    failed = (
        _self_test_parsing()
        + _self_test_relativize()
        + _self_test_ranking()
        + _self_test_facts_lookup()
        + _self_test_briefs()
        + _self_test_graph()
        + _self_test_affected()
        + _self_test_cli()
        + _self_test_slug()
    )
    print("self-test: FAILED" if failed else "self-test: ok", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(self_test())
