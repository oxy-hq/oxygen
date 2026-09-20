#!/usr/bin/env python3
"""Seed a checkout's target/ with artifacts hardlinked from a warm checkout.

A fresh checkout costs ~418s for `cargo check -p oxy-app --lib`. Nearly all of
that work has already been done in a sibling checkout, so hardlink it across.

WHAT THIS BUYS, AND WHAT IT DOES NOT. It removes the dependency graph for good:
a cold build is 88% registry deps, you never edit those, so they never come
back. It does NOT hand over a warm edit loop -- rustc's incremental cache is
per-crate and bound to the absolute path it was built at, so the first edit to
each crate still compiles that crate from scratch (~61s for oxy-app, the worst
case here). Judge a seed by that first edit. The settle-only numbers -- 0.76s at
the same commit, ~27s from a branch carrying 98 changed crate sources -- only
assert that the fingerprints matched; nobody makes a worktree to run a no-op.
See internal-docs/rust-build-performance.md.

    python3 scripts/seed-target.py              # auto-pick the best warm checkout
    python3 scripts/seed-target.py ../other     # or name one explicitly

SETTLE THE SOURCE WITH THE COMMAND YOU WILL RUN IN THE DESTINATION. Cargo's
-C metadata includes the resolved FEATURE SET, so one crate under two feature
unions is two units with two filenames, and only the one the source built is
seeded. `cargo check --tests`/nextest unify dev-dependency features in; a plain
`cargo build` does not. Measured: a tree seeded from a check-warm source settles
`cargo check -p oxy-app --lib` in 2.7s and then spends 7m20s on
`cargo build --bin oxy`, compiling 216 crates -- ~170 of them third-party, all
of them units nobody in the chain had ever built. Nothing is stale; the seeded
units are simply never addressed. So if you build the binary, build it in the
source too. See internal-docs/rust-build-performance.md.

THE SOURCE MUST BE SETTLED -- `cargo check` in it already a no-op. Seeding
reproduces the source's state faithfully, an unsettled one included, and the
destination then pays that settling cost itself (measured: 18s from a source
mid-rebuild, where a settled one gave 0.76s). If a seeded build is unexpectedly
slow, run `cargo check` in the source, let it finish, and re-seed -- a tree
holding nothing but seeded artifacts is re-seeded in place, replacing them
wholesale, with no `cargo clean` needed. Once you have compiled something here
yourself that stops being safe and the script says so; see write_marker below.

WORKSPACE CRATES ARE SHARED TOO, and this is the bulk of the win. An earlier
version of this script excluded them on the belief that cargo derives a path
package's `-C metadata` from its ABSOLUTE path. That is false for crates inside
the workspace: `SourceId::stable_hash` strips the workspace-root prefix, so the
hash is workspace-RELATIVE and two checkouts produce byte-identical artifact
names. (Verified on cargo 1.98.1: the same workspace at two paths emits
`-C metadata=36fa3f2e909640ae` from both. The absolute path only leaks in for a
path dependency OUTSIDE the workspace root, where the strip_prefix fails — which
is what the original experiment must have measured.)

MTIMES ARE THE REAL BLOCKER, and normalize_mtimes() below is what removes it.
A fresh checkout stamps every file with the checkout time, newer than the seeded
artifacts, so cargo treats every path crate as dirty and the seeding is wasted.
Restoring the source checkout's mtime for byte-identical files fixes it. Read
that function before changing it: the content match is a correctness
requirement, not an optimisation.

HOW MUCH YOU GET depends on THIRD-PARTY dependency drift, not on how much code
differs — source churn only touches workspace crates, which are cheap to redo.
A deep dependency bump cascades: measured against a sibling differing by 6
third-party versions, 27s; against one differing by 29, 389s — barely better
than a cold 418s. That is why this script scores candidate sources by
third-party lockfile overlap and picks the closest, and why the printed
"N differ" line is worth a glance before trusting the result.

ONLY deps/ IS HARDLINKED, AND NOT ALL OF IT. The compiler outputs there
(.rlib/.rmeta/binaries) are write-once -- rustc writes a temp file and renames,
so a rebuild breaks the link instead of writing through it -- and that is where
the size is (9.4G against build/'s 1.3G). deps/*.d is the exception and is
copied: dep-info is a plain create+truncate, so a rebuild in the destination
rewrites the SOURCE's .d through the shared inode (verified: same inode, link
count unchanged, contents replaced with the destination's paths). Cargo reads
.fingerprint/dep-* rather than deps/*.d for freshness, so nothing miscompiles
either way, but 26M is not worth leaving a write-through into another checkout.

.fingerprint/ and build/ are copied outright, because cargo rewrites both in
place and a shared inode would let a build in one checkout corrupt the other --
measured for .fingerprint/, the source went from a ~1s no-op to 24s of rework.
build/ is the same hazard: a build-script rerun truncates output / root-output /
invoked.timestamp and rewrites out/, and reruns really do happen here. So
seeding costs ~1.3G per checkout rather than nothing, and takes ~9s rather
than ~2s.

COPYING build/ IS NOT ENOUGH ON ITS OWN -- build-script stdout bakes in absolute
paths, and PathRewriter below is what unbakes them. See its docstring.
"""

