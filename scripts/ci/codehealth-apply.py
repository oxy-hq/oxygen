#!/usr/bin/env python3
"""Apply a weekly-code-health patch, or refuse it.

The patch comes from an agent that ran in another job, so every claim about it
is the agent's claim. In particular the loop's first version validated a
`touched.txt` that the same job had written — it checked the attacker's own
manifest rather than the patch, and a patch could declare one innocuous path
while carrying an edit to `crates/<anything>/Cargo.toml`, which
`git apply --include='crates/*'` would have applied without complaint.

So the file list here comes from `git apply --numstat` and the file *kinds* from
`git apply --summary`, both read from the patch itself, before anything is
applied. What survives that is applied with `--include` as a second layer, and
the applied result is read back out of the index and checked again.

What is allowed: an edit to an existing file under `crates/`, `web-app/` or
`sdk/`, or a new ordinary file there, up to four of them. Nothing else — no
rename, no copy, no delete, no symlink, no mode change, and none of the files
that decide how the verification around this script runs.

    python3 scripts/ci/codehealth-apply.py <patch>        # validate, then apply
    python3 scripts/ci/codehealth-apply.py --self-test    # check the rules
"""

from __future__ import annotations

import re
import subprocess
import sys

# The trees the loop may change at all. `scripts/` is deliberately absent: the
# loop must not edit what verifies it, and worklist.py refuses to rank anything
# there so it is never assigned that work either.
ALLOWED = ("crates/", "web-app/", "sdk/")

# What the loop actually edits: source files in the three languages it ranks.
# This is an allow-list because the first version was a deny-list of config
# filenames — Cargo.toml, package.json, the lockfiles, the vitest configs — and
# a deny-list of configuration is unbounded. `crates/.cargo/config.toml`,
# `web-app/.env`, `sdk/cli/.pnpmfile.cjs` and `web-app/rust-toolchain.toml` all
# sit inside an allowed tree, all change what the verification runs, and none of
# them was on the list. An extension allow-list rules out every one of them, and
# every future one, by construction.
SOURCE_SUFFIXES = (".rs", ".ts", ".tsx")

# Files that carry an allowed extension and are still configuration.
CONFIG_BASENAME = re.compile(
    r"^(build\.rs|vite\.config\.[cm]?ts|vitest\.config\.[cm]?ts|vitest\.workspace\.[cm]?ts)$"
)

# One function per PR means one file, plus at most a test file or two.
MAX_FILES = 4

# The only mode a new file may have. 120000 is a symlink, 100755 is executable.
ORDINARY = "100644"

INCLUDES = [f"--include={prefix}*" for prefix in ALLOWED]


def parse_numstat(text: str) -> list[str]:
    """Paths from `git apply --numstat`: `<added>\\t<removed>\\t<path>`.

    A binary file's counts are `-`, which is why the counts are not parsed: the
    path is the only field this needs, and a format it cannot read must not be
    silently treated as an empty patch.
    """
    paths = []
    for line in text.splitlines():
        fields = line.split("\t")
        if len(fields) >= 3 and fields[2]:
            paths.append(fields[2].strip())
    return paths


def parse_summary(text: str) -> list[str]:
    """The non-empty lines of `git apply --summary`, stripped.

    Each is one of `create mode <mode> <path>`, `delete mode <mode> <path>`,
    `mode change <old> => <new> <path>`, `rename <...>` or `copy <...>`.
    """
    return [line.strip() for line in text.splitlines() if line.strip()]


def path_reason(path: str) -> str | None:
    """Why this path may not be in the patch, or None."""
    parts = path.split("/")
    # Both halves of this are subsumed, and mutation testing is how that is
    # known: an absolute path never starts with one of ALLOWED, and `..` is a
    # dot-prefixed component, so the two checks below already refuse everything
    # this one does and disabling it fails no case. It stays because a path
    # validator that does not visibly refuse traversal makes a reader prove the
    # absence, and the message it gives is the clearer one.
    if path.startswith("/") or ".." in parts:
        return f"not a plain repo-relative path: {path}"
    if not path.startswith(ALLOWED):
        return f"outside the trees this loop may change: {path}"
    # No dot-prefixed component anywhere. This is what keeps `crates/.cargo/`,
    # `web-app/.env` and `sdk/cli/.pnpmfile.cjs` out without naming them.
    if any(part.startswith(".") for part in parts):
        return f"a dot-prefixed path is configuration, not source: {path}"
    if not path.endswith(SOURCE_SUFFIXES):
        return f"not a source file this loop edits ({', '.join(SOURCE_SUFFIXES)}): {path}"
    if CONFIG_BASENAME.match(parts[-1]):
        return f"decides how verification runs, so it is off limits: {path}"
    return None


