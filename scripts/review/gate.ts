#!/usr/bin/env node
// The only writer of the two merge statuses, run by .github/workflows/review-gate.yaml
// from the default branch (the review job itself holds no status permission):
//   --gate  `review-gate` from the review state, after a push, a label change, or the
//           review run finishing
//   --risk  `high-risk-approval` from the PR's files and reviews
//   --roadmap `roadmap` from the PR's files, its body's `Roadmap:` trailer, the
//           roadmap doc at the PR head and, with no trailer, the review's judgement

import { parseArgs } from "node:util";
import { parseTrailer } from "../roadmap/trailer.ts";
import * as config from "./config.ts";
import { parseCodeowners } from "./src/codeowners.ts";
import { evaluateGate, evaluateRisk } from "./src/gate.ts";
import * as gh from "./src/github.ts";
import { evaluateRoadmap } from "./src/roadmap.ts";
import { decodeState, findSummaryComment } from "./src/state.ts";
import type { CommitStatus } from "./src/types.ts";

const { values: args } = parseArgs({
  options: {
    repo: { type: "string", default: process.env.GITHUB_REPOSITORY },
    pr: { type: "string" },
    gate: { type: "boolean", default: false },
    risk: { type: "boolean", default: false },
    roadmap: { type: "boolean", default: false },
    "run-url": { type: "string", default: process.env.RUN_URL },
    "dry-run": { type: "boolean", default: false },
    // The conclusion of the review workflow run that triggered this, if one did.
    "review-conclusion": { type: "string", default: process.env.REVIEW_CONCLUSION || "" },
    "review-head": { type: "string", default: process.env.REVIEW_HEAD_SHA || "" }
  }
});

const repo = args.repo ?? "";
const pr = Number(args.pr);
if (!repo || !Number.isInteger(pr) || pr <= 0)
  throw new Error("--repo and --pr <number> are required");
const meta = await gh.getPr(repo, pr);
const target = args["run-url"] || undefined;

async function report(context: string, result: CommitStatus): Promise<void> {
  console.log(`${context}: ${result.state} · ${result.description}`);
  if (!args["dry-run"])
    await gh.setStatus(repo, meta.head.sha, {
      state: result.state,
      context,
      description: result.description,
      target_url: target
    });
}

/** The review's posted state, from the summary comment the workflow bot wrote. */
async function reviewState() {
  const comments = await gh.listIssueComments(repo, pr);
  return decodeState(
    findSummaryComment(comments, config.SUMMARY_MARKER, config.REVIEWER_LOGINS)?.body,
    config.STATE_VERSION
  );
}

if (args.gate) {
  const state = await reviewState();
  const labels = meta.labels.map((l) => l.name);
  await report(
    config.GATE_CONTEXT,
    evaluateGate({
      state,
      headSha: meta.head.sha,
      labels,
      draft: meta.draft,
      overrideLabel: config.OVERRIDE_LABEL,
      // Only a failed run for this very head: an older head's failure says nothing here.
      reviewJobFailed:
        args["review-conclusion"] === "failure" && args["review-head"] === meta.head.sha
    })
  );
}

async function prFiles(): Promise<{ files: string[]; patches: Record<string, string> }> {
  const listed = await gh.listPrFiles(repo, pr);
  const files = listed.flatMap((f) =>
    f.previous_filename ? [f.filename, f.previous_filename] : [f.filename]
  );
  const patches: Record<string, string> = {};
  for (const f of listed) if (f.patch) patches[f.filename] = f.patch;
  return { files, patches };
}

if (args.risk) {
  const { files } = await prFiles();
  const reviews = await gh.listReviews(repo, pr);
  // The base branch's CODEOWNERS: a PR cannot make its own author an owner.
  const owners = await gh
    .getFile(repo, ".github/CODEOWNERS", meta.base.sha ?? meta.base.ref)
    .catch(() => "");
  const rules = parseCodeowners(owners);
  await report(
    config.RISK_CONTEXT,
    evaluateRisk({ files, reviews, author: meta.user.login, rules, highRisk: config.HIGH_RISK })
  );
}

if (args.roadmap) {
  const { files, patches } = await prFiles();
  const trailer = parseTrailer(meta.body);
  // The doc and summary as the PR leaves them, and the doc as it found them: read only
  // when a verdict needs them.
  const docChanged = files.includes(config.ROADMAP.doc) || files.includes(config.ROADMAP.summary);
  const needDoc = trailer.kind === "lines" || docChanged;
  const at = (path: string, ref: string) => gh.getFile(repo, path, ref).catch(() => null);
  // The merge base, not the base branch tip: a line another PR moved on main since this
  // branch left it is not this PR's move.
  const base = docChanged ? await gh.mergeBase(repo, meta.base.ref, meta.head.sha) : null;
  await report(
    config.ROADMAP_CONTEXT,
    evaluateRoadmap({
      files,
      title: meta.title,
      patches,
      trailer,
      pr,
      draft: meta.draft,
      doc: needDoc ? await at(config.ROADMAP.doc, meta.head.sha) : null,
      summary: needDoc ? await at(config.ROADMAP.summary, meta.head.sha) : null,
      docBefore: base ? await at(config.ROADMAP.doc, base) : null,
      // The review's answer to "does behaviour change?" decides a PR with product files
      // and no trailer; read it only then.
      ...(trailer.kind === "missing"
        ? { review: { state: await reviewState(), headSha: meta.head.sha } }
        : {}),
      config: config.ROADMAP
    })
  );
}
