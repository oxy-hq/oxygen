#!/usr/bin/env node
// Automated PR review. CI: `node scripts/review/review.ts --pr 123 --post`.
// Locally, a dry run prints the summary instead of posting it:
//   node scripts/review/review.ts --repo oxy-hq/oxygen-internal --pr 123 --fresh --out /tmp/r
// internal-docs/code-review.md describes the pipeline.

import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import * as config from "./config.ts";
import * as gh from "./src/github.ts";
import { runPass } from "./src/llm.ts";
import type { Loaded } from "./src/load.ts";
import { loadRound } from "./src/load.ts";
import {
  minimizeLegacy,
  pickInline,
  postInline,
  resolveClosed,
  upsertSummary
} from "./src/publish.ts";
import { redact, secretValues } from "./src/redact.ts";
import { isReleaseBump } from "./src/release.ts";
import type { SummaryView } from "./src/render.ts";
import { renderSummary } from "./src/render.ts";
import type { RoundOutcome } from "./src/round.ts";
import { runRound } from "./src/round.ts";
import { compactForStorage, encodeState, openFindings } from "./src/state.ts";
import type { ReviewState, ReviewStatus, Round } from "./src/types.ts";

interface Args {
  repo: string;
  pr: number;
  head: string | undefined;
  base: string | undefined;
  post: boolean;
  fresh: boolean;
  graph: boolean;
  requireKey: boolean;
  out: string | undefined;
  runUrl: string | undefined;
}

function cli(): Args {
  const { values } = parseArgs({
    options: {
      repo: { type: "string", default: process.env.GITHUB_REPOSITORY },
      pr: { type: "string" },
      head: { type: "string" },
      base: { type: "string" },
      post: { type: "boolean", default: false },
      fresh: { type: "boolean", default: false },
      graph: { type: "boolean", default: true },
      "no-graph": { type: "boolean", default: false },
      "require-key": { type: "boolean", default: false },
      out: { type: "string" },
      "run-url": { type: "string", default: process.env.RUN_URL }
    }
  });
  const pr = Number(values.pr);
  if (!values.repo || !Number.isInteger(pr) || pr <= 0)
    throw new Error("--repo and --pr <number> are required");
  return {
    repo: values.repo,
    pr,
    head: values.head,
    base: values.base,
    post: values.post,
    fresh: values.fresh,
    graph: values.graph && !values["no-graph"],
    requireKey: values["require-key"],
    out: values.out,
    runUrl: values["run-url"]
  };
}

// Every posted text is scrubbed of the secrets this job holds: see src/redact.ts.
const SECRETS = secretValues(process.env);
const clean = (text: string): string => redact(text, SECRETS);

/** A publishing step that failed: said on the PR, not only in the log. */
function warn(view: SummaryView, message: string): void {
  console.warn(`::warning::${message}`);
  view.warnings = [...(view.warnings ?? []), message.slice(0, 300)];
}

const messageOf = (err: unknown): string => (err instanceof Error ? err.message : String(err));

/** GitHub rejects a comment over 65,536 characters: shed the stored explanations first. */
function summaryBody(state: ReviewState, view: SummaryView): string {
  // The state is redacted before it is encoded: base64 would carry a secret straight
  // past a scrub of the rendered text.
  const stateBlock = (lean: boolean) =>
    encodeState(
      JSON.parse(clean(JSON.stringify(compactForStorage(state, { lean })))) as ReviewState
    );
  const render = (lean: boolean) =>
    clean(
      renderSummary(state, view, { marker: config.SUMMARY_MARKER, stateBlock: stateBlock(lean) })
    );
  const full = render(false);
  return full.length <= 65_000 ? full : render(true);
}

function writeOut(args: Args, files: Record<string, unknown>): void {
  if (!args.out) return;
  mkdirSync(args.out, { recursive: true });
  for (const [name, data] of Object.entries(files))
    writeFileSync(
      join(args.out, name),
      typeof data === "string" ? data : JSON.stringify(data, null, 2)
    );
}

function stepSummary(state: ReviewState, view: SummaryView): void {
  const file = process.env.GITHUB_STEP_SUMMARY;
  if (!file) return;
  const passes = Object.entries(state.rounds.at(-1)?.passes ?? {}).map(([n, p]) => {
    const tools = Object.entries(p.tools)
      .map(([t, c]) => `${t}×${c}`)
      .join(" ");
    return `| ${n} | ${p.ok ? "ok" : p.subtype} | $${p.cost} | ${Math.round(p.ms / 1000)}s | ${tools} |`;
  });
  const title = `### ${state.verdict ?? "no verdict"} (${state.status}) · $${view.cost.toFixed(2)}`;
  appendFileSync(
    file,
    [
      title,
      "",
      "| pass | result | cost | time | tools |",
      "|---|---|---|---|---|",
      ...passes,
      ""
    ].join("\n")
  );
}

