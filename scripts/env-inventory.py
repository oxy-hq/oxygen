#!/usr/bin/env python3
"""Inventory of every `OXY_*` environment variable the Rust crates read.

    python3 -I scripts/env-inventory.py            # regenerate internal-docs/configuration.md
    python3 -I scripts/env-inventory.py --check    # exit 1 when the doc is stale or a variable
                                                   # has no entry in scripts/env-inventory.json
    python3 -I scripts/env-inventory.py --sites    # one "VARIABLE<TAB>file" line per read site

The scan and the guard test (`crates/app/tests/routing/env_reads/`) apply the same
rules, and the test runs `--sites` to prove it. Keep the two in step: a rule added
here is added there in the same PR.

What counts as a read site, in non-test code under `crates/**/src/**/*.rs`:

1. `env::var("OXY_X")` / `env::var_os("OXY_X")` (with or without `std::`).
2. A call to a function whose name contains `env` or `var` with an `"OXY_X"` literal
   as its first argument (`env_usize("OXY_X", 8)`, `require_var("OXY_X")`).
3. Either of the above through a named constant (`const GATE: &str = "OXY_X";`
   then `env::var(GATE)`), and, more broadly, a constant declared in a file that
   reads the environment at all: a gate read through a closure
   (`read(|name| std::env::var(name))`) is still a read of that file.

4. In the central config module (`crates/app-core/src/config/`) every `"OXY_X"`
   literal: that module exists to read them, through a lookup it is handed.

Test code is left out: files named `*_tests.rs` / `*_test.rs` / `tests.rs` and every
item under a `#[cfg(test)]` (or `cfg(any(test, ..))`) attribute. Comments and string
contents are lexed first so a brace in a string cannot unbalance the item skip.

Annotations (kind, default, what it gates) come from `scripts/env-inventory.json`;
a variable the scan finds but the file does not name fails `--check`, so the table
can never carry an undocumented variable.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

DOC = Path("internal-docs/configuration.md")
ANNOTATIONS = Path("scripts/env-inventory.json")
KINDS = {
    "rollout": "rollout gate (temporary)",
    "deployment": "deployment config",
    "developer": "developer override",
}
SKIP_NAME = re.compile(r"(_tests?\.rs|^tests\.rs)$")
CENTRAL = "crates/app-core/src/config/"
SKIP_CRATES = {"workspace-hack"}

VAR = r"OXY_[A-Z0-9_]+"
LITERAL = re.compile(rf'"({VAR})"')
CONST_DECL = re.compile(rf'\bconst\s+([A-Z_][A-Z0-9_]*)\s*:\s*&(?:\'static\s+)?str\s*=\s*"({VAR})"')
DIRECT = re.compile(rf'\benv::var(?:_os)?\(\s*"({VAR})"')
DIRECT_CONST = re.compile(r"\benv::var(?:_os)?\(\s*([A-Z_][A-Z0-9_]*)\s*\)")
HELPER = re.compile(rf'\b([A-Za-z_][A-Za-z0-9_]*)\(\s*"({VAR})"')
HELPER_CONST = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\(\s*([A-Z_][A-Z0-9_]*)\s*[,)]")
ANY_ENV_CALL = re.compile(r"\benv::var(?:_os)?\(")
CFG_ATTR = re.compile(r"#\[cfg\(([^\]]*)\)\]")
TEST_CFG = re.compile(r"(?<!not\()\btest\b")


def is_env_helper(ident: str) -> bool:
    """A function that reads the environment for its caller: `env_usize`,
    `require_var`, `parse_env_days`. Not the std functions the direct rule
    covers, and not the writers (`set_var`, `remove_var`)."""
    lower = ident.lower()
    return ("env" in lower or "var" in lower) and ident not in (
        "var",
        "var_os",
        "set_var",
        "remove_var",
    )


# ── Lexing ────────────────────────────────────────────────────────────────


def lex(src: str) -> str:
    """One kind per character: `c` code, `s` string/char literal, `m` comment."""
    kinds = []
    i, n = 0, len(src)
    while i < n:
        ch = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if ch == "/" and nxt == "/":
            j = src.find("\n", i)
            j = n if j < 0 else j
            kinds.append("m" * (j - i))
            i = j
        elif ch == "/" and nxt == "*":
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            kinds.append("m" * (j - i))
            i = j
        elif ch == '"' or (ch in "brc" and nxt == '"'):
            j = lex_string(src, i)
            kinds.append("s" * (j - i))
            i = j
        elif (ch == "r" or (ch in "bc" and nxt == "r")) and raw_string_start(src, i):
            j = lex_raw_string(src, i)
            kinds.append("s" * (j - i))
            i = j
        elif ch == "'" and char_literal_end(src, i) is not None:
            j = char_literal_end(src, i)
            kinds.append("s" * (j - i))
            i = j
        else:
            kinds.append("c")
            i += 1
    return "".join(kinds)


def lex_string(src: str, i: int) -> int:
    j = src.index('"', i) + 1
    n = len(src)
    while j < n:
        if src[j] == "\\":
            j += 2
        elif src[j] == '"':
            return j + 1
        else:
            j += 1
    return n


def raw_string_start(src: str, i: int) -> bool:
    j = i + (1 if src[i] == "r" else 2)
    while j < len(src) and src[j] == "#":
        j += 1
    return j < len(src) and src[j] == '"'


def lex_raw_string(src: str, i: int) -> int:
    j = i + (1 if src[i] == "r" else 2)
    hashes = 0
    while src[j] == "#":
        hashes, j = hashes + 1, j + 1
    close = '"' + "#" * hashes
    end = src.find(close, j + 1)
    return len(src) if end < 0 else end + len(close)


def char_literal_end(src: str, i: int):
    """End of a char literal starting at `i`, or None for a lifetime (`'a`)."""
    n = len(src)
    if i + 1 >= n:
        return None
    if src[i + 1] == "\\":
        j = src.find("'", i + 2)
        return None if j < 0 else j + 1
    if i + 2 < n and src[i + 2] == "'":
        return i + 3
    return None


def blank(src: str, kinds: str, drop: str) -> str:
    return "".join(c if (k not in drop or c == "\n") else " " for c, k in zip(src, kinds))


# ── Test-code removal ─────────────────────────────────────────────────────


def strip_test_items(src: str) -> str:
    """`src` with comments blanked and every `#[cfg(test)]` item blanked."""
    kinds = lex(src)
    search = blank(src, kinds, "ms")  # for structure: no strings, no comments
    scan = list(blank(src, kinds, "m"))  # for matching: strings kept
    for attr in CFG_ATTR.finditer(search):
        if not TEST_CFG.search(attr.group(1)):
            continue
        end = item_end(search, attr.end())
        for k in range(attr.start(), end):
            if scan[k] != "\n":
                scan[k] = " "
    return "".join(scan)


def item_end(search: str, i: int) -> int:
    """End of the item that starts after the attribute ending at `i`."""
    n = len(search)
    while i < n:
        while i < n and search[i].isspace():
            i += 1
        if search.startswith("#[", i):  # a further attribute
            i = bracket_end(search, i + 1, "[", "]")
            continue
        break
    brace = search.find("{", i)
    semi = search.find(";", i)
    if semi >= 0 and (brace < 0 or semi < brace):
        return semi + 1
    if brace < 0:
        return n
    return bracket_end(search, brace, "{", "}")


def bracket_end(search: str, i: int, open_: str, close: str) -> int:
    depth = 0
    for j in range(i, len(search)):
        if search[j] == open_:
            depth += 1
        elif search[j] == close:
            depth -= 1
            if depth == 0:
                return j + 1
    return len(search)


# ── Sites ─────────────────────────────────────────────────────────────────


def source_files(root: Path):
    crates = root / "crates"
    for dirpath, dirnames, filenames in os.walk(crates):
        rel = Path(dirpath).relative_to(crates)
        if rel.parts and rel.parts[0] in SKIP_CRATES:
            dirnames[:] = []
            continue
        if "src" not in rel.parts:
            continue
        for name in sorted(filenames):
            if name.endswith(".rs") and not SKIP_NAME.search(name):
                yield Path(dirpath) / name


def sites_in(text: str, central: bool = False) -> set[str]:
    if central:
        return {m.group(1) for m in LITERAL.finditer(text)}
    found = {m.group(1) for m in DIRECT.finditer(text)}
    found |= {m.group(2) for m in HELPER.finditer(text) if is_env_helper(m.group(1))}
    consts = {m.group(1): m.group(2) for m in CONST_DECL.finditer(text)}
    if consts:
        reads_env = bool(ANY_ENV_CALL.search(text))
        for m in DIRECT_CONST.finditer(text):
            if m.group(1) in consts:
                found.add(consts[m.group(1)])
        for m in HELPER_CONST.finditer(text):
            if is_env_helper(m.group(1)) and m.group(2) in consts:
                found.add(consts[m.group(2)])
        if reads_env:
            found |= set(consts.values())
    return found


def scan(root: Path) -> dict[str, set[str]]:
    """variable → the files (repo-relative, `/`-separated) that read it."""
    by_var: dict[str, set[str]] = {}
    for path in source_files(root):
        text = strip_test_items(path.read_text(encoding="utf-8"))
        rel = path.relative_to(root).as_posix()
        for var in sites_in(text, central=rel.startswith(CENTRAL)):
            by_var.setdefault(var, set()).add(rel)
    return by_var


# ── Rendering ─────────────────────────────────────────────────────────────

HEADER = """\
# Configuration: every `OXY_*` variable the server reads

