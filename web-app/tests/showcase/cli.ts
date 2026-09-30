// Release showcase — design: internal-docs/release-showcase.md.
//
//   detect  --pr N --out DIR [--force]
//                                Should this push capture? No boot, no model. Sets `action`:
//                                capture | refresh (a stale comment to correct) | none.
//   pr      --pr N --out DIR     Plan and capture on the booted instance; writes DIR/record.json.
//   comment --pr N --out DIR     Post or refresh the PR's showcase comment from DIR/record.json.
//   release --from A --to B --out DIR --channel C --thread-ts T
//                                Replay each announced PR on the released build; post to the thread.
//
// Spend is capped, hard: SHOWCASE_BUDGET_USD per PR run (default $0.50),
// SHOWCASE_FEATURE_BUDGET_USD per feature at release ($0.10) inside
// SHOWCASE_RELEASE_BUDGET_USD per release ($0.50). A run stops at its cap.
//
// Env: GITHUB_REPOSITORY, ANTHROPIC_API_KEY, OXY_BASE_URL (the SPA the browser
// opens), OXY_BACKEND_URL (dev-login; defaults to OXY_BASE_URL),
// OXY_DATABASE_URL, SLACK_BOT_TOKEN (release), GITHUB_RUN_ID / GITHUB_SERVER_URL,
// GITHUB_OUTPUT / GITHUB_STEP_SUMMARY when on a runner.

import { appendFileSync, mkdirSync } from "node:fs";
import { parseArgs } from "node:util";
import { createMeter } from "../agentic/runner/budget";
import { detect, uiHash } from "./detect";
import {
  compareSubjects,
  findShowcaseComment,
  prDiff,
  prFacts,
  upsertShowcaseComment
} from "./github";
import { type PipelineEnv, showcasePr } from "./pipeline";
import { parsePointer, readRecord, renderComment, writeRecord } from "./record";
import { announcedPrs } from "./release";
import { type ReleaseRow, type ReleaseTarget, releaseOne, summaryTable } from "./release-run";
import type { RecordPointer, ShowcaseRecord } from "./types";

function need(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`${name} is not set`);
  return v;
}

function budget(name: string, fallback: number): number {
  const raw = process.env[name];
  const v = raw ? Number(raw) : fallback;
  if (!(v > 0)) throw new Error(`${name} must be a positive dollar amount, got '${raw}'`);
  return v;
}

function output(key: string, value: string): void {
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
}

function summary(markdown: string): void {
  if (process.env.GITHUB_STEP_SUMMARY)
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${markdown}\n`);
  console.log(markdown);
}

function runUrl(): string {
  const server = process.env.GITHUB_SERVER_URL ?? "https://github.com";
  return `${server}/${need("GITHUB_REPOSITORY")}/actions/runs/${process.env.GITHUB_RUN_ID ?? "0"}`;
}

function pipelineEnv(): PipelineEnv {
  const baseUrl = need("OXY_BASE_URL");
  return {
    repo: need("GITHUB_REPOSITORY"),
    apiKey: need("ANTHROPIC_API_KEY"),
    baseUrl,
    backendUrl: process.env.OXY_BACKEND_URL ?? baseUrl,
    databaseUrl: need("OXY_DATABASE_URL")
  };
}

function blankRecord(pr: number, title: string, headSha: string): ShowcaseRecord {
  return {
    version: 1,
    pr,
    title,
    head_sha: headSha,
    outcome: "failed",
    reason: "",
    cost_usd: 0,
    captured_at: new Date().toISOString()
  };
}

/**
 * The cheapest decision first. A push whose browser source and steer are
 * unchanged since the last record reuses it: re-planning the same diff buys
 * the same picture, or the same failure, at the same price. Only a failure
 * outside the plan — an API or judge error, a boot that broke — is worth
 * running again.
 */
function cmdDetect(pr: number, out: string, force: boolean): void {
  const repo = need("GITHUB_REPOSITORY");
  const facts = prFacts(repo, pr);
  const d = detect(facts);
  const existing = findShowcaseComment(repo, pr);
  const prior = existing ? parsePointer(existing.body) : undefined;
  let action: "capture" | "refresh" | "none";
  let why: string;
  if (d.candidate) {
    const hash = uiHash(prDiff(repo, pr), d.hint);
    const worthAnotherTry =
      prior?.retryable === true || (prior?.outcome === "failed" && (prior.spent_usd ?? 0) === 0);
    const unchanged = prior?.ui_hash === hash && !worthAnotherTry;
    action = unchanged && !force ? "none" : "capture";
    why = action === "none" ? `unchanged since run ${prior?.run_id} (${prior?.outcome})` : d.type;
  } else if (prior && prior.outcome !== "not_candidate") {
    action = "refresh";
    why = d.reason;
    mkdirSync(out, { recursive: true });
    writeRecord(out, {
      ...blankRecord(pr, facts.title, facts.headSha),
      outcome: "not_candidate",
      reason: d.reason
    });
  } else {
    action = "none";
    why = d.reason;
  }
  output("action", action);
  console.log(`#${pr}: ${action} — ${why}`);
}