import argparse
import errno
import filecmp
import json
import os
import re
import shutil
import subprocess
import sys
import time

LOCK_ENTRY = re.compile(r'name = "([^"]+)"\nversion = "([^"]+)"')


def run_metadata(root):
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        sys.exit(f"cargo metadata failed in {root}:\n{out.stderr.strip()}")
    return {p["name"] for p in json.loads(out.stdout)["packages"]}


def third_party_lock(checkout, members):
    """{(name, version)} for non-workspace packages, or None if unreadable.

    Keyed on the pair, not name -> version: this lockfile has 1288 entries under
    1083 distinct names, and the 205 duplicates are exactly the deep crates whose
    drift is catastrophic (arrow, arrow-array, hashbrown, ahash). Collapsing them
    let a sibling that matches on arrow 56 but differs on arrow 54 score as a
    perfect match, so pick_source could confidently choose a 389s source over a
    27s one.
    """
    path = os.path.join(checkout, "Cargo.lock")
    try:
        with open(path) as fh:
            entries = LOCK_ENTRY.findall(fh.read())
    except OSError:
        return None
    return {(n, v) for n, v in entries if n not in members}


def crate_name(package):
    """Package name as it appears in artifact filenames: oxy-app -> oxy_app."""
    return package.replace("-", "_")


def is_warm(checkout, profile):
    return os.path.isdir(os.path.join(checkout, "target", profile, "deps"))


def has_artifacts(checkout, profile):
    """True if this checkout has already compiled something of its own."""
    fp = os.path.join(checkout, "target", profile, ".fingerprint")
    try:
        return any(os.scandir(fp))
    except OSError:
        return False


def has_dynamic(checkout, profile):
    """True if this checkout has already built the dev-dynamic dylib
    (`oxy-app-dylib`). Seeding from it carries the ~1.4 GB `liboxy_app_dylib.dylib`
    via hardlink, so a `cargo build --features dev-dynamic` in the destination
    reuses it instead of paying the ~20 min link. See `just build-backend-dyn`."""
    # `.dylib` on macOS, `.so` on Linux — check both so `--dynamic` works on either.
    deps = os.path.join(checkout, "target", profile, "deps")
    return any(
        os.path.isfile(os.path.join(deps, f"liboxy_app_dylib{ext}"))
        for ext in (".dylib", ".so")
    )


SEED_MARKER = ".seeded-from"


def marker_path(checkout, profile):
    return os.path.join(checkout, "target", profile, SEED_MARKER)


def read_marker(checkout, profile):
    """What seed() recorded here, or None if this tree was never seeded."""
    try:
        with open(marker_path(checkout, profile)) as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def write_marker(checkout, profile, src):
    """Record that everything in this target/ came from a seed, and when.

    This is what separates the two warm destinations that has_artifacts() cannot
    tell apart. A tree that has only ever held SEEDED artifacts is safe to seed
    again -- replacing them all leaves it a faithful copy of the new source,
    which is exactly what a first seed produces -- and re-seeding is the
    recovery the module docstring prescribes for a source that turned out to be
    unsettled. Without a marker the first seed made itself the last: it fills
    .fingerprint/, so has_artifacts() is true from then on and every later run
    was refused, with `cargo clean` (throwing the seed away) the only way out.

    Written last, after seed() and normalize_mtimes(), so its timestamp is above
    every mtime this run produced and built_since_seed() can compare against it.
    """
    path = marker_path(checkout, profile)
    tmp = path + ".tmp"
    with open(tmp, "w") as fh:
        json.dump({"source": os.path.abspath(src), "at": time.time()}, fh)
    os.replace(tmp, path)