async function publish(args: Args, load: Loaded, outcome: RoundOutcome): Promise<void> {
  const { state, view } = outcome;
  if (args.post) {
    const picks = pickInline(outcome.added, load.files, config.MAX_INLINE);
    const inline = await postInline({
      gh,
      repo: args.repo,
      pr: args.pr,
      head: load.head,
      picks,
      summaryUrl: load.summary?.html_url,
      clean
    }).catch((e: unknown) => {
      warn(view, `inline comments were not posted: ${messageOf(e)}`);
      return new Map<string, number>();
    });
    for (const f of state.findings) {
      const id = inline.get(f.id);
      if (id !== undefined) f.inline = id;
    }
    await resolveClosed({
      gh,
      repo: args.repo,
      pr: args.pr,
      closed: outcome.closedNow,
      clean
    }).catch((e: unknown) =>
      warn(view, `closed findings were not marked in their threads: ${messageOf(e)}`)
    );
  }
  const body = summaryBody(state, { ...view, ...(args.runUrl ? { runUrl: args.runUrl } : {}) });
  if (args.post) {
    await upsertSummary({ gh, repo: args.repo, pr: args.pr, existing: load.summary, body });
    if (!load.summary) await minimizeLegacy({ gh, comments: load.comments });
  } else {
    console.log(body);
  }
  stepSummary(state, view);
  writeOut(args, {
    "plan.json": load.plan,
    "state.json": state,
    "summary.md": body,
    "result.json": outcome.debug ?? {},
    "passes.json": outcome.results ?? []
  });
}

/** A round that needs no model: a release bump, no credentials, nothing reviewable, or nothing changed. */
function shortcut(args: Args, load: Loaded): RoundOutcome | null {
  // The last round's roadmap judgement was about the last head; a round that runs no
  // general pass has none for this one (the carried case below keeps it: nothing changed).
  const { roadmap: carriedJudgement, ...prev } = load.prev;
  const base: ReviewState = { ...prev, head: load.head, base: load.mergeBase };
  const round = (mode: string, status: ReviewStatus): Round => ({
    sha: load.head.slice(0, 9),
    at: new Date().toISOString(),
    mode,
    status,
    verdict: base.verdict,
    cost: 0,
    ms: 0,
    lines: load.plan.size.lines,
    lenses: [],
    added: [],
    closed: []
  });
  const view: SummaryView = {
    mode: load.plan.mode,
    ignored: load.plan.ignored,
    size: load.plan.size,
    cost: 0,
    ms: 0
  };
  const outcome = (
    state: ReviewState,
    mode: string,
    extra: Partial<SummaryView> = {}
  ): RoundOutcome => ({
    state: { ...state, rounds: [...base.rounds, round(mode, state.status)] },
    view: { ...view, ...extra },
    added: [],
    closedNow: []
  });
  const notReviewed = (reason: string, mode: string) =>
    outcome({ ...base, status: "skipped", reason, verdict: "Not reviewed" }, mode, { reason });

  if (isReleaseBump({ meta: load.meta, files: load.files }, config.RELEASE_AUTOMATION))
    return notReviewed("a version bump the release app opened", "release");
  if (args.requireKey && !process.env.ANTHROPIC_API_KEY)
    return notReviewed(
      "no reviewer credentials on this run (Dependabot PRs get no secrets)",
      "skipped"
    );
  if (!load.plan.reviewable.length) {
    const verdict = openFindings(base).length ? base.verdict : "LGTM";
    return outcome({ ...base, status: "complete", verdict }, "nothing-reviewable", {
      notReviewed: ["only lockfile, generated or binary files changed"]
    });
  }
  if (load.plan.mode === "incremental" && !load.plan.inScope.length)
    return outcome(
      {
        ...base,
        status: load.prev.status,
        ...(carriedJudgement ? { roadmap: carriedJudgement } : {})
      },
      "carried",
      { carriedFrom: load.since }
    );
  return null;
}

async function failed(args: Args, load: Loaded | null, err: unknown): Promise<void> {
  console.error(err);
  if (!args.post || !load) return;
  // No general pass finished for this head, so no roadmap judgement for it either.
  const { roadmap: _stale, ...prev } = load.prev;
  const state: ReviewState = {
    ...prev,
    head: load.head,
    base: load.mergeBase,
    status: "incomplete"
  };
  const body = summaryBody(state, {
    reason: `It stopped with an error: \`${messageOf(err).slice(0, 300)}\`.`,
    ...(args.runUrl ? { runUrl: args.runUrl } : {}),
    cost: 0,
    ms: 0
  });
  await upsertSummary({ gh, repo: args.repo, pr: args.pr, existing: load.summary, body }).catch(
    () => undefined
  );
}

/** The roadmap at the PR checkout, for the general pass; a checkout without one gets none. */
function roadmapDoc(cwd: string): string | null {
  try {
    return readFileSync(join(cwd, config.ROADMAP.doc), "utf8");
  } catch {
    return null;
  }
}

async function main(): Promise<void> {
  const args = cli();
  const cwd = process.cwd();
  let load: Loaded | null = null;
  try {
    load = await loadRound({ gh, repo: args.repo, pr: args.pr, args, cwd });
    const outcome =
      shortcut(args, load) ??
      (await runRound({
        load: { ...load, roadmapDoc: roadmapDoc(cwd) },
        repo: args.repo,
        runPass,
        cwd,
        graph: args.graph
      }));
    await publish(args, load, outcome);
    if (outcome.state.status === "incomplete") process.exitCode = 1;
  } catch (err) {
    await failed(args, load, err);
    process.exitCode = 1;
  }
}

await main();
