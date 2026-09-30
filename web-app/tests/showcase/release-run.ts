// A release's showcase: for each PR the announcement lists, find the record its
// PR run left, replay it on the released build, and post what passes the judge
// under the announcement. Each PR ends in exactly one row of the job summary,
// so "no picture" always says why.
//
// A release only REPLAYS. Plans are made, and paid for, at review time; here a
// captured record costs a judge call, a re-record only when the UI moved on,
// and every feature runs under its own cap inside the release's cap.

import { existsSync } from "node:fs";
import { join } from "node:path";
import { type CostMeter, createMeter, remaining, worstCaseUsd } from "../agentic/runner/budget";
import { JUDGE_INPUT_BYTES, JUDGE_MAX_TOKENS } from "../agentic/runner/case-runner";
import { JUDGE_MODEL } from "./capture";
import { SKIP_LABEL } from "./detect";
import {
  type Comment,
  downloadArtifact,
  findShowcaseComment,
  prLabels,
  updateComment,
  upsertShowcaseComment
} from "./github";
import { type PipelineEnv, recaptureForRelease } from "./pipeline";
import { markPosted, parsePointer, readRecord, renderComment } from "./record";
import { type AnnouncedPr, threadComment } from "./release";
import { uploadToThread } from "./slack";
import type { Outcome, RecordPointer, ShowcaseRecord } from "./types";

export interface ReleaseTarget {
  channel: string;
  threadTs: string;
  slackToken?: string;
  runId: string;
  runUrl: string;
  /** Cap for one feature's replay (and a re-record if it needs one). */
  featureBudgetUsd: number;
  /** Cap for the whole release; features after it is spent are skipped. */
  total: CostMeter;
}

export interface ReleaseRow {
  item: AnnouncedPr;
  outcome: Outcome | "skipped";
  /** Which build the posted media came from. */
  source?: "release" | "review";
  reason: string;
  cost_usd: number;
}

// Below this a feature cannot afford even the judge call its replay makes.
const FEATURE_FLOOR_USD = worstCaseUsd(JUDGE_MODEL, JUDGE_INPUT_BYTES, JUDGE_MAX_TOKENS);

interface Media {
  files: { path: string; title: string }[];
  source: "release" | "review";
}

function mediaFor(record: ShowcaseRecord, dir: string): Media | undefined {
  const plan = record.plan;
  if (!plan) return undefined;
  const wantVideo = plan.media === "video";
  const pick = (base: string, source: Media["source"]): Media | undefined => {
    const shot = join(dir, base, "screenshot.png");
    if (!existsSync(shot)) return undefined;
    const files = [{ path: shot, title: plan.headline }];
    const video = join(dir, base, "video.mp4");
    if (wantVideo && existsSync(video))
      files.push({ path: video, title: `${plan.headline} (video)` });
    return { files, source };
  };
  if (record.outcome === "captured") return pick("media", "release");
  return pick("review-media", "review");
}

function reviewRecord(env: PipelineEnv, pointer: RecordPointer, dir: string): ShowcaseRecord {
  downloadArtifact(env.repo, pointer.run_id, pointer.artifact, dir);
  return readRecord(dir);
}

async function post(
  target: ReleaseTarget,
  env: PipelineEnv,
  item: AnnouncedPr,
  record: ShowcaseRecord,
  media: Media
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
    files: media.files
  });
}

function remember(
  env: PipelineEnv,
  item: AnnouncedPr,
  existing: Comment | undefined,
  record: ShowcaseRecord,
  target: ReleaseTarget
) {
  const marked = existing ? markPosted(existing.body, target.threadTs) : undefined;
  if (existing && marked) return updateComment(env.repo, existing.id, marked);
  const pointer: RecordPointer = {
    run_id: target.runId,
    artifact: "",
    head_sha: record.head_sha,
    outcome: record.outcome,
    posted_in: [target.threadTs]
  };
  upsertShowcaseComment(env.repo, item.pr, renderComment(record, pointer, target.runUrl));
}

export async function releaseOne(
  env: PipelineEnv,
  target: ReleaseTarget,
  item: AnnouncedPr,
  out: string
): Promise<ReleaseRow> {
  const row = (
    outcome: ReleaseRow["outcome"],
    reason: string,
    cost_usd = 0,
    source?: ReleaseRow["source"]
  ) => ({ item, outcome, reason, cost_usd, source });
  const existing = findShowcaseComment(env.repo, item.pr);
  const pointer = existing ? parsePointer(existing.body) : undefined;
  if (!pointer)
    return row("skipped", "no review-time record (the PR's showcase run never finished)");
  if (pointer.posted_in?.includes(target.threadTs))
    return row("skipped", "already posted in this thread");
  if (pointer.outcome !== "captured" || !pointer.artifact)
    return row(pointer.outcome, "decided at review time");
  // Read now, not trusted from the record: a label added after the last push
  // (or after the review run) still has to keep the PR out of the thread.
  if (prLabels(env.repo, item.pr).includes(SKIP_LABEL))
    return row("skipped", `labelled \`${SKIP_LABEL}\``);
  const budget = Math.min(target.featureBudgetUsd, remaining(target.total));
  if (budget < FEATURE_FLOOR_USD) return row("skipped", "the release's spend limit is used up");

  const dir = join(out, `pr-${item.pr}`);
  const meter = createMeter(budget);
  let record: ShowcaseRecord;
  try {
    record = await recaptureForRelease(env, reviewRecord(env, pointer, dir), dir, meter);
  } finally {
    target.total.spentUsd += meter.spentUsd;
  }
  const media = mediaFor(record, dir);
  if (!media) return row(record.outcome, record.reason, record.cost_usd);
  await post(target, env, item, record, media);
  if (target.slackToken) remember(env, item, existing, record, target);
  const why = media.source === "review" ? `release capture: ${record.reason}` : record.reason;
  return row("captured", why, record.cost_usd, media.source);
}

export function summaryTable(rows: ReleaseRow[]): string {
  const lines = ["| PR | Outcome | Media from | Why | Cost |", "|---|---|---|---|---|"];
  for (const r of rows) {
    const why = r.reason.replace(/\|/g, "\\|").replace(/\n/g, " ").slice(0, 200);
    lines.push(
      `| #${r.item.pr} | ${r.outcome} | ${r.source ?? "—"} | ${why} | $${r.cost_usd.toFixed(3)} |`
    );
  }
  return lines.join("\n");
}