def built_since_seed(checkout, profile, marker, members):
    """Workspace units this checkout compiled itself after it was last seeded.

    Empty is the licence to re-seed. A non-empty answer is precisely the case
    the warm-destination refusal exists for: normalize_mtimes() would backdate
    that unit's sources against the source checkout and could put them below an
    artifact built HERE from older content, marking a stale unit fresh.

    Only workspace units count. A third-party unit built here was built from
    registry sources that are byte-identical in every checkout, so no amount of
    backdating can make its fingerprint lie.

    invoked.timestamp is excluded: cargo rewrites it whenever it *runs* a build
    script's unit, fresh or not, so counting it would report a plain no-op
    `cargo check` as a local build and refuse every re-seed.
    """
    at = marker.get("at")
    if at is None:
        return ["<marker unreadable>"]
    out = []
    try:
        units = list(
            os.scandir(os.path.join(checkout, "target", profile, ".fingerprint"))
        )
    except OSError:
        return out
    for unit in units:
        if not unit.is_dir() or not is_workspace_unit(unit.name, members):
            continue
        try:
            newest = max(
                (
                    f.stat().st_mtime
                    for f in os.scandir(unit.path)
                    if f.name != "invoked.timestamp"
                ),
                default=0,
            )
        except OSError:
            continue
        if newest > at:
            out.append(unit.name)
    return out


def first_party_differing(src, dst, rust_files):
    """How many of dst's crate sources differ from src's. Lower is better."""
    n = 0
    for rel in rust_files:
        s, d = os.path.join(src, rel), os.path.join(dst, rel)
        try:
            if os.stat(s).st_size != os.stat(d).st_size or not filecmp.cmp(
                s, d, shallow=False
            ):
                n += 1
        except OSError:
            n += 1
    return n


def pick_source(dest, members, profile, prefer_dynamic=False):
    """Choose the warm sibling closest on third-party deps, then on our own sources.

    Both axes matter and they are not interchangeable, so they are ranked
    lexicographically rather than blended. A third-party mismatch is
    catastrophic -- deep crates like sea-orm/sqlx cascade through the whole
    graph, and a sibling differing by 29 versions measured 389s against 27s for
    one differing by 6. So third-party overlap dominates absolutely.

    Underneath it, first-party distance is what sets the residual: seeding
    leaves exactly the crates whose sources differ, plus everything downstream.
    A sibling carrying 86 changed `entity` files costs a near-full rebuild
    because `entity` is the root of the graph, even though the checkout itself
    changed no Rust at all. Ranking on it means a checkout tracking `main` wins
    over a colleague's feature branch, which is what you want.
    """
    mine = third_party_lock(dest, members)
    if mine is None:
        sys.exit(f"no Cargo.lock in {dest}")
    dst_abs = os.path.abspath(dest)
    tracked = git_tracked(dst_abs) or []
    rust_files = [p for p in tracked if p.endswith(".rs") and p.startswith("crates/")]
    mine_repo = repo_identity(dst_abs)
    ranked = []
    for cand in candidate_dirs(dst_abs):
        if not is_warm(cand, profile):
            continue
        # Same repo only. A sibling checkout of an unrelated Rust project has a
        # Cargo.lock and a warm target/, so it is otherwise eligible — and with
        # nothing else around it would be picked, seeding foreign artifacts and
        # backdating our sources against its tree.
        theirs_repo = repo_identity(cand)
        if mine_repo is not None and not (theirs_repo or set()) & mine_repo:
            continue
        theirs = third_party_lock(cand, members)
        if theirs is None:
            continue
        shared = len(mine & theirs)
        differing = first_party_differing(cand, dst_abs, rust_files)
        # Final tie-break on how recently the source was built: the freshest
        # target/ is the one most likely to be complete rather than half-populated.
        built = os.path.getmtime(os.path.join(cand, "target", profile, "deps"))
        # With --dynamic, prefer a source that already carries the dev-dynamic
        # dylib — but only as a tie-break BELOW dep/source matching, which stays
        # correctness-critical: the seeded dylib's fingerprint is reusable only
        # when third-party deps AND oxy-app source already match.
        dyn = 1 if (prefer_dynamic and has_dynamic(cand, profile)) else 0
        ranked.append((shared, -differing, dyn, built, cand))
    if not ranked:
        sys.exit(
            f"no warm checkout of this repo found near {dst_abs} — build one "
            f"first, or name a source explicitly"
        )
    ranked.sort(reverse=True)
    best = ranked[0][4]
    for shared, neg_differing, _dyn, _built, cand in ranked:
        mark = "->" if cand == best else "  "
        print(
            f"  {mark} {os.path.basename(cand)}: {len(mine) - shared} third-party "
            f"deps differ, {-neg_differing} crate sources differ"
        )
    return best


