#!/usr/bin/env python3
"""Describe a pushed container image as Markdown, for a GitHub job summary.

Usage:
  image-report.py REF [--title T] [--tags a,b] [--baseline REF --baseline-label L]
                      [--layers N] [--note TEXT]...

REF is anything `docker buildx imagetools inspect` resolves: a tag, or better a
`repo@sha256:…` digest, which cannot move under the report. It may be a
multi-platform index or a single-platform manifest. What it prints, per
platform: the compressed size a `docker pull` downloads, the layer count, the
change against BASELINE (the image this one replaces), how much of it a host
that already has BASELINE must download, and the largest layers with the
Dockerfile instruction that made each one.

Why this and not docker/build-push-action's own summary: that one reports the
build (a build-record link for Docker Desktop, cached-step counts) and nothing
about the artifact. Whether an image got bigger, and which layer did it, is
what a reader of a release run wants to know.

Reads the registry through the docker CLI, so it uses whatever `docker login`
the job did. Needs nothing beyond the Python standard library and `docker`.
Exits 1 when REF cannot be inspected; the caller decides whether that matters.
"""

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass, field

ATTESTATION = "attestation-manifest"


@dataclass
class Layer:
    digest: str
    size: int
    created_by: str


@dataclass
class Platform:
    name: str
    digest: str
    size: int  # config + layers: what a pull downloads
    layers: list = field(default_factory=list)
    created: str = ""


# A report makes about six registry calls per image, twelve with a baseline.
# Each is bounded so a stalled registry costs the report, never the job that
# runs it: the callers' step timeouts assume 12 calls fit in 6 minutes.
CALL_TIMEOUT_S = 30


def inspect(ref: str, fmt: str | None = None) -> str:
    cmd = ["docker", "buildx", "imagetools", "inspect", ref]
    cmd += ["--format", fmt] if fmt else ["--raw"]
    done = subprocess.run(cmd, capture_output=True, text=True, timeout=CALL_TIMEOUT_S)
    if done.returncode != 0:
        raise RuntimeError(done.stderr.strip() or f"exit {done.returncode}")
    return done.stdout


def repo_of(ref: str) -> str:
    if "@" in ref:
        return ref.split("@", 1)[0]
    head, _, tail = ref.rpartition("/")
    return f"{head}/{tail.split(':', 1)[0]}" if head else tail.split(":", 1)[0]


def digest_of(ref: str) -> str:
    return json.loads(inspect(ref, "{{json .Manifest}}"))["digest"]


def platform_name(p: dict) -> str:
    name = f"{p.get('os', '?')}/{p.get('architecture', '?')}"
    return f"{name}/{p['variant']}" if p.get("variant") else name


def load_platform(repo: str, digest: str, name: str | None) -> Platform:
    manifest = json.loads(inspect(f"{repo}@{digest}"))
    config = json.loads(inspect(f"{repo}@{digest}", "{{json .Image}}"))
    history = [h for h in config.get("history", []) if not h.get("empty_layer")]
    layers = []
    for i, blob in enumerate(manifest.get("layers", [])):
        made_by = history[i].get("created_by", "") if i < len(history) else ""
        layers.append(Layer(blob["digest"], int(blob["size"]), made_by))
    size = int(manifest.get("config", {}).get("size", 0)) + sum(l.size for l in layers)
    return Platform(
        name=name or platform_name(config),
        digest=digest,
        size=size,
        layers=layers,
        created=config.get("created", ""),
    )


def load(ref: str) -> tuple[str, list[Platform], int]:
    """(digest of REF, its platforms, how many attestation manifests it carries)."""
    repo = repo_of(ref)
    digest = digest_of(ref)
    raw = json.loads(inspect(ref))
    if "manifests" not in raw:
        return digest, [load_platform(repo, digest, None)], 0
    platforms, attestations = [], 0
    for entry in raw["manifests"]:
        kind = (entry.get("annotations") or {}).get("vnd.docker.reference.type")
        plat = entry.get("platform") or {}
        if kind == ATTESTATION or plat.get("os") == "unknown":
            attestations += 1
            continue
        platforms.append(load_platform(repo, entry["digest"], platform_name(plat)))
    return digest, platforms, attestations


def human(n: float) -> str:
    """Decimal units, as `docker images` and the registries print them."""
    for unit in ("B", "kB", "MB", "GB"):
        if abs(n) < 1000 or unit == "GB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1000
    return f"{n:.1f} GB"


def signed_delta(new: int, old: int) -> str:
    diff = new - old
    if diff == 0:
        return "no change"
    sign = "+" if diff > 0 else "−"
    share = abs(diff) / old * 100 if old else 0
    pct = f" ({sign}{share:.1f}%)" if share >= 0.1 else ""
    return f"{sign}{human(abs(diff))}{pct}"