<!-- GENERATED by scripts/env-inventory.py from the Rust sources and
     scripts/env-inventory.json. Do not edit by hand: run
     `python3 -I scripts/env-inventory.py` and commit the result. The guard test
     `crates/app/tests/routing/env_reads/` fails when this file is stale. -->

One table, three kinds of variable, and the rule that keeps the table from growing
back into the code: **a new `OXY_*` read goes through `oxy_app_core::config`**
(`crates/app-core/src/config/`), the one struct parsed at boot, with its default and
its validation in one place. A read anywhere else fails the guard unless it is in the
backlog (`env_reads/allowed.rs`), which shrinks and never grows.

| Kind | Meaning | Lifetime |
| --- | --- | --- |
| rollout gate (temporary) | Switches a behaviour on for one deployment at a time while it is proven. The intended end state is the behaviour on and the gate gone. Three of these are now feature-flag registry entries with a `sunset` (`internal-docs/feature-flags.md`, "Rollout gates and their sunset"); the env var is a deprecated override for one release. | Deleted when the sunset passes |
| deployment config | Describes the deployment: roles, URLs, zones, bucket names, intervals, limits. | Permanent |
| developer override | Changes a local process for a developer or a test: a knob nobody sets in production. | Permanent, local only |

"Read in" names the files that read the variable. The default and what the variable
gates are from `scripts/env-inventory.json`, which the author of a new variable edits
in the PR that adds the read.