def repo_identity(checkout):
    """Root commits reachable from HEAD -- "same repo", not "same clone".

    Keyed on history rather than the shared .git dir so that a second *clone*
    beside a warm one still qualifies: same sources, same lockfile, its own
    .git, which is the ideal source. --git-common-dir rejected it, so
    `just seed-target` in a fresh clone could only print "no warm checkout of
    this repo found" -- and a fresh clone is exactly where you want this.

    A set, compared by intersection: a repo with merged histories has several
    root commits, and which of them a given branch reaches varies. ~170ms per
    checkout on this repo, run once per candidate.
    """
    out = subprocess.run(
        ["git", "rev-list", "--max-parents=0", "HEAD"],
        cwd=checkout,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        return None
    roots = set(out.stdout.split())
    return roots or None


def candidate_dirs(dst_abs):
    """Sibling checkouts, plus each enclosing directory itself.

    Two conventions are in play. These checkouts sit side by side under
    ~/workspace, but `oxy`'s own tooling puts them in <repo>/.worktrees/<branch>
    (crates/git/src/cli/worktree.rs), where the only siblings are other
    worktrees and the warm main checkout is the *grandparent*. Scanning both
    levels covers each without the caller having to care.

    The bases themselves are candidates, not just their entries: listing
    <repo> yields <repo>/crates, <repo>/web-app, <repo>/.worktrees … and never
    <repo>, which under the .worktrees layout is the one checkout guaranteed to
    be warm. Omitting them made that layout fall back to sibling worktrees, or
    to "no warm checkout of this repo found".
    """
    seen, out = {dst_abs}, []
    parent = os.path.dirname(dst_abs)
    for base in (parent, os.path.dirname(parent)):
        for cand in [base] + [os.path.join(base, n) for n in _listdir(base)]:
            if cand in seen or not os.path.isdir(cand):
                continue
            seen.add(cand)
            out.append(cand)
    return out


def _listdir(path):
    try:
        return sorted(os.listdir(path))
    except OSError:
        return []


FINGERPRINT_UNIT = re.compile(r"^(.+)-[0-9a-f]{16}$")


def is_workspace_unit(dirname, members):
    """.fingerprint/ holds one dir per unit, named `<package>-<16 hex>`.

    Note the HYPHEN, the opposite of is_workspace_artifact below: the two trees
    use different conventions. deps/ names files after the *target*
    (`liboxy_app-<hash>.rlib`), .fingerprint/ after the *package*
    (`oxy-app-<hash>/`, `agentic-airway-<hash>/`). Matching underscored names
    here missed all 11 `agentic-*` members outright, and skipped the other
    hyphenated ones only by accident -- bare member `oxy` supplied a prefix that
    happens to cover `oxy-app-…`, `oxy-shared-…`, which would equally have
    swallowed any third-party `oxy-*` crate. Hence a full name match against the
    hash-anchored prefix rather than startswith().
    """
    m = FINGERPRINT_UNIT.match(dirname)
    return m is not None and m.group(1) in members


def is_workspace_artifact(filename, members):
    """deps/ entries look like `liboxy_app-<hash>.rlib` or `oxy_app-<hash>.d`.

    Note the underscore: `cargo metadata` says `oxy-app`, the artifact says
    `oxy_app`. Comparing the two directly matched only the 5 of 44 members with
    no hyphen, which made --no-workspace-crates a near no-op and would have made
    any A/B run with it measure the shared-workspace behaviour instead.
    """
    stem = filename[3:] if filename.startswith("lib") else filename
    return stem.split("-")[0] in {crate_name(m) for m in members}


def checkout_root(path):
    """Resolve a path to the checkout root it sits in.

    --dest defaults to the cwd, and the docstring above tells people to run the
    script by hand, so the cwd is often a subdirectory. Seeding one produced
    crates/app/target/debug/ — a 1.3G tree cargo never looks at — while
    `git ls-files` returned paths relative to *that* directory, so every
    join(src, rel) missed, normalize_mtimes() backdated nothing and reported
    everything as "not in source", and the warm-destination refusal looked in
    the wrong place too. Silent in every one of those, hence resolving here
    rather than warning.
    """
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        cwd=path,
        capture_output=True,
        text=True,
    )
    root = out.stdout.strip()
    return (
        os.path.abspath(root) if out.returncode == 0 and root else os.path.abspath(path)
    )


