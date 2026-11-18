#!/usr/bin/env node
// Replay the reviewer on golden cases (known bugs at a known commit) and score it.
// Every case reserves the full per-review cap before it starts, and the run stops
// before it could pass --max-total-usd.
//   node scripts/review/eval/eval.ts --max-total-usd 15 [--only id,id] [--out dir]

import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { BUDGET } from "../config.ts";
import { git, run } from "../src/exec.ts";
import type { GoldenCase, Hit, Score, Stage, StageDebug } from "../src/score.ts";
import { scoreCase, summarize } from "../src/score.ts";
import type { ReviewState } from "../src/types.ts";

const here = fileURLToPath(new URL(".", import.meta.url));
const { values: args } = parseArgs({
  options: {
    repo: { type: "string", default: process.env.GITHUB_REPOSITORY || "oxy-hq/oxygen-internal" },
    cases: { type: "string", default: join(here, "golden.json") },
    only: { type: "string" },
    "max-total-usd": { type: "string" },
    out: { type: "string" },
    graph: { type: "boolean", default: false }
  }
});

const max = Number(args["max-total-usd"]);
if (!(max > 0))
  throw new Error("--max-total-usd is required: every case can spend up to the per-review cap");
const reserve = BUDGET.total + BUDGET.overshoot;
const only = args.only ? new Set(args.only.split(",")) : null;
const golden = JSON.parse(readFileSync(args.cases, "utf8")) as { cases: GoldenCase[] };
const cases = golden.cases.filter((c) => !only || only.has(c.id));
const work = mkdtempSync(join(tmpdir(), "review-eval-"));

async function review(kase: GoldenCase, tree: string, out: string): Promise<Score> {
  const flags = [
    "--repo",
    args.repo,
    "--pr",
    String(kase.pr),
    "--head",
    kase.head,
    "--base",
    kase.base,
    "--fresh",
    "--out",
    out
  ];
  if (!args.graph) flags.push("--no-graph");
  const result = await run("node", [join(here, "..", "review.ts"), ...flags], {
    cwd: tree,
    allowFail: true
  });
  if (!existsSync(join(out, "state.json")))
    throw new Error(`review.ts exited ${result.code}: ${result.stderr.slice(-1500)}`);
  const state = JSON.parse(readFileSync(join(out, "state.json"), "utf8")) as ReviewState;
  const debugPath = join(out, "result.json");
  const debug = existsSync(debugPath)
    ? {
        ...(JSON.parse(readFileSync(debugPath, "utf8")) as Omit<StageDebug, "complete">),
        complete: state.status === "complete"
      }
    : undefined;
  return scoreCase(kase, {
    debug,
    findings: state.findings.filter((f) => f.status === "open"),
    possible: state.possible ?? [],
    cost: state.rounds.at(-1)?.cost ?? 0
  });
}

async function runCase(kase: GoldenCase): Promise<Score> {
  const tree = join(work, kase.id);
  await git(["fetch", "--no-tags", "origin", kase.head, kase.base]).catch(() => undefined);
  await git(["worktree", "add", "--detach", tree, kase.head]);
  try {
    return await review(kase, tree, join(work, `${kase.id}-out`));
  } catch (err) {
    return {
      id: kase.id,
      pr: kase.pr,
      hits: (kase.expect ?? []).map((e) => ({ what: e.what, result: "error", stage: "unknown" })),
      extra: [],
      cleanFalseAlarms: 0,
      cost: reserve,
      error: err instanceof Error ? err.message : String(err)
    };
  } finally {
    await git(["worktree", "remove", "--force", tree]).catch(() => undefined);
  }
}

const LOST: Record<Stage, string> = {
  posted: "posted",
  possible: "uncertain after verify",
  filtered: "dropped by a filter",
  rejected: "rejected by verify",
  "never-found": "no pass saw it",
  unknown: "stage unknown"
};
const describe = (h: Hit): string =>
  h.result === "missed" || h.result === "possible-only"
    ? `${h.result} (${LOST[h.stage]})`
    : h.result;

const scores: Score[] = [];
let spent = 0;
for (const kase of cases) {
  if (spent + reserve > max) {
    console.log(
      `stop: ${kase.id} would need $${reserve} reserved, $${(max - spent).toFixed(2)} left`
    );
    break;
  }
  const s = await runCase(kase);
  spent += s.cost;
  scores.push(s);
  const extra = s.extra.length ? ` · ${s.extra.length} other finding(s)` : "";
  console.log(
    `${s.id} (#${s.pr}) $${s.cost}: ${s.hits.map(describe).join(", ") || "clean case"}${extra}`
  );
}
const summary = summarize(scores);
console.log(JSON.stringify(summary, null, 2));
if (args.out) writeFileSync(args.out, JSON.stringify({ summary, scores }, null, 2));