async function cmdPr(pr: number, out: string): Promise<void> {
  mkdirSync(out, { recursive: true });
  const meter = createMeter(budget("SHOWCASE_BUDGET_USD", 0.5));
  let record: ShowcaseRecord;
  try {
    record = await showcasePr(pipelineEnv(), pr, out, meter);
  } catch (err) {
    // The comment and summary still say what happened; a thrown run would say nothing.
    record = {
      ...blankRecord(pr, "", process.env.GITHUB_SHA ?? ""),
      reason: err instanceof Error ? err.message : String(err),
      retryable: true,
      cost_usd: meter.spentUsd
    };
  }
  writeRecord(out, record);
  output("outcome", record.outcome);
  summary(
    `### Release showcase — #${pr}\n\n**${record.outcome}**: ${record.reason}\n\n` +
      `Spent $${record.cost_usd.toFixed(3)} of a $${meter.limitUsd.toFixed(2)} limit.`
  );
}

function cmdComment(pr: number, out: string): void {
  const repo = need("GITHUB_REPOSITORY");
  const record = readRecord(out);
  const previous = findShowcaseComment(repo, pr);
  const prior = previous ? parsePointer(previous.body) : undefined;
  const pointer: RecordPointer = {
    run_id: process.env.GITHUB_RUN_ID ?? "",
    artifact: record.screenshot ? `showcase-pr-${pr}` : "",
    head_sha: record.head_sha,
    outcome: record.outcome,
    posted_in: prior?.posted_in,
    ui_hash: record.ui_hash,
    spent_usd: Number(record.cost_usd.toFixed(4)),
    retryable: record.retryable
  };
  // A PR the filter drops gets no comment — unless one is already there and now stale.
  if (record.outcome === "not_candidate" && !previous) return;
  upsertShowcaseComment(repo, pr, renderComment(record, pointer, runUrl()));
}

async function cmdRelease(
  from: string,
  to: string,
  out: string,
  target: ReleaseTarget
): Promise<void> {
  const env = pipelineEnv();
  const items = announcedPrs(compareSubjects(env.repo, from, to));
  const rows: ReleaseRow[] = [];
  for (const item of items) {
    try {
      rows.push(await releaseOne(env, target, item, out));
    } catch (err) {
      const reason = err instanceof Error ? err.message : String(err);
      rows.push({ item, outcome: "failed", reason, cost_usd: 0 });
    }
  }
  const table = rows.length ? summaryTable(rows) : "_No new features or fixes in this range._";
  summary(
    `### Release showcase — ${from.slice(0, 7)}…${to.slice(0, 7)}\n\n${table}\n\n` +
      `Spent $${target.total.spentUsd.toFixed(3)} of a $${target.total.limitUsd.toFixed(2)} limit.`
  );
}

async function main(): Promise<void> {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: {
      pr: { type: "string" },
      out: { type: "string" },
      from: { type: "string" },
      to: { type: "string" },
      channel: { type: "string" },
      "thread-ts": { type: "string" },
      force: { type: "boolean" }
    }
  });
  const pr = Number(values.pr);
  const out = values.out ?? "showcase-out";
  switch (positionals[0]) {
    case "detect":
      return cmdDetect(pr, out, values.force ?? false);
    case "pr":
      return cmdPr(pr, out);
    case "comment":
      return cmdComment(pr, out);
    case "release":
      if (!values.from || !values.to || !values.channel || !values["thread-ts"]) {
        throw new Error("release needs --from, --to, --channel and --thread-ts");
      }
      return cmdRelease(values.from, values.to, out, {
        channel: values.channel,
        threadTs: values["thread-ts"],
        slackToken: process.env.SLACK_BOT_TOKEN || undefined,
        runId: process.env.GITHUB_RUN_ID ?? "",
        runUrl: runUrl(),
        featureBudgetUsd: budget("SHOWCASE_FEATURE_BUDGET_USD", 0.1),
        total: createMeter(budget("SHOWCASE_RELEASE_BUDGET_USD", 0.5))
      });
    default:
      throw new Error("usage: cli.ts detect|pr|comment|release …");
  }
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