def git_tracked(checkout):
    out = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=checkout,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        return None
    return [p for p in out.stdout.split("\0") if p]


def normalize_mtimes(src, dst):
    """Give dst's unchanged sources the mtime they carry in src.

    A fresh checkout stamps every file with the checkout time, which is newer
    than the artifacts we just seeded, so cargo sees every path crate as dirty
    and the seeded workspace artifacts go to waste. Restoring the *source*
    checkout's mtime -- and only where the content is byte-identical -- puts
    back exactly the ordering cargo saw when it built those artifacts.

    This is content-matched on purpose. Blanket-backdating the whole tree would
    also mark genuinely-changed files as old, and cargo would then skip
    rebuilding crates that really did change: a silently stale build. Here a
    file that differs keeps its fresh mtime, so its crate rebuilds, as does
    everything downstream of it.
    """
    tracked = git_tracked(dst)
    if tracked is None:
        print("  (dest is not a git checkout — skipping mtime normalization)")
        return
    matched = differ = absent = 0
    for rel in tracked:
        s, d = os.path.join(src, rel), os.path.join(dst, rel)
        try:
            ss, ds = os.stat(s), os.stat(d)
        except OSError:
            absent += 1
            continue
        if ss.st_size != ds.st_size or not filecmp.cmp(s, d, shallow=False):
            differ += 1
            continue
        # ns=, not a float pair: cargo records build-script input mtimes at full
        # nanosecond precision, and st_mtime as a float rounds to ~100ns at
        # current epoch values. A 61ns drift on one duckdb_pool.rs was enough to
        # rerun oxy's build script and cascade UnitDependencyInfoChanged through
        # every crate downstream of it — 74s instead of 1s.
        os.utime(d, ns=(ss.st_atime_ns, ss.st_mtime_ns))
        matched += 1
    print(
        f"  mtimes: {matched} files matched source and were backdated, "
        f"{differ} differ (left dirty), {absent} not in source"
    )


# Files under build/ larger than this are native artifacts (.a/.o/.so) and are
# copied verbatim. Anything a build script emits for cargo or for an include! is
# far below it — the largest carrying a path here is openssl-sys's 1.2M output,
# and nothing over 4M carries one at all.
REWRITE_MAX_BYTES = 4 << 20