def instruction(created_by: str) -> str:
    """`RUN |2 A=b /bin/sh -c apt-get …  # buildkit` → `RUN apt-get …`, cell-safe."""
    text = re.sub(r"\s*# buildkit$", "", created_by.strip())
    text = re.sub(r"^RUN \|\d+(?: \S+=\S*)* ", "RUN ", text)
    text = text.replace("/bin/sh -c ", "")
    text = re.sub(r"\s+", " ", text) or "(no history)"
    if len(text) > 110:
        text = text[:107] + "…"
    return "`" + text.replace("`", "'").replace("|", "\\|") + "`"


def layer_table(p: Platform, base: Platform | None, top: int) -> list[str]:
    known = {l.digest for l in base.layers} if base else set()
    ranked = sorted(p.layers, key=lambda l: l.size, reverse=True)
    out = [
        f"<details><summary>Largest layers — {p.name}</summary>",
        "",
        "| Size | Share | " + ("vs baseline | " if base else "") + "Made by |",
        "| ---: | ---: | " + ("--- | " if base else "") + "--- |",
    ]
    for l in ranked[:top]:
        share = f"{l.size / p.size * 100:.0f}%" if p.size else "–"
        reuse = ("shared" if l.digest in known else "**new**") if base else None
        cells = [human(l.size), share] + ([reuse] if reuse else []) + [instruction(l.created_by)]
        out.append("| " + " | ".join(cells) + " |")
    rest = ranked[top:]
    if rest:
        out.append(f"\n…and {len(rest)} smaller layer(s), {human(sum(l.size for l in rest))} together.")
    out += ["", "</details>", ""]
    return out


def render(args, digest, platforms, attestations, baseline) -> list[str]:
    repo = repo_of(args.ref)
    title = args.title or f"Image `{repo}`"
    created = min((p.created for p in platforms if p.created), default="")
    rows = [("Image", f"`{repo}`"), ("Digest", f"`{digest}`")]
    if args.tags:
        rows.append(("Tags", ", ".join(f"`{t}`" for t in args.tags)))
    if created:
        rows.append(("Created", re.sub(r"(\.\d+)?Z$", " UTC", created.replace("T", " "))))
    if attestations:
        rows.append(("Attestations", f"{attestations} manifest(s) (provenance / SBOM)"))
    rows += [(k, v) for k, v in (n.split("=", 1) for n in args.note)]

    out = [f"### {title}", "", "| | |", "| --- | --- |"]
    out += [f"| {k} | {v} |" for k, v in rows]
    out.append("")

    base_by_name = {p.name: p for p in baseline} if baseline else {}
    head = "| Platform | Pull size (compressed) | Layers |"
    rule = "| --- | ---: | ---: |"
    if baseline is not None:
        head += f" vs {args.baseline_label} | New bytes to pull |"
        rule += " ---: | ---: |"
    out += [head, rule]
    for p in platforms:
        row = f"| {p.name} | **{human(p.size)}** | {len(p.layers)} |"
        if baseline is not None:
            base = base_by_name.get(p.name)
            if base:
                known = {l.digest for l in base.layers}
                fresh = sum(l.size for l in p.layers if l.digest not in known)
                row += f" {signed_delta(p.size, base.size)} | {human(fresh)} |"
            else:
                row += " not in baseline | – |"
        out.append(row)
    out.append("")
    if args.layers > 0:
        for p in platforms:
            out += layer_table(p, base_by_name.get(p.name), args.layers)
    out += ["```bash", f"docker pull {repo}@{digest}", "```", ""]
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("ref")
    ap.add_argument("--title")
    ap.add_argument("--tags", type=lambda s: [t for t in re.split(r"[,\s]+", s) if t], default=[])
    ap.add_argument("--baseline", help="the image this one replaces (pin it by digest)")
    ap.add_argument("--baseline-label", default="baseline")
    ap.add_argument("--layers", type=int, default=6, help="largest layers listed per platform; 0 lists none")
    ap.add_argument("--note", action="append", default=[], help="extra KEY=VALUE summary row")
    args = ap.parse_args()

    try:
        digest, platforms, attestations = load(args.ref)
    except (RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as err:
        print(f"> [!WARNING]\n> Could not inspect `{args.ref}`: {err}\n")
        print(f"image-report: {args.ref}: {err}", file=sys.stderr)
        return 1

    baseline = None
    if args.baseline:
        try:
            _, baseline, _ = load(args.baseline)
        except (RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as err:
            print(f"image-report: baseline {args.baseline}: {err}", file=sys.stderr)
            args.note.append(f"Baseline=could not read `{args.baseline}`")

    print("\n".join(render(args, digest, platforms, attestations, baseline)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
