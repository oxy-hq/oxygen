// The words a PR author reads. One summary comment, edited in place; inline comments
// for findings worth interrupting someone for.

import { isClosed } from "./state.ts";
import type {
  Finding,
  LensPick,
  Mode,
  Possible,
  ReviewState,
  RoadmapJudgement,
  Severity,
  Skipped
} from "./types.ts";

const LIMIT = 60_000;
const SEVERITY_LABEL: Record<Severity, string> = {
  blocking: "blocking",
  should_fix: "should fix",
  nit: "nit"
};
const SEVERITY_ORDER: readonly Severity[] = ["blocking", "should_fix", "nit"];

/** What the summary says besides the state: how this round ran. */
export interface SummaryView {
  cost: number;
  ms: number;
  mode?: Mode;
  since?: string | null;
  lenses?: readonly LensPick[];
  skippedLenses?: readonly Skipped[];
  failedPasses?: readonly { name: string; subtype: string }[];
  notReviewed?: readonly string[];
  warnings?: string[];
  ignored?: readonly string[];
  large?: boolean;
  size?: { files: number; lines: number };
  largeLimit?: number;
  dropped?: readonly unknown[];
  closedNow?: readonly Finding[];
  runUrl?: string;
  reason?: string | null;
  carriedFrom?: string | null;
}

const sha = (s: string | null | undefined): string => `\`${(s ?? "").slice(0, 9)}\``;
const where = (f: Pick<Finding, "file" | "line" | "end_line">): string =>
  `\`${f.file}:${f.line}${f.end_line ? `-${f.end_line}` : ""}\``;
const oneLine = (s: string | null | undefined): string =>
  (s ?? "").replace(/\s+/g, " ").replace(/\|/g, "\\|").trim();

function money(n: number): string {
  return `$${(Math.round(n * 100) / 100).toFixed(2)}`;
}

function duration(ms: number): string {
  const s = Math.round(ms / 1000);
  return s >= 60 ? `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s` : `${s}s`;
}

function headline(state: ReviewState, view: SummaryView): string {
  const at = sha(state.head);
  if (state.status === "incomplete") return `### The automated review did not finish · ${at}`;
  if (state.status === "skipped") return `### Not reviewed · ${at}`;
  const partial = state.status === "partial" ? " (partial, see Coverage)" : "";
  const carried = view.carriedFrom ? ` · no PR file changed since ${sha(view.carriedFrom)}` : "";
  return `### Review: **${state.verdict ?? "LGTM"}**${partial} · ${at}${carried}`;
}