def summary_reason(line: str) -> str | None:
    """Why this `--summary` line may not be in the patch, or None.

    A rename's own paths are not checked, because a rename is refused outright:
    `--numstat` reports only its new path, so the old one would have to be
    recovered from this line to be validated at all, and a behaviour-preserving
    change to one function does not need to move a file.
    """
    if line.startswith(("rename ", "copy ")):
        return f"renames and copies are refused: {line}"
    if line.startswith("mode change"):
        return f"mode changes are refused: {line}"
    if line.startswith("delete mode"):
        return f"deleting a file is not a one-function change: {line}"
    if line.startswith("create mode"):
        mode = line.split()[2] if len(line.split()) > 2 else "?"
        if mode != ORDINARY:
            return f"a new file must be an ordinary file, not mode {mode}: {line}"
    return None


def violations(paths: list[str], summary: list[str]) -> list[str]:
    """Every reason to refuse this patch. Empty means it may be applied."""
    reasons = [reason for path in paths if (reason := path_reason(path))]
    reasons += [reason for line in summary if (reason := summary_reason(line))]
    if not paths:
        reasons.append("the patch changes no file")
    elif len(paths) > MAX_FILES:
        reasons.append(f"{len(paths)} files changed; the brief named one function")
    return reasons


def git(*args: str) -> str:
    proc = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise SystemExit(f"::error::git {' '.join(args[:3])} failed: {proc.stderr.strip()[:400]}")
    return proc.stdout


def main(argv: list[str]) -> int:
    if argv[:1] == ["--self-test"]:
        return _self_test()
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 1
    patch = argv[0]

    paths = parse_numstat(git("apply", "--numstat", patch))
    summary = parse_summary(git("apply", "--summary", patch))
    reasons = violations(paths, summary)
    if reasons:
        for reason in reasons:
            print(f"::error::{reason}")
        return 1

    # `--include` as a second layer: even a patch whose numstat disagrees with
    # its own hunks cannot write outside these trees. Hooks off because nothing
    # in this repo's hooks belongs in an apply.
    git("-c", "core.hooksPath=/dev/null", "apply", "--index", "--binary", *INCLUDES, patch)

    # And read back what actually landed, which is the only list that is not a
    # claim about the patch.
    applied = [line for line in git("diff", "--cached", "--name-only", "HEAD").splitlines() if line]
    after = violations(applied, [])
    if after:
        for reason in after:
            print(f"::error::after applying: {reason}")
        return 1
    for path in applied:
        print(f"applied: {path}")
    return 0


