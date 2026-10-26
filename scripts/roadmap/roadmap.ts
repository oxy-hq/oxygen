#!/usr/bin/env node
// The roadmap doc from the command line, for people and agents editing it, and the
// same checks the `roadmap` merge status runs (scripts/review/src/roadmap.ts):
//
//   node scripts/roadmap/roadmap.ts ids                     every line id, by audience and horizon
//   node scripts/roadmap/roadmap.ts summary [--write]       regenerate platform-roadmap-summary.md
//   node scripts/roadmap/roadmap.ts pin [--write]           pin every unpinned line to its id
//   node scripts/roadmap/roadmap.ts link --id <id> --pr <n> [--write]   cite PR n on a line
//   node scripts/roadmap/roadmap.ts check [--body-file <f>] [--pr <n>]  what the gate will say
//
// Without --write, an edit is printed as a diff and nothing is changed.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { ROADMAP, ROADMAP_CONTEXT } from "../review/config.ts";
import { evaluateRoadmap } from "../review/src/roadmap.ts";
import { linkPr, parseRoadmap, pinAll, renderSummary } from "./doc.ts";
import { parseTrailer } from "./trailer.ts";

const { positionals, values: args } = parseArgs({
  allowPositionals: true,
  options: {
    write: { type: "boolean", default: false },
    id: { type: "string" },
    pr: { type: "string" },
    "body-file": { type: "string" },
    base: { type: "string", default: "origin/main" },
    repo: { type: "string", default: process.env.GITHUB_REPOSITORY ?? "oxy-hq/oxygen-internal" }
  }
});

const command = positionals[0] ?? "check";
const read = (p: string): string => readFileSync(p, "utf8");

function apply(path: string, next: string, what: string): void {
  const current = read(path);
  if (current === next) {
    console.log(`${path}: already ${what}`);
    return;
  }
  if (args.write) {
    writeFileSync(path, next);
    console.log(`${path}: ${what}`);
    return;
  }
  const tmp = `${process.env.TMPDIR ?? "/tmp"}/roadmap-${process.pid}.md`;
  writeFileSync(tmp, next);
  try {
    execFileSync("git", ["--no-pager", "diff", "--no-index", "--", path, tmp], {
      stdio: "inherit"
    });
  } catch {
    // `git diff` exits 1 when the files differ, which is the point.
  }
  console.log(`\n(not written: pass --write to ${what})`);
}

switch (command) {
  case "ids": {
    const doc = parseRoadmap(read(ROADMAP.doc));
    for (const a of doc.audiences) {
      console.log(`\n${a.title}`);
      for (const h of ["now", "next", "later"] as const)
        for (const it of a.horizons[h])
          console.log(`  ${h.padEnd(5)} ${it.id}${it.landed ? "  (landed)" : ""}`);
    }
    break;
  }
  case "summary":
    apply(ROADMAP.summary, renderSummary(read(ROADMAP.doc)), "regenerated");
    break;
  case "pin":
    apply(ROADMAP.doc, pinAll(read(ROADMAP.doc)), "pinned");
    break;
  case "link": {
    const pr = Number(args.pr);
    if (!args.id || !Number.isInteger(pr) || pr <= 0)
      throw new Error("link needs --id <line-id> and --pr <number>");
    apply(ROADMAP.doc, linkPr(read(ROADMAP.doc), args.id, pr, args.repo), `linked to #${pr}`);
    break;
  }
  case "check": {
    // The gate's own verdict for this checkout against `--base`, with the PR body from
    // `--body-file` (default: no trailer, so the message says what one would need).
    // The working tree against the merge base with `--base` (not its tip: what main did
    // since the branch left it is not this PR's), uncommitted and untracked files
    // included, so the answer is the one the push will get.
    const git = (...a: string[]): string => execFileSync("git", a, { encoding: "utf8" }).trim();
    const mergeBase = git("merge-base", args.base, "HEAD");
    const changed = git("diff", "--name-only", mergeBase);
    const untracked = git("ls-files", "--others", "--exclude-standard");
    const files = `${changed}\n${untracked}`.split("\n").filter(Boolean);
    const body = args["body-file"] ? read(args["body-file"]) : "";
    const before = (): string | null => {
      try {
        return execFileSync("git", ["show", `${mergeBase}:${ROADMAP.doc}`], { encoding: "utf8" });
      } catch {
        return null;
      }
    };
    const result = evaluateRoadmap({
      files,
      trailer: parseTrailer(body),
      pr: args.pr ? Number(args.pr) : null,
      draft: false,
      doc: read(ROADMAP.doc),
      summary: read(ROADMAP.summary),
      docBefore: files.includes(ROADMAP.doc) ? before() : null,
      config: ROADMAP
    });
    console.log(`${ROADMAP_CONTEXT}: ${result.state} · ${result.description}`);
    if (result.state !== "success") {
      console.log(`\nLine ids: node scripts/roadmap/roadmap.ts ids`);
      process.exitCode = 1;
    }
    break;
  }
  default:
    throw new Error(`unknown command ${JSON.stringify(command)}; see the header of this file`);
}