class PathRewriter:
    """Repoints absolute paths in copied build/ files at the destination.

    Copying build/ verbatim does NOT decouple the checkouts. `output` and
    `root-output` are build-script stdout, and for every -sys crate that stdout
    carries absolute paths — measured here, 115 root-output and 14 output files
    naming the checkout they were seeded from, including
    `cargo:rustc-link-search=native=<src>/target/debug/build/libduckdb-sys-*/out`
    and the DEP_OPENSSL_* include/lib dirs. Cargo replays those directives for
    every dependent whenever the build script is fresh, so an unrewritten
    destination links against the source's out/ even though an identical copy
    sits in its own target/. Delete or `cargo clean` the source and the
    destination fails at link time having changed nothing; rebuild
    libduckdb-sys there and the destination silently follows the new static lib
    while its own fingerprint still reads fresh.

    Two passes, and the second is not redundant:

    * the source checkout's own prefix, which also catches paths outside
      target/ (a generated source `include!`ing something under `<src>/crates`);
    * ANY `<root>/target/<profile>/`, because the staleness is transitive.
      Seeding a checkout that was itself seeded leaves it naming its *own*
      source: observed on the first end-to-end run, where the destination came
      out pointing `libduckdb-sys` at a third checkout the first pass had never
      heard of. Matching on the shape rather than on a known prefix collapses
      the whole chain in one go.

    Applied to every small text file under build/, not just `output` and
    `root-output`: build scripts also emit generated sources and pkg-config
    files carrying the same paths (embed.rs, openssl.pc here). Binaries are left
    alone — a path inside an archive's debug info is inert, and lengths differ.
    """

    # Bytes that cannot occur inside a path we would repoint, so scanning back
    # from `/target/<profile>/` to one of them lands on the start of the path.
    # `=` and `:` are in here because that is what precedes the interesting
    # ones: `cargo:rustc-link-search=native=/…`.
    DELIMITERS = frozenset(b" \t\n\r\f\v\"'`;=,:()[]{}<>|")

    def __init__(self, src, dst, profile):
        self.prefixes = self._prefixes(src, dst, profile)
        self.marker = f"/target/{profile}/".encode()
        self.target_repl = (
            os.path.join(os.path.abspath(dst), "target", profile).encode() + b"/"
        )

    @staticmethod
    def _prefixes(src, dst, profile):
        """(source, destination) byte prefixes, plus the resolved target dirs.

        target/ is often a symlink to a bigger disk, and cargo then emits the
        resolved path rather than the one under the checkout root.
        """
        pairs = [(os.path.abspath(src), os.path.abspath(dst))]
        real = (
            os.path.realpath(os.path.join(src, "target", profile)),
            os.path.realpath(os.path.join(dst, "target", profile)),
        )
        if real[0] != pairs[0][0] and real[0] != real[1]:
            pairs.append(real)
        return [(a.encode(), b.encode()) for a, b in pairs]

    def repoint(self, data):
        """Rewrite every absolute `<root>/target/<profile>/` to the destination's.

        A linear find/scan-back rather than a regex: the obvious pattern,
        `/[^\\s:;="']*/target/<profile>/`, backtracks hard over the multi-MB
        generated sources some build scripts leave in out/, and measured 86s
        against 4s for the whole seed.
        """
        out, i = bytearray(), 0
        while True:
            j = data.find(self.marker, i)
            if j < 0:
                break
            start = j
            while start > 0 and data[start - 1] not in self.DELIMITERS:
                start -= 1
            end = j + len(self.marker)
            if data[start] != 0x2F:  # not an absolute path — leave it alone
                out += data[i:end]
            else:
                out += data[i:start] + self.target_repl
            i = end
        if not out:
            return data
        out += data[i:]
        return bytes(out)

    def copy(self, s, d):
        """Copy s to d with paths repointed. False if the caller should copy verbatim."""
        try:
            if os.path.getsize(s) > REWRITE_MAX_BYTES:
                return False
            with open(s, "rb") as fh:
                data = fh.read()
        except OSError:
            return False
        if b"\0" in data:
            return False
        new = data
        for old, repl in self.prefixes:
            new = new.replace(old, repl)
        if self.marker in new:
            new = self.repoint(new)
        if new == data:
            return False
        with open(d, "wb") as fh:
            fh.write(new)
        st = os.stat(s)
        shutil.copymode(s, d)
        os.utime(d, ns=(st.st_atime_ns, st.st_mtime_ns))  # cargo fingerprints need it
        return True


SUBTREES = ("deps", "build", ".fingerprint")


def abort_partial(dst, profile, preexisting, path, err):
    """Undo a half-finished seed, then exit.

    A seed that died mid-walk used to leave a tree that was both unusable and
    un-re-seedable: normalize_mtimes() runs only after seed() RETURNS, so every
    source file kept its fresh checkout mtime and not one copied artifact was
    addressable -- while the now-populated .fingerprint/ made the
    warm-destination guard refuse the retry. `cargo clean` was the only way out.
    Real triggers: the source being built concurrently (rustc renames a temp
    over a deps/ entry, giving ENOENT on os.link), or running out of room part
    way through the 1.3G build/ copy.

    Only the trees this run created from nothing are removed -- that restores
    the checkout exactly, so re-running the seed is the retry. A tree that was
    already there is left alone: our files are indistinguishable from its own,
    so deleting it would be destroying something we were not asked to touch.
    """
    dst_root = os.path.join(dst, "target", profile)
    removed = []
    for sub in SUBTREES:
        if sub in preexisting:
            continue
        if os.path.exists(os.path.join(dst_root, sub)):
            shutil.rmtree(os.path.join(dst_root, sub), ignore_errors=True)
            removed.append(sub)
    hint = {
        errno.ENOENT: "the source looks like it is building right now, and seeding needs it "
        "settled — let its build finish first",
        errno.ENOSPC: "no room left for the copy — build/ alone is ~1.3G per checkout",
        errno.EXDEV: "source and destination are on different filesystems — hardlinks are "
        "impossible",
    }.get(err.errno)
    if removed:
        state = (
            f"rolled back target/{profile}/{{{','.join(removed)}}} — this checkout is "
            f"cold again, so re-running the seed is the retry"
        )
    else:
        state = (
            f"target/{profile} predates this run and is left as it is; it now mixes two "
            f"seeds — re-run to finish replacing it, or `cargo clean` to start over"
        )
    sys.exit(f"failed on {path}: {err}\n" + (f"{hint}\n" if hint else "") + state)