def _self_test() -> int:
    """Every case here was written against a mutation of the rule it checks."""
    # bca: suppress(abc) -- a case table; the score tracks coverage, not tangle
    numstat = "1\t0\tcrates/a/Cargo.toml\n1\t0\tcrates/a/src/added.rs\n1\t1\tcrates/a/src/lib.rs\n"
    cases: list[tuple[str, bool]] = [
        ("numstat paths parsed", parse_numstat(numstat)[0] == "crates/a/Cargo.toml"),
        ("all three paths parsed", len(parse_numstat(numstat)) == 3),
        ("a binary row still yields its path", parse_numstat("-\t-\tweb-app/a.png") == ["web-app/a.png"]),
        ("a malformed row is not a path", parse_numstat("garbage") == []),
        ("summary lines stripped", parse_summary(" create mode 100644 x\n\n") == ["create mode 100644 x"]),
        # The hole this script exists to close.
        ("a manifest inside an allowed tree is refused", path_reason("crates/a/Cargo.toml") is not None),
        ("so is a nested one", path_reason("crates/a/b/c/Cargo.toml") is not None),
        ("so is a vitest config", path_reason("web-app/vitest.config.ts") is not None),
        ("so is a nested vite config", path_reason("web-app/sub/vite.config.mts") is not None),
        ("so is a build script", path_reason("crates/server/build.rs") is not None),
        ("so is a lockfile", path_reason("web-app/pnpm-lock.yaml") is not None),
        # What a filename deny-list would have missed, all inside allowed trees.
        ("a nested .cargo config is refused", path_reason("crates/.cargo/config.toml") is not None),
        ("a .config dir is refused", path_reason("web-app/.config/nextest.toml") is not None),
        ("a dotfile is refused", path_reason("web-app/.env") is not None),
        ("a .pnpmfile is refused", path_reason("sdk/cli/.pnpmfile.cjs") is not None),
        ("a nested rust-toolchain is refused", path_reason("crates/rust-toolchain.toml") is not None),
        ("a .gitattributes is refused", path_reason("crates/.gitattributes") is not None),
        ("a yaml file is refused: not source", path_reason("crates/a/x.yml") is not None),
        ("a shell script is refused: not source", path_reason("crates/a/x.sh") is not None),
        ("a .js file is refused: this loop ranks rs/ts/tsx", path_reason("web-app/src/x.js") is not None),
        # These two carry an allowed extension, so only the dot-component rule
        # and the prefix rule respectively can refuse them. Both mutations that
        # disable those rules survived until these cases existed.
        (
            "a dot directory holding a .ts file is still refused",
            path_reason("web-app/.storybook/main.ts") is not None,
        ),
        ("a hidden rust file is refused", path_reason("crates/.hidden/evil.rs") is not None),
        (
            "a .ts file outside the allowed trees is refused",
            path_reason("scripts/ci/evil.ts") is not None,
        ),
        ("a .rs file in an unlisted tree is refused", path_reason("mobile/app/x.rs") is not None),
        ("ordinary source is allowed", path_reason("crates/authz/src/lib.rs") is None),
        ("a tsx file is allowed", path_reason("web-app/src/pages/x.tsx") is None),
        ("an sdk file is allowed", path_reason("sdk/cli/src/main.ts") is None),
        ("a new test file is allowed", path_reason("web-app/src/x.test.ts") is None),
        ("a rust test module is allowed", path_reason("crates/a/tests/integration/x.rs") is None),
        ("scripts/ is outside the allowed trees", path_reason("scripts/codehealth/worklist.py") is not None),
        ("so is .github/", path_reason(".github/workflows/ci.yaml") is not None),
        ("so is .cargo/config.toml", path_reason(".cargo/config.toml") is not None),
        ("an absolute path is refused", path_reason("/etc/passwd") is not None),
        ("a traversal is refused", path_reason("crates/../../etc/passwd") is not None),
        ("a dotdot component anywhere is refused", path_reason("crates/a/../../../x") is not None),
        (
            "a traversal that looks like source is refused",
            path_reason("crates/../../x.rs") is not None,
        ),
        ("a rename is refused", summary_reason("rename crates/a/{x.rs => y.rs} (100%)") is not None),
        ("a copy is refused", summary_reason("copy crates/a/{x.rs => y.rs} (100%)") is not None),
        ("a mode change is refused", summary_reason("mode change 100644 => 100755 crates/a/src/lib.rs") is not None),
        ("a delete is refused", summary_reason("delete mode 100644 crates/a/src/gone.rs") is not None),
        ("a symlink is refused", summary_reason("create mode 120000 crates/a/src/link.rs") is not None),
        ("an executable new file is refused", summary_reason("create mode 100755 crates/a/x.rs") is not None),
        ("an ordinary new file is allowed", summary_reason("create mode 100644 crates/a/x.rs") is None),
        ("an empty patch is refused", violations([], []) == ["the patch changes no file"]),
        (
            "five files are refused, by count",
            violations([f"crates/a/{i}.rs" for i in range(5)], [])
            == ["5 files changed; the brief named one function"],
        ),
        ("four files are allowed", violations([f"crates/a/{i}.rs" for i in range(4)], []) == []),
        (
            "the real three-file patch is refused for its Cargo.toml",
            len(violations(parse_numstat(numstat), [])) == 1,
        ),
        (
            "every include glob names an allowed tree",
            sorted(INCLUDES) == sorted(f"--include={p}*" for p in ALLOWED),
        ),
    ]
    failed = [name for name, ok in cases if not ok]
    for name in failed:
        print(f"FAIL {name}", file=sys.stderr)
    print(f"codehealth-apply: {len(cases) - len(failed)}/{len(cases)} passed", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
