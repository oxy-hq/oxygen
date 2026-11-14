#!/usr/bin/env python3
"""Describe a built web bundle (a Vite `dist/`) as Markdown for a job summary.

Usage: bundle-summary.py DIST [--title T] [--largest N]

Prints the total, what `index.html` loads before the app can render (raw and
gzipped, which is what a first visit downloads), the split by file type, and
the largest assets. The web-app dist is embedded into every `oxy` binary, so
its size is also the binary's floor.
"""

import argparse
import gzip
import os
import re
import sys
from pathlib import Path


def mb(n: int) -> str:
    return f"{n / 1e6:.2f} MB" if n >= 1e6 else f"{n / 1e3:.1f} kB"


def gz(path: Path) -> int:
    return len(gzip.compress(path.read_bytes(), compresslevel=6))


def initial_load(dist: Path) -> list[Path]:
    """The scripts, stylesheets and modulepreloads index.html names."""
    index = dist / "index.html"
    if not index.is_file():
        return []
    refs = re.findall(r'(?:src|href)="/?([^"#?]+\.(?:js|mjs|css))"', index.read_text(errors="replace"))
    return [dist / r for r in dict.fromkeys(refs) if (dist / r).is_file()]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("dist")
    ap.add_argument("--title", default="web-app bundle")
    ap.add_argument("--largest", type=int, default=8)
    args = ap.parse_args()

    dist = Path(args.dist)
    files = [p for p in dist.rglob("*") if p.is_file()]
    if not files:
        print(f"### {args.title}\n\n> [!WARNING]\n> `{dist}` is empty or missing.\n")
        return 1
    maps = [p for p in files if p.suffix == ".map"]
    shipped = [p for p in files if p.suffix != ".map"]
    size = {p: p.stat().st_size for p in files}
    total = sum(size[p] for p in shipped)

    out = [f"### {args.title}", "", "| | |", "| --- | --- |"]
    out.append(f"| Total | **{mb(total)}** in {len(shipped):,} files |")
    first = initial_load(dist)
    if first:
        raw = sum(size[p] for p in first)
        out.append(f"| Initial load | {mb(raw)}, **{mb(sum(gz(p) for p in first))} gzipped** ({len(first)} files `index.html` loads) |")
    by_kind: dict = {}
    for p in shipped:
        kind = {".js": "JS", ".mjs": "JS", ".css": "CSS", ".wasm": "WASM"}.get(p.suffix, "other")
        by_kind[kind] = by_kind.get(kind, 0) + size[p]
    out.append("| By type | " + ", ".join(f"{k} {mb(v)}" for k, v in sorted(by_kind.items(), key=lambda kv: -kv[1])) + " |")
    if maps:
        out.append(f"| Source maps | {mb(sum(size[p] for p in maps))} in {len(maps)} files, not counted above |")
    out.append("")

    top = sorted(shipped, key=lambda p: -size[p])[: args.largest]
    out += [f"<details><summary>Largest {len(top)} assets</summary>", ""]
    out += ["| Asset | Size | gzip |", "| --- | ---: | ---: |"]
    for p in top:
        out.append(f"| `{os.path.relpath(p, dist)}` | {mb(size[p])} | {mb(gz(p))} |")
    out += ["", "</details>", ""]
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