def seed(src, dst, profile, members, share_workspace, replace=False):
    src_root = os.path.join(src, "target", profile)
    # Which trees the destination already had, so abort_partial() knows which of
    # them it may remove to undo a failure.
    preexisting = {
        sub
        for sub in SUBTREES
        if os.path.exists(os.path.join(dst, "target", profile, sub))
    }
    rewriter = PathRewriter(src, dst, profile)
    linked = copied = rewritten = skipped = 0
    started = time.time()

    # deps/ is write-once apart from *.d (see the module docstring). build/ is
    # NOT: a build-script rerun rewrites output / root-output /
    # invoked.timestamp and anything the script put in out/, all via
    # create+truncate, which goes straight through a shared inode into the
    # source checkout — the same hazard .fingerprint/ is copied for. Reruns
    # demonstrably happen here (a 61ns mtime drift caused one), and the source
    # would keep its own fingerprint saying "fresh" while its recorded
    # rustc-cfg / link paths had been replaced by another checkout's. build/ is
    # 1.2G against deps/'s 9.7G, so copying it is the cheap side of the trade.
    for sub in SUBTREES:
        src_dir = os.path.join(src_root, sub)
        if not os.path.isdir(src_dir):
            continue
        for cur, dirs, files in os.walk(src_dir):
            # .fingerprint holds one directory per unit; drop workspace units wholesale
            if (
                sub == ".fingerprint"
                and not share_workspace
                and is_workspace_unit(os.path.basename(cur), members)
            ):
                dirs[:] = []
                skipped += 1
                continue
            dst_dir = os.path.join(
                dst, "target", profile, sub, os.path.relpath(cur, src_dir)
            )
            try:
                os.makedirs(dst_dir, exist_ok=True)
            except OSError as err:
                abort_partial(dst, profile, preexisting, dst_dir, err)
            for name in files:
                if (
                    sub == "deps"
                    and not share_workspace
                    and is_workspace_artifact(name, members)
                ):
                    skipped += 1
                    continue
                s, d = os.path.join(cur, name), os.path.join(dst_dir, name)
                if os.path.lexists(d):
                    if not replace:
                        continue
                    # Unlink rather than write through: a deps/ entry is a
                    # hardlink INTO the source checkout, so truncating it in
                    # place would rewrite the source's artifact. The copy below
                    # then creates a fresh inode.
                    try:
                        os.unlink(d)
                    except OSError as err:
                        abort_partial(dst, profile, preexisting, d, err)
                try:
                    if sub == "deps" and not name.endswith(".d"):
                        os.link(s, d)
                        linked += 1
                    elif sub == "build" and rewriter.copy(s, d):
                        rewritten += 1
                    else:
                        shutil.copy2(
                            s, d
                        )  # copy2 keeps mtime — cargo fingerprints need it
                        copied += 1
                except OSError as err:
                    abort_partial(dst, profile, preexisting, s, err)

    print(
        f"seeded in {time.time() - started:.2f}s: {linked} hardlinked, "
        f"{copied} copied, {rewritten} repointed at this checkout, "
        f"{skipped} workspace artifacts skipped"
    )


