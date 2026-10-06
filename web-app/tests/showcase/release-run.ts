// A release's showcase: for each feature and fix the release shipped, plan a
// picture from the PR, capture it on the released build, and post what passes
// the judge under the announcement. Each PR ends in exactly one row of the job
// summary, so "no picture" always says why.
//
// The release is where a picture is decided and paid for — nothing runs when a
// PR is pushed. It used to be the other way round: a PR push planned and
// captured, and a release only replayed what that left. Three prod releases
// then posted nothing, because every PR they shipped had failed its one try at
// review time and a release was not allowed a second.
//
// Every PR runs under its own cap inside the release's cap, features first.
//
// Nothing a release runs or posts comes from anywhere but the PR and this
// run. A preview (showcase.yaml) leaves an artifact with a plan, a recording
// and media; a release used to replay that recording and fall back to that
// media. Anyone who can start a workflow can make such an artifact, so a
// release reads none of it: it plans every PR itself, from the PR.

import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { type CostMeter, createMeter, remaining, worstCaseUsd } from "../agentic/runner/budget";
import { JUDGE_INPUT_BYTES, JUDGE_MAX_TOKENS } from "../agentic/runner/case-runner";
import { JUDGE_MODEL } from "./capture";
import { detect } from "./detect";
import { findShowcaseComment, prFacts, upsertShowcaseComment } from "./github";
import { type PipelineEnv, showcasePr } from "./pipeline";
import { parsePointer, renderComment, writeRecord } from "./record";
import { type ShippedPr, threadComment } from "./release";
import { uploadToThread } from "./slack";
import type { Outcome, RecordPointer, ShowcaseRecord } from "./types";

export interface ReleaseTarget {
  channel: string;
  threadTs: string;
  slackToken?: string;
  runId: string;
  runUrl: string;
  /** Cap for one PR: its plan, its capture, and one corrected plan if enough is left. */
  featureBudgetUsd: number;
  /** Cap for the whole release; features after it is spent are skipped. */
  total: CostMeter;
}

/** How a row reads when the capture succeeded and Slack refused it. */
export const NOT_POSTED = "captured, not posted";

export interface ReleaseRow {
  item: ShippedPr;
  outcome: Outcome | "skipped";
  reason: string;
  cost_usd: number;
}

// Below this a PR cannot afford even the judge call a replay makes.
const FEATURE_FLOOR_USD = worstCaseUsd(JUDGE_MODEL, JUDGE_INPUT_BYTES, JUDGE_MAX_TOKENS);

type MediaFiles = { path: string; title: string }[];

/** What this run captured and its judge passed. */
function mediaFor(record: ShowcaseRecord, dir: string): MediaFiles | undefined {
  const plan = record.plan;
  if (record.outcome !== "captured" || !plan) return undefined;
  const shot = join(dir, "media", "screenshot.png");
  if (!existsSync(shot)) return undefined;
  const files = [{ path: shot, title: plan.headline }];
  const video = join(dir, "media", "video.mp4");
  if (plan.media === "video" && existsSync(video)) {
    files.push({ path: video, title: `${plan.headline} (video)` });
  }
  return files;
}

async function post(
  target: ReleaseTarget,
  env: PipelineEnv,
  item: ShippedPr,
  record: ShowcaseRecord,
  files: MediaFiles
) {
  const comment = threadComment(env.repo, item, record.plan?.headline ?? item.text);
  if (!target.slackToken) {
    console.log(
      `[showcase] SLACK_BOT_TOKEN unset — would post to ${target.channel}/${target.threadTs}:\n${comment}`
    );
    return;
  }
  await uploadToThread({
    token: target.slackToken,
    channel: target.channel,
    threadTs: target.threadTs,
    comment,
    files
  });
}

/**
 * Mark the PR as pictured in this thread, so a re-run never posts it twice.
 * The comment says what was shown, replacing whatever a preview said before.
 */
function remember(
  env: PipelineEnv,
  item: ShippedPr,
  before: RecordPointer | undefined,
  record: ShowcaseRecord,
  target: ReleaseTarget
) {
  const pointer: RecordPointer = {
    run_id: target.runId,
    artifact: "",
    head_sha: record.head_sha,
    outcome: record.outcome,
    posted_in: [...new Set([...(before?.posted_in ?? []), target.threadTs])]
  };
  upsertShowcaseComment(env.repo, item.pr, renderComment(record, pointer, target.runUrl));
}

export async function releaseOne(
  env: PipelineEnv,
  target: ReleaseTarget,
  item: ShippedPr,
  out: string
): Promise<ReleaseRow> {
  const row = (outcome: ReleaseRow["outcome"], reason: string, cost_usd = 0) => ({
    item,
    outcome,
    reason,
    cost_usd
  });
  // The one thing read from the PR's showcase comment: where it was already posted.
  const existing = findShowcaseComment(env.repo, item.pr);
  const pointer = existing ? parsePointer(existing.body) : undefined;
  if (pointer?.posted_in?.includes(target.threadTs))
    return row("skipped", "already posted in this thread");
  // The free filter, on the PR as it is now: a `no-showcase` label keeps it
  // out, and a PR with no screen costs nothing.
  const facts = prFacts(env.repo, item.pr);
  const detection = detect(facts);
  if (!detection.candidate) return row("not_candidate", detection.reason);
  const budget = Math.min(target.featureBudgetUsd, remaining(target.total));
  if (budget < FEATURE_FLOOR_USD) return row("skipped", "the release's spend limit is used up");

  const dir = join(out, `pr-${item.pr}`);
  const meter = createMeter(budget);
  let record: ShowcaseRecord;
  try {
    record = await showcasePr(env, item.pr, dir, meter, facts);
  } finally {
    target.total.spentUsd += meter.spentUsd;
  }
  // The release's cap, not this PR's, refused its first call: nothing was tried.
  const starved = meter.spentUsd === 0 && budget < target.featureBudgetUsd;
  if (record.outcome === "over_budget" && starved) {
    return row("skipped", "the release's spend limit is used up");
  }
  // Beside its media in the run's artifact: the plan, the outcome and why.
  mkdirSync(dir, { recursive: true });
  writeRecord(dir, record);
  const files = mediaFor(record, dir);
  if (!files) return row(record.outcome, record.reason, record.cost_usd);
  try {
    await post(target, env, item, record, files);
  } catch (err) {
    // The picture exists and was paid for — it is in the run's artifact — and
    // the PR is not marked posted, so a re-run after the fix posts it.
    const why = err instanceof Error ? err.message : String(err);
    return row("failed", `${NOT_POSTED} — ${why}`, record.cost_usd);
  }
  if (target.slackToken) remember(env, item, pointer, record, target);
  return row("captured", record.reason, record.cost_usd);
}

export function summaryTable(rows: ReleaseRow[]): string {
  const lines = ["| PR | Outcome | Why | Cost |", "|---|---|---|---|"];
  for (const r of rows) {
    const why = r.reason.replace(/\|/g, "\\|").replace(/\n/g, " ").slice(0, 200);
    lines.push(`| #${r.item.pr} | ${r.outcome} | ${why} | $${r.cost_usd.toFixed(3)} |`);
  }
  return lines.join("\n");
}