function roadmapWord(j: RoadmapJudgement): string {
  if (j.behaviour === "unchanged") return "roadmap: no behaviour change";
  return `roadmap: behaviour changes, line ${j.line && j.line !== "new" ? `\`${j.line}\`` : "new"}`;
}

function counts(state: ReviewState, open: readonly Finding[], view: SummaryView): string {
  const parts: string[] = [];
  for (const s of SEVERITY_ORDER) {
    const n = open.filter((f) => f.severity === s).length;
    if (n) parts.push(`${n} ${SEVERITY_LABEL[s]}`);
  }
  if (!parts.length) parts.push("no open findings");
  if (view.closedNow?.length) parts.push(`${view.closedNow.length} closed by this push`);
  if (state.roadmap) parts.push(roadmapWord(state.roadmap));
  if (view.lenses?.length) parts.push(`lenses: ${view.lenses.map((l) => l.id).join(", ")}`);
  return parts.join(" · ");
}

function table(open: readonly Finding[]): string {
  if (!open.length) return "";
  const rows = open.map(
    (f) => `| **${f.id}** | ${SEVERITY_LABEL[f.severity]} | ${oneLine(f.title)} | ${where(f)} |`
  );
  return ["| | Severity | Finding | Where |", "|---|---|---|---|", ...rows].join("\n");
}

function details(f: Finding, withBody: boolean): string {
  const meta = [`found by ${f.sources.join(", ") || f.lens}`, `confidence ${f.confidence}`];
  if (f.rule) meta.push(`rule: ${oneLine(f.rule)}`);
  const body = withBody
    ? [f.reasoning ?? "", f.fix ? `**Fix:** ${f.fix}` : ""].filter(Boolean).join("\n\n")
    : "";
  return [
    `<details><summary><b>${f.id}</b> · ${SEVERITY_LABEL[f.severity]} · ${f.category}: ${oneLine(f.title)}</summary>`,
    "",
    body,
    "",
    `<sub>${meta.join(" · ")}</sub>`,
    "</details>"
  ].join("\n");
}

function closedSection(state: ReviewState): string {
  const closed = state.findings.filter((f) => isClosed(f.status));
  if (!closed.length) return "";
  const lines = closed.map(
    (f) =>
      `- ~~**${f.id}** ${oneLine(f.title)}~~ (${f.status} in ${sha(f.closed)}${f.why ? `: ${oneLine(f.why)}` : ""})`
  );
  return [`<details><summary>Closed (${closed.length})</summary>`, "", ...lines, "</details>"].join(
    "\n"
  );
}

function possibleSection(possible: readonly Possible[]): string {
  if (!possible.length) return "";
  const lines = possible.map(
    (p) =>
      `- ${SEVERITY_LABEL[p.severity]} · ${oneLine(p.title)} (${where({ ...p, end_line: null })}): ${oneLine(p.why)}`
  );
  return [
    `<details><summary>Possible issues the verifier could not confirm (${possible.length})</summary>`,
    "",
    "Not counted in the verdict. Worth a look if the area is yours.",
    "",
    ...lines,
    "</details>"
  ].join("\n");
}

function coverageSection(view: SummaryView, roadmap: RoadmapJudgement | undefined): string {
  const lines: string[] = [];
  if (roadmap)
    lines.push(
      `- Roadmap: ${roadmap.behaviour === "changed" ? "behaviour changes" : "no behaviour change"}${roadmap.reason ? `: ${oneLine(roadmap.reason)}` : ""}`
    );
  lines.push(
    `- Mode: ${view.mode === "incremental" ? `incremental, files changed since ${sha(view.since)}` : "full PR diff"}`
  );
  for (const l of view.lenses ?? []) lines.push(`- Lens **${l.id}**: ${l.reason}`);
  for (const l of view.skippedLenses ?? []) lines.push(`- Lens **${l.id}** not run: ${l.reason}`);
  for (const p of view.failedPasses ?? [])
    lines.push(`- Pass **${p.name}** did not finish (${p.subtype}); its findings are missing`);
  for (const n of view.notReviewed ?? []) lines.push(`- Not reviewed: ${oneLine(n)}`);
  for (const w of view.warnings ?? []) lines.push(`- **Could not finish posting:** ${oneLine(w)}`);
  if (view.ignored?.length)
    lines.push(`- Skipped as lockfile, generated or binary: ${view.ignored.length} file(s)`);
  if (view.large && view.size)
    lines.push(
      `- ${view.size.lines} changed lines: past ${view.largeLimit ?? 1000}, a review reads less of each part. Stacking it (\`gh-stack\` skill) gets each part read.`
    );
  if (view.dropped?.length)
    lines.push(
      `- Filtered before posting: ${view.dropped.length} (low confidence, suppressed category, nit cap, or rejected by the verifier)`
    );
  return ["<details><summary>Coverage</summary>", "", ...lines, "</details>"].join("\n");
}

function footer(view: SummaryView): string {
  const how =
    "Rate a finding by replying `F2 not useful: <why>` or with 👍/👎 on its inline comment. " +
    "Disagree? Reply in its thread; the next push re-checks it. " +
    "A Blocking or unfinished review fails `review-gate`; a maintainer can apply the `review-override` label.";
  const run = view.runUrl ? ` · [run](${view.runUrl})` : "";
  return `<sub>${how} · ${money(view.cost)} · ${duration(view.ms)}${run}</sub>`;
}

function incompleteBody(view: SummaryView): string {
  return [
    `**This push has not had a complete review.** ${view.reason ?? ""}`,
    "",
    "**What to do:** re-run the `claude-review` job, or push again. A change that keeps stopping at the spending cap is too large for one automated review: split it, or ask a person to review it."
  ].join("\n");
}

/** The whole summary comment, state included. Drops finding bodies if it would not fit. */
export function renderSummary(
  state: ReviewState,
  view: SummaryView,
  { marker, stateBlock }: { marker: string; stateBlock: string }
): string {
  const open = state.findings.filter((f) => !isClosed(f.status));
  const build = (withBody: boolean) =>
    [
      marker,
      headline(state, view),
      "",
      state.status === "incomplete" ? incompleteBody(view) : counts(state, open, view),
      state.status === "skipped" ? `\n${view.reason ?? ""}` : "",
      "",
      table(open),
      "",
      ...open.map((f) => details(f, withBody)),
      closedSection(state),
      possibleSection(state.possible),
      coverageSection(view, state.roadmap),
      "",
      footer(view),
      stateBlock
    ].join("\n");
  const full = build(true);
  return full.length <= LIMIT ? full : build(false);
}

/** The body of one inline review comment. */
export function renderInline(f: Finding): string {
  return [
    `**${f.id} · ${SEVERITY_LABEL[f.severity]} · ${f.category}**: ${f.title}`,
    "",
    f.reasoning ?? "",
    "",
    f.fix ? `**Fix:** ${f.fix}` : "",
    "",
    "<sub>Rate it with 👍/👎. If it is wrong, reply here: the next push re-checks it.</sub>",
    `<!-- oxy-review:finding ${f.id} -->`
  ]
    .filter((s, i, all) => !(s === "" && all[i - 1] === ""))
    .join("\n");
}