def dst_target_device_path(dst, profile):
    """Nearest existing ancestor of the destination's target dir.

    The destination is cold, so target/<profile> usually does not exist yet;
    walk up until something does so st_dev reflects where the artifacts will
    actually land (which is not the checkout root if target/ is a symlink).
    """
    path = os.path.join(dst, "target", profile)
    while not os.path.exists(path):
        parent = os.path.dirname(path)
        if parent == path:
            return dst
        path = parent
    return path


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument(
        "source", nargs="?", help="warm checkout to seed from (default: auto-pick)"
    )
    ap.add_argument(
        "--dest", default=os.getcwd(), help="checkout to seed (default: cwd)"
    )
    ap.add_argument(
        "--profile", default="debug", help="target subdirectory (default: debug)"
    )
    ap.add_argument(
        "--no-workspace-crates",
        action="store_true",
        help="seed third-party artifacts only, and skip mtime normalization (the pre-2026-08 "
        "behaviour, kept for A/B measurement)",
    )
    ap.add_argument(
        "--force",
        action="store_true",
        help="seed over a destination that compiled artifacts of its own, replacing them "
        "(see the warning this bypasses — it can produce a stale build). Not needed to "
        "re-seed a tree that only ever held seeded artifacts; that is automatic.",
    )
    ap.add_argument(
        "--dynamic",
        action="store_true",
        help="prefer a source that has the dev-dynamic dylib built, so a "
        "`cargo build --features dev-dynamic` (`just build-backend-dyn`) in the destination "
        "reuses the seeded ~1.4 GB dylib instead of paying the ~20 min link. Warns if the "
        "chosen source lacks it. Only a tie-break — never seeds a worse dep/source match.",
    )
    args = ap.parse_args()

    dst = checkout_root(args.dest)
    members = run_metadata(dst)

    # Refuse a destination that compiled something ITSELF. normalize_mtimes()
    # backdates sources against the *source* checkout, so if this checkout built
    # an older version of a file that has since become byte-identical to the
    # source's copy, backdating puts it below the local stale artifact and cargo
    # calls the crate fresh -- linking the pre-checkout code. The content match
    # that makes normalization safe only reasons about the source, so it cannot
    # see this.
    #
    # A tree holding nothing but SEEDED artifacts is not that case, and this is
    # where a re-seed is allowed: replacing them all reproduces the new source
    # exactly, the same state a first seed leaves behind. Replacing is the whole
    # point -- seed() skips paths that already exist, so a re-seed that did not
    # replace would keep every stale fingerprint and change nothing.
    marker = read_marker(dst, args.profile)
    replace = args.force
    if has_artifacts(dst, args.profile) and not args.force:
        self_built = (
            built_since_seed(dst, args.profile, marker, members) if marker else None
        )
        if marker is None:
            sys.exit(
                f"{dst} already has target/{args.profile} artifacts of its own.\n"
                f"Seeding is for a cold checkout: backdating sources against another "
                f"checkout here can mark a stale unit fresh and build the wrong code.\n"
                f"Run `cargo clean` first, or --force to seed over them anyway."
            )
        if self_built:
            shown = ", ".join(sorted(self_built)[:3])
            more = f" and {len(self_built) - 3} more" if len(self_built) > 3 else ""
            sys.exit(
                f"{dst} was seeded from {marker.get('source', 'another checkout')}, but has "
                f"since compiled {len(self_built)} workspace unit(s) of its own "
                f"({shown}{more}).\n"
                f"Re-seeding over those would backdate their sources against the new source "
                f"checkout, which can mark a stale unit fresh and build the wrong code.\n"
                f"Run `cargo clean` first, or --force to seed over them anyway."
            )
        replace = True
        print(
            f"re-seeding: everything in target/{args.profile} came from "
            f"{marker.get('source', 'an earlier seed')} and nothing was compiled here since, "
            f"so it is replaced wholesale"
        )

    if args.source:
        src = checkout_root(args.source)
        if not is_warm(src, args.profile):
            sys.exit(f"{src} has no warm target/{args.profile}/deps — build it first")
    else:
        print("scanning for a warm checkout to seed from:")
        src = pick_source(dst, members, args.profile, prefer_dynamic=args.dynamic)

    if args.dynamic and not has_dynamic(src, args.profile):
        print(
            f"WARNING: --dynamic requested but {os.path.basename(src)} has no dev-dynamic dylib "
            f"(target/{args.profile}/deps/liboxy_app_dylib.dylib). Seeding static artifacts only — "
            f"a `--features dev-dynamic` build here will still pay the ~20 min dylib link. Build one "
            f"first with `just build-backend-dyn` in a warm worktree, then re-seed.",
            file=sys.stderr,
        )

    if src == dst:
        sys.exit("source and destination are the same checkout")
    # Stat the target dirs, not the checkout roots: target/ is often a symlink to
    # a bigger disk, in which case the roots match, os.link fails with EXDEV part
    # way through the walk, and the tree is left half-seeded.
    src_dev = os.stat(os.path.join(src, "target", args.profile)).st_dev
    dst_dev = os.stat(dst_target_device_path(dst, args.profile)).st_dev
    if src_dev != dst_dev:
        sys.exit(
            f"target/{args.profile} in the source and destination are on different "
            f"filesystems — hardlinks impossible"
        )

    share_workspace = not args.no_workspace_crates
    print(f"seeding {dst}\n   from {src}")
    seed(src, dst, args.profile, members, share_workspace, replace)
    if share_workspace:
        normalize_mtimes(src, dst)
    write_marker(dst, args.profile, src)


if __name__ == "__main__":
    main()
