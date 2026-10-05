// The record one PR's showcase leaves behind, and the one PR comment that
// explains it to the author: written by a preview run by hand, or by the
// release that pictured the PR. A release reads one thing back from it — the
// threads the PR was already posted in.

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { RECORD_FILE, type RecordPointer, type ShowcaseRecord } from "./types";

export const COMMENT_MARKER = "<!-- oxy-showcase -->";
const POINTER_RE = /<!-- oxy-showcase-record (\{.*?\}) -->/s;

export function writeRecord(dir: string, record: ShowcaseRecord): void {
  writeFileSync(join(dir, RECORD_FILE), `${JSON.stringify(record, null, 2)}\n`);
}

export function readRecord(dir: string): ShowcaseRecord {
  return JSON.parse(readFileSync(join(dir, RECORD_FILE), "utf-8")) as ShowcaseRecord;
}

export function parsePointer(commentBody: string): RecordPointer | undefined {
  const m = POINTER_RE.exec(commentBody);
  if (!m) return undefined;
  try {
    return JSON.parse(m[1]) as RecordPointer;
  } catch {
    return undefined;
  }
}

function pointerLine(pointer: RecordPointer): string {
  // `--` cannot appear inside an HTML comment; JSON of these fields never has it.
  return `<!-- oxy-showcase-record ${JSON.stringify(pointer)} -->`;
}

const OUTCOME_LINE: Record<ShowcaseRecord["outcome"], string> = {
  captured: "✅ **Captured** — a preview of what the release thread will show.",
  not_candidate: "➖ **Nothing to show**",
  not_visual: "➖ **Nothing to show** — no visible change.",
  needs_seed: "🌱 **Visible, but the demo data can't reach it yet.**",
  rejected: "❌ **Not captured** — the final screen did not show the change.",
  failed: "⚠️ **Not captured** — the capture failed.",
  over_budget: "💸 **Not captured** — it hit the spend limit before finishing."
};

export function renderComment(
  record: ShowcaseRecord,
  pointer: RecordPointer,
  runUrl: string
): string {
  const posted = record.outcome === "captured" && (pointer.posted_in?.length ?? 0) > 0;
  const outcome = posted
    ? "✅ **Pictured under the release announcement.**"
    : OUTCOME_LINE[record.outcome];
  const lines = [COMMENT_MARKER, "### Release showcase", "", outcome];
  if (record.reason) lines.push("", `> ${record.reason}`);
  const plan = record.plan;
  if (plan && plan.verdict === "show") {
    lines.push("", `**${plan.headline}**`, "", `Starting at \`${plan.start_path}\`:`);
    for (const [i, step] of plan.steps.entries()) lines.push(`${i + 1}. ${step}`);
    if (plan.steps.length === 0) lines.push("_(the start page shows it)_");
    lines.push("", `Checked for: _${plan.expect}_`);
  }
  // A release keeps its media in its own run's artifact, not one named for the PR.
  if (pointer.artifact && (record.screenshot || record.video)) {
    lines.push(
      "",
      `Media: the \`${pointer.artifact}\` artifact on [this run](${runUrl}) (kept 30 days).`
    );
  } else if (posted) {
    lines.push("", `Captured by [the release run](${runUrl}).`);
  }
  lines.push(
    "",
    "<sub>Pictures are taken when a release ships. To steer one, add a `## Showcase` section to " +
      "the description saying what to show; the `no-showcase` label leaves a PR out.</sub>",
    pointerLine(pointer)
  );
  return lines.join("\n");
}
