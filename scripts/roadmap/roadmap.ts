#!/usr/bin/env node
// The roadmap doc from the command line, for people and agents editing it, and the
// same checks the `roadmap` merge status runs (scripts/review/src/roadmap.ts):
//
//   node scripts/roadmap/roadmap.ts ids                     every line id, by audience and horizon
//   node scripts/roadmap/roadmap.ts summary [--write]       regenerate platform-roadmap-summary.md
//   node scripts/roadmap/roadmap.ts pin [--write]           pin every unpinned line to its id
//   node scripts/roadmap/roadmap.ts check [--body-file <f>] [--title <t>]
//                                        what the status will say (title: the PR's, else HEAD's subject)
//   node scripts/roadmap/roadmap.ts draft [--body-file <f>] [--title <t>]
//                                        the brief for writing the trailer: the verdict as it stands,
//                                        the product files, and the lines closest to this branch
//   --dir <checkout>                     read that checkout instead of the current directory
//
// Without --write, an edit is printed as a diff and nothing is changed. There is no
// command that cites a PR on a line: the PR body's `Roadmap: <id>` trailer is the link.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { ROADMAP, ROADMAP_CONTEXT } from "../review/config.ts";
import { evaluateRoadmap, productFiles } from "../review/src/roadmap.ts";
import type { CommitStatus } from "../review/src/types.ts";
import { parseRoadmap, pinAll, renderSummary } from "./doc.ts";
import { rankLines, renderDraft } from "./draft.ts";
import { parseTrailer } from "./trailer.ts";

// `link` is gone on purpose: every PR citing itself on the doc made every pair of open
// PRs conflict on it. Say so before parsing (its old `--id`/`--pr` would fail as unknown
// options first) to whoever still reaches for it.
if (process.argv[2] === "link")
  throw new Error(
    "link is gone: PRs are no longer cited in the roadmap. End the PR body with Roadmap: <id>; that trailer is the link"
  );

const { positionals, values: args } = parseArgs({
  allowPositionals: true,
  options: {
    write: { type: "boolean", default: false },
    "body-file": { type: "string" },
    title: { type: "string" },
    base: { type: "string", default: "origin/main" },
    // The checkout to read (doc, summary, git): lets a trusted copy of this script judge
    // another checkout, as the PR guard hook does, without running that checkout's code.
    dir: { type: "string" }
  }
});
if (args.dir) process.chdir(args.dir);

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

const git = (...a: string[]): string => execFileSync("git", a, { encoding: "utf8" }).trim();

interface Branch {
  mergeBase: string;
  files: string[];
  patches: Record<string, string>;
  title: string;
  subjects: string[];
}

/**
 * The working tree against the merge base with `--base` (not its tip: what main did
 * since the branch left it is not this PR's), uncommitted and untracked files included,
 * so the answer is the one the push will get.
 */
function branch(): Branch {
  const mergeBase = git("merge-base", args.base, "HEAD");
  const changed = git("diff", "--name-only", mergeBase);
  const untracked = git("ls-files", "--others", "--exclude-standard");
  const files = `${changed}\n${untracked}`.split("\n").filter(Boolean);
  const patches: Record<string, string> = {};
  for (const f of files) {
    try {
      patches[f] = git("diff", mergeBase, "--", f);
    } catch {
      // an untracked file has no diff against the merge base
    }
  }
  const subjects = git("log", "--format=%s", `${mergeBase}..HEAD`).split("\n").filter(Boolean);
  return { mergeBase, files, patches, title: args.title ?? subjects[0] ?? "", subjects };
}

/** The gate's own verdict for the branch, with the PR body from `--body-file` (default: none). */
function verdict(b: Branch): CommitStatus {
  const body = args["body-file"] ? read(args["body-file"]) : "";
  const before = (): string | null => {
    try {
      return execFileSync("git", ["show", `${b.mergeBase}:${ROADMAP.doc}`], { encoding: "utf8" });
    } catch {
      return null;
    }
  };
  return evaluateRoadmap({
    files: b.files,
    title: b.title,
    patches: b.patches,
    trailer: parseTrailer(body),
    draft: false,
    doc: read(ROADMAP.doc),
    summary: read(ROADMAP.summary),
    docBefore: b.files.includes(ROADMAP.doc) ? before() : null,
    config: ROADMAP
  });
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
  case "check": {
    const result = verdict(branch());
    console.log(`${ROADMAP_CONTEXT}: ${result.state} · ${result.description}`);
    if (result.state !== "success") {
      console.log(
        `\nDraft the trailer: node scripts/roadmap/roadmap.ts draft --title "<PR title>"`
      );
      process.exitCode = 1;
    }
    break;
  }
  case "draft": {
    // Read before the PR body is written: the verdict as the branch stands, the files
    // that count, and the lines whose words it shares, so the id comes from the doc.
    const b = branch();
    const doc = parseRoadmap(read(ROADMAP.doc));
    console.log(
      renderDraft({
        title: b.title,
        files: b.files,
        product: productFiles(b.files, ROADMAP, b.patches),
        verdict: verdict(b),
        ranked: rankLines(doc, [b.title, ...b.subjects, ...b.files])
      })
    );
    break;
  }
  default:
    throw new Error(`unknown command ${JSON.stringify(command)}; see the header of this file`);
}