"""


def render(by_var: dict[str, set[str]], notes: dict) -> str:
    lines = [HEADER]
    counts = {k: 0 for k in KINDS}
    for var in by_var:
        counts[notes[var]["kind"]] += 1
    lines.append(
        f"**{len(by_var)} variables**: {counts['rollout']} rollout gates, "
        f"{counts['deployment']} deployment config, {counts['developer']} developer overrides.\n"
    )
    lines.append("| Variable | Kind | Read in | Default | What it gates |")
    lines.append("| --- | --- | --- | --- | --- |")
    for var in sorted(by_var):
        note = notes[var]
        files = "<br>".join(f"`{f}`" for f in sorted(by_var[var]))
        lines.append(
            f"| `{var}` | {KINDS[note['kind']]} | {files} | {cell(note['default'])} | "
            f"{cell(note['gates'])} |"
        )
    lines.append("")
    return "\n".join(lines)


def cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ").strip() or "—"


# ── Entry ─────────────────────────────────────────────────────────────────


def load_notes(root: Path) -> dict:
    return json.loads((root / ANNOTATIONS).read_text(encoding="utf-8"))


def check_annotations(by_var: dict[str, set[str]], notes: dict) -> list[str]:
    problems = []
    for var in sorted(set(by_var) - set(notes)):
        problems.append(
            f"{var} is read ({', '.join(sorted(by_var[var]))}) but has no entry in "
            f"{ANNOTATIONS}: add {{\"kind\", \"default\", \"gates\"}} for it"
        )
    for var in sorted(set(notes) - set(by_var)):
        problems.append(f"{var} is in {ANNOTATIONS} but nothing reads it any more: delete its entry")
    for var, note in notes.items():
        if var in by_var and note.get("kind") not in KINDS:
            problems.append(f"{var}: kind must be one of {sorted(KINDS)}")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=".", help="repository root (default: cwd)")
    parser.add_argument("--check", action="store_true", help="fail if the doc is stale")
    parser.add_argument("--sites", action="store_true", help="print VARIABLE<TAB>file lines")
    args = parser.parse_args()
    root = Path(args.root).resolve()
    by_var = scan(root)
    if args.sites:
        for var in sorted(by_var):
            for f in sorted(by_var[var]):
                print(f"{var}\t{f}")
        return 0
    notes = load_notes(root)
    problems = check_annotations(by_var, notes)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 2
    rendered = render(by_var, notes)
    doc = root / DOC
    if args.check:
        current = doc.read_text(encoding="utf-8") if doc.exists() else ""
        if current != rendered:
            print(
                f"{DOC} is stale: run `python3 -I scripts/env-inventory.py` and commit it",
                file=sys.stderr,
            )
            return 1
        return 0
    doc.write_text(rendered, encoding="utf-8")
    print(f"wrote {DOC} ({len(by_var)} variables)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
