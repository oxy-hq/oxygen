// What this change did to the shape of the code, as facts the reviewer cannot
// get any other way.
//
// Why precomputed text rather than a tool: this job deliberately runs with the
// code-review-graph MCP server off, because across 30 sampled passes the
// reviewer never called it while its tool definitions sat in every prompt
// (see .github/workflows/claude-review.yaml). A handful of lines it cannot
// ignore costs a fraction of a tool definition and is always read.
//
// Silent by default. A change that adds no offender renders nothing at all, so
// the usual cost of this block is zero tokens — which is the only reason it can
// afford to be in every pass.

import { run } from "./exec.ts";

/** One function that got worse, as `bca check` reports it. */
export interface Offender {
  file: string;
  lines: string;
  fn: string;
  metric: string;
  value: string;
  limit: string;
  /**
   * `new`: no baseline entry. `worse`: over its recorded value, by `delta`.
   * `over`: bca reported it with no tag at all, which is what every row looks
   * like when no baseline is in play — then the row is simply over the limit
   * and says nothing about whether this change caused it.
   */
  kind: "new" | "worse" | "over";
  delta: string | null;
}

export interface ScanInput {
  /** Path to `bca`, or null when it is not installed — then there is nothing to say. */
  bin: string | null;
  mergeBase: string;
  cwd?: string;
}

// The tag is OPTIONAL on purpose. bca prefixes rows with `[new]` / `[regr …]`
// only when a baseline is in play; with none configured — a tree without
// `.bca-baseline.toml`, or one bca could not read — it prints the same rows
// bare. Requiring the tag made the parser silently return nothing in exactly
// the case where the most is wrong.
const ROW =
  /^(?:\[(new|regr([^\]]*))\]\s+)?(.+?):(\d+-\d+):\s+(.+?):\s+(\S+)\s+=\s+(\S+)\s+\(limit\s+([^)]+)\)\s*$/;

/** Rows `bca check` prints, as data. Anything that is not a row is ignored. */
export function parseOffenders(stdout: string): Offender[] {
  const out: Offender[] = [];
  for (const line of stdout.split("\n")) {
    const m = ROW.exec(line.trim());
    if (!m) continue;
    const [, tag, pct, file, lines, fn, metric, value, limit] = m;
    // Only the tag and the percentage are optional; the checker cannot know
    // that, and a malformed row is better skipped than guessed at.
    if (!file || !lines || !fn || !metric || !value || !limit) continue;
    const kind = tag === "new" ? "new" : tag ? "worse" : "over";
    out.push({
      file,
      lines,
      fn,
      metric,
      value: shortNumber(value),
      limit: limit.trim(),
      kind,
      delta: kind === "worse" ? (pct ?? "").trim() || null : null
    });
  }
  return out;
}

/** `111.26994203287786` reads as noise in a prompt; `111.3` does not. */
function shortNumber(value: string): string {
  const n = Number(value);
  if (!Number.isFinite(n)) return value;
  return Number.isInteger(n) ? String(n) : n.toFixed(1);
}

/**
 * The block for the PR context, or "" when there is nothing to report.
 *
 * The closing caveat is load-bearing: without it a reviewer treats every row as
 * a finding, and a repo whose clippy `complexity` group is deliberately `allow`
 * does not want a second bot filing style tickets.
 */
export function complexityBlock(offenders: readonly Offender[], max = 20): string {
  if (!offenders.length) return "";
  const shown = offenders.slice(0, max);
  const rows = shown.map((o) => {
    const how =
      o.kind === "new"
        ? "new"
        : o.kind === "over"
          ? "over the limit"
          : `worse${o.delta ? ` ${o.delta}` : ""}`;
    return `- \`${o.file}\`:${o.lines} \`${o.fn}\` — ${o.metric} ${o.value} (limit ${o.limit}), ${how}`;
  });
  const more =
    offenders.length > shown.length ? [`- …and ${offenders.length - shown.length} more.`] : [];
  return [
    "",
    "Measured effect of this change on code shape (`bca check --changed-only`,",
    "limits in `bca.toml`). Offenders already recorded in `.bca-baseline.toml`",
    "are filtered out, so every line below is new in this PR, or worse than it",
    "was:",
    "",
    "<complexity>",
    ...rows,
    ...more,
    "</complexity>",
    "",
    "These are measurements, not findings. A dispatch table, a case list or a",
    "generated mapping can score high and be perfectly clear. Raise one only",
    "where you can say what makes the code hard to follow, and never ask for a",
    "split that would move the number without making the code easier to read.",
    ""
  ].join("\n");
}

/**
 * Run the gate over this PR's diff. Never throws: a reviewer that fails because
 * a measurement tool is missing is worse than one that measures nothing.
 */
export async function scan({ bin, mergeBase, cwd }: ScanInput): Promise<Offender[]> {
  if (!bin || !mergeBase) return [];
  try {
    const result = await run(
      bin,
      [
        "check",
        "--changed-only",
        "--since",
        mergeBase,
        "--no-summary",
        "--no-remediation",
        "--github-annotations=never"
      ],
      cwd === undefined ? { allowFail: true } : { cwd, allowFail: true }
    );
    // 0 clean, 2-5 offenders, 1 tool error. Only the offender codes carry rows.
    if (result.code < 2 || result.code > 5) return [];
    return parseOffenders(result.stdout);
  } catch {
    return [];
  }
}
