#!/usr/bin/env node
// The weekly code review report: coverage, what findings led to, cost, and bugs that
// got past review. Run by .github/workflows/review-metrics.yaml; locally:
//   node scripts/review/metrics.ts --repo oxy-hq/oxygen-internal --days 30 --out /tmp/m
// GitHub calls: one GraphQL query per 20 merged PRs, plus one per 20 earlier PRs the
// escaped-bug trace lands on. Everything else is local git.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import * as config from "./config.ts";
import type { Culprit, Escape } from "./src/escapes.ts";
import { attribute, introducedBy, isFixTitle, wasFlagged } from "./src/escapes.ts";
import { git } from "./src/exec.ts";
import type { Fetched } from "./src/fetch-merged.ts";
import { fetchMerged, fetchPrs } from "./src/fetch-merged.ts";
import type { Aggregate, GuardrailCrossCut, PrRecord } from "./src/metrics.ts";
import { aggregate, guardrailCrossCut, prRecord } from "./src/metrics.ts";
import { headline, renderReport } from "./src/report.ts";
import type { PrMeta } from "./src/types.ts";

const { values: args } = parseArgs({
  options: {
    repo: { type: "string", default: process.env.GITHUB_REPOSITORY },
    days: { type: "string", default: "7" },
    lookback: { type: "string", default: "90" },
    out: { type: "string" }
  }
});

const repo = args.repo ?? "";
if (!repo) throw new Error("--repo is required");
const DAY = 86_400_000;
const now = new Date();
const since = new Date(now.getTime() - Number(args.days) * DAY);

interface Merged {
  pr: PrMeta;
  rec: PrRecord;
}

interface Trace {
  fix: PrMeta;
  broad: boolean;
  culprits: Culprit[];
}

/** Changed paths from the merge commit; empty when it is not in this clone. */
async function filesOf(pr: PrMeta): Promise<string[]> {
  if (!pr.merge_commit_sha) return [];
  const names = await git([
    "diff",
    "--name-only",
    `${pr.merge_commit_sha}^`,
    pr.merge_commit_sha
  ]).catch(() => "");
  return names.split("\n").filter(Boolean);
}

async function toRecord(fetched: Fetched): Promise<Merged> {
  const files = await filesOf(fetched.pr);
  return { pr: fetched.pr, rec: prRecord({ ...fetched, files, config }) };
}

/** Trace each fix merged into the default branch back to the PRs that wrote its lines. */
async function traceFixes(merged: readonly Merged[]): Promise<Trace[]> {
  const traces: Trace[] = [];
  for (const { pr } of merged) {
    // A fix into a stack branch is squashed again when the stack lands, and its own
    // merge commit is often gone by then.
    const sha = pr.merge_commit_sha;
    if (!isFixTitle(pr.title) || !sha || pr.base.ref !== pr.base.repo?.default_branch) continue;
    const intros = await introducedBy({ git, fix: { sha, number: pr.number } }).catch(
      (err: unknown) => {
        const message = err instanceof Error ? err.message : String(err);
        console.error(`could not trace #${pr.number}: ${message.split("\n")[0] ?? ""}`);
        return [];
      }
    );
    traces.push({ fix: pr, ...attribute(intros) });
  }
  return traces;
}

/** One entry per fix: the PR(s) it most likely fixes, and whether their review saw it. */
async function escapesFor(merged: readonly Merged[]): Promise<Escape[]> {
  const traces = await traceFixes(merged);
  const known = new Map(merged.map((m) => [m.pr.number, m]));
  const missing = [...new Set(traces.flatMap((t) => t.culprits.map((c) => c.pr)))].filter(
    (n) => !known.has(n)
  );
  for (const fetched of await fetchPrs(repo, missing)) {
    const m = await toRecord(fetched);
    known.set(m.pr.number, m);
  }
  const oldest = new Date(now.getTime() - Number(args.lookback) * DAY).toISOString();
  const out: Escape[] = [];
  for (const { fix, broad, culprits } of traces) {
    if (broad) {
      out.push({ fix: fix.number, fixTitle: fix.title, broad: true });
      continue;
    }
    for (const c of culprits) {
      const m = known.get(c.pr);
      if (!m?.pr.merged_at || m.pr.merged_at < oldest) continue;
      const verdicts = c.files.map((f) => ({
        file: f.file,
        ...wasFlagged(m.rec, f.file, f.lines)
      }));
      const hit = verdicts.find((v) => v.flagged) ?? verdicts[0] ?? { flagged: false };
      out.push({
        fix: fix.number,
        fixTitle: fix.title,
        intro: c.pr,
        files: c.files.map((f) => f.file),
        ...hit
      });
    }
  }
  return out;
}

function write(
  report: string,
  agg: Aggregate,
  escapes: readonly Escape[],
  records: readonly PrRecord[],
  crossCut: GuardrailCrossCut
): void {
  console.log(report);
  if (process.env.GITHUB_STEP_SUMMARY)
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${report}\n`);
  if (!args.out) return;
  mkdirSync(args.out, { recursive: true });
  writeFileSync(join(args.out, "report.md"), report);
  const window = `${since.toISOString().slice(0, 10)} to ${now.toISOString().slice(0, 10)}`;
  writeFileSync(
    join(args.out, "slack.txt"),
    [`Code review, ${window}`, ...headline({ agg, escapes })].join("\n")
  );
  writeFileSync(
    join(args.out, "metrics.json"),
    JSON.stringify(
      {
        window: { since, now },
        agg,
        guardrailCrossCut: crossCut,
        escapes,
        records
      },
      null,
      2
    )
  );
}

const merged: Merged[] = [];
for (const fetched of await fetchMerged(repo, since.toISOString()))
  merged.push(await toRecord(fetched));
const records = merged.map((m) => m.rec);
const escapes = await escapesFor(merged);
const agg = aggregate(records);
const window = { from: since.toISOString().slice(0, 10), to: now.toISOString().slice(0, 10) };
const crossCut = guardrailCrossCut(records, escapes);
write(renderReport({ agg, escapes, crossCut, window }), agg, escapes, records, crossCut);
