// The record a PR run leaves behind, and the one PR comment that explains it
// to the author and points a later release at it.

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
  captured: "✅ **Captured** — this is what the release thread will show.",
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
  const lines = [COMMENT_MARKER, "### Release showcase", "", OUTCOME_LINE[record.outcome]];
  if (record.reason) lines.push("", `> ${record.reason}`);
  const plan = record.plan;
  if (plan && plan.verdict === "show") {
    lines.push("", `**${plan.headline}**`, "", `Starting at \`${plan.start_path}\`:`);
    for (const [i, step] of plan.steps.entries()) lines.push(`${i + 1}. ${step}`);
    if (plan.steps.length === 0) lines.push("_(the start page shows it)_");
    lines.push("", `Checked for: _${plan.expect}_`);
  }
  if (record.screenshot || record.video) {
    lines.push(
      "",
      `Media: the \`${pointer.artifact}\` artifact on [this run](${runUrl}) (kept 90 days).`
    );
  }
  lines.push(
    "",
    "<sub>Wrong screen? Add a `## Showcase` section to the description saying what to show, " +
      "or the `no-showcase` label to leave this PR out. Re-runs on every push that touches `web-app/`.</sub>",
    pointerLine(pointer)
  );
  return lines.join("\n");
}

/** The same comment with the release's thread added to `posted_in`. */
export function markPosted(commentBody: string, threadTs: string): string | undefined {
  const pointer = parsePointer(commentBody);
  if (!pointer) return undefined;
  const posted = new Set(pointer.posted_in ?? []);
  posted.add(threadTs);
  return commentBody.replace(POINTER_RE, pointerLine({ ...pointer, posted_in: [...posted] }));
}
