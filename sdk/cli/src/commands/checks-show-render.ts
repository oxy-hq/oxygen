/**
 * How one attempt of a build's checks reads on a terminal: `oxyc checks
 * show`'s report, and the verdict `oxyc promote` prints once production has
 * been checked. Pure: a body in, lines out.
 *
 * Every kind renders the same way, by its label, its summary and its
 * findings, so a kind this file has never heard of still prints. Three things
 * are said beyond that:
 *
 * - WHAT CHANGED says how many screens differ and whether a person has looked
 *   at them: that is the one check a person closes, not a runner.
 * - EVERY SCREEN OPENS names the screens of the app it saw linked and did not
 *   open, so "every screen" is not read as more than the list it was given.
 * - A NOTE IS MARKED AS WHAT IT IS. A model writes notes under a check, and a
 *   summary of the build. They are advice, never a verdict, and each prints
 *   under the words "note by a model" so it is not read as the check's own
 *   finding.
 */

import { out } from "../ui/tty.js";
import type { BuildCheck, BuildChecks, CheckNote } from "./checks-show.js";

/** The console's words for each kind, for a row the server sent without a label. */
export const CHECK_LABELS: Record<string, string> = {
  validation: "Builds and validates",
  functions: "Its checks pass",
  loads: "It loads, desk and phone",
  design_system: "Follows the design system",
  smoke: "Every screen opens",
  changes: "What changed",
  production_smoke: "Production opens after promote",
  design_audit: "Design audit"
};

/** Findings listed under a failed check before the rest are counted. */
const MAX_FINDINGS = 3;
/** Unopened screens named under the `smoke` row before the rest are counted. */
const MAX_UNLISTED = 5;

type Style = typeof out;

/** The rows the state rolls up: advisory ones inform and never count. */
export function gating(body: BuildChecks): BuildCheck[] {
  return body.checks.filter((c) => !c.advisory);
}

export function labelOf(check: BuildCheck): string {
  return check.label ?? CHECK_LABELS[check.kind] ?? check.kind;
}

function mark(status: string, style: Style): string {
  switch (status) {
    case "passed":
      return style.green("✓");
    case "failed":
      return style.red("✗");
    case "error":
      return style.yellow("!");
    case "skipped":
      return style.dim("-");
    case "queued":
    case "running":
      return style.cyan("…");
    default:
      return style.dim("?");
  }
}

/** `2 of 4 finished`: how far a running attempt has got. */
export function progress(body: BuildChecks): string {
  const rows = gating(body);
  const done = rows.filter((c) => c.status !== "queued" && c.status !== "running").length;
  return `${done} of ${rows.length} finished`;
}

/** `1 failed, 1 error, 2 passed` — the non-zero counts, worst first. */
function tally(body: BuildChecks): string {
  const rows = gating(body);
  const parts: string[] = [];
  for (const status of ["failed", "error", "passed", "skipped"]) {
    const n = rows.filter((c) => c.status === status).length;
    if (n > 0) parts.push(`${n} ${status}`);
  }
  return parts.join(", ");
}

function header(label: string, body: BuildChecks, style: Style): string {
  const parts = [
    style.bold(label),
    `build ${body.build?.build_id ?? "?"}`,
    body.environment ?? "staging"
  ];
  if (body.attempt === null || body.attempt === undefined) {
    parts.push("not checked");
  } else {
    const latest = body.attempts?.[0]?.attempt;
    const older = latest !== undefined && latest !== body.attempt ? ` (latest is ${latest})` : "";
    parts.push(`attempt ${body.attempt}${older}`);
  }
  return parts.join(" · ");
}

function rollUp(body: BuildChecks, style: Style): string {
  switch (body.state) {
    case "passed":
      return `${style.green("passed")} · ${tally(body)}`;
    case "failed":
      return `${style.red("failed")} · ${tally(body)}`;
    case "running":
      return `${style.cyan("running")} · ${progress(body)}`;
    case "none":
      return `${style.dim("none")} · this build was not checked`;
    default:
      return `${body.state} · ${tally(body)}`;
  }
}

/** Who ran the checks, with the link to the run when there is one. */
function runnerLine(body: BuildChecks, style: Style): string[] {
  if (!body.runner?.label) return [];
  const url = body.runner.url ? ` · ${body.runner.url}` : "";
  return [style.dim(`  run by ${body.runner.label}${url}`)];
}

/** What an attempt that got no answer in time says, and what to do about it. */
function stalledLine(body: BuildChecks, style: Style): string[] {
  if (!body.stalled) return [];
  const why =
    body.stalled === "no_runner"
      ? "no runner picked these checks up"
      : "the runner stopped before it finished";
  return [
    `  ${style.yellow("!")} the runner did not answer (${why}): run the checks again from the app's Builds page`
  ];
}

/** A failed or errored check's findings, a few of them, then how many more. */
function findingLines(check: BuildCheck, style: Style): string[] {
  if (check.status !== "failed" && check.status !== "error") return [];
  const findings = check.detail?.findings ?? [];
  const lines = findings
    .slice(0, MAX_FINDINGS)
    .map((f) => `      ${f.at ? `${style.dim(f.at)}  ` : ""}${f.message}`);
  if (findings.length > MAX_FINDINGS) {
    lines.push(`      ${style.dim(`+${findings.length - MAX_FINDINGS} more`)}`);
  }
  return lines;
}

/**
 * Under the `smoke` row: the screens of the app the opened ones link to that
 * the list did not hold, so they were not opened. Nothing when the runner
 * named none (or is one from before it named any).
 */
function unlistedLines(check: BuildCheck, style: Style): string[] {
  if (check.kind !== "smoke") return [];
  const detail = check.detail ?? {};
  const named = Array.isArray(detail.screens_unlisted) ? detail.screens_unlisted : [];
  const paths = named.filter((path): path is string => typeof path === "string");
  const counted = detail.screens_unlisted_total;
  const total = typeof counted === "number" ? counted : paths.length;
  if (total <= 0) return [];
  const shown = paths.slice(0, MAX_UNLISTED).map((path) => `/${path}`);
  const more = total - shown.length;
  const names = [...shown, ...(more > 0 ? [`+${more} more`] : [])].join(", ");
  const noun = total === 1 ? "linked screen" : "linked screens";
  return [
    `      ${style.yellow("!")} ${total} ${noun} not opened: ${names} (declare the ones that matter in checks.screens)`
  ];
}

/** `a model (claude-…)`, `a rule`: who wrote a note. */
function authorOf(note: CheckNote): string {
  if (note.author === "model") return note.model ? `a model (${note.model})` : "a model";
  return note.author === "rule" ? "a rule" : note.author;
}

/** One note, under `lead` (`note by a model (…):`), its text indented beneath when it has lines. */
export function noteLines(note: CheckNote, lead: string, indent: string, style: Style): string[] {
  const text = (typeof note.text === "string" ? note.text : "").trim().split("\n");
  const head = `${indent}${style.dim(`${lead} by ${authorOf(note)}:`)}`;
  if (text.length === 1) return [`${head} ${text[0] ?? ""}`.trimEnd()];
  return [head, ...text.map((line) => `${indent}  ${line}`.trimEnd())];
}

/** A check's notes, each marked as a note and by whom. */
function notesOf(check: BuildCheck, style: Style): string[] {
  const notes = Array.isArray(check.notes) ? check.notes : [];
  return notes.flatMap((note) => noteLines(note, "note", "      ", style));
}

/**
 * What the `changes` row says beside its summary: whether a person has looked
 * at the screens that differ. Nothing when none differ, or it did not run.
 */
function seenBy(check: BuildCheck): string {
  if (check.kind !== "changes" || check.status !== "passed") return "";
  const detail = check.detail ?? {};
  if (typeof detail.changed !== "number" || detail.changed === 0) return "";
  const seen = detail.seen as { by?: unknown } | null | undefined;
  if (!seen) return " · not seen by a person yet";
  return typeof seen.by === "string" && seen.by ? ` · seen by ${seen.by}` : " · seen by a person";
}

/** The row's one line of words: its summary, or how many screens differ when it sent none. */
function summaryOf(check: BuildCheck): string {
  if (check.summary) return check.summary;
  const detail = check.detail ?? {};
  if (check.kind === "changes" && typeof detail.compared === "number") {
    return `${typeof detail.changed === "number" ? detail.changed : 0} of ${detail.compared} screens differ`;
  }
  return check.status === "queued" || check.status === "running" ? check.status : "";
}

/**
 * The human report: a header, who ran the checks, one line per check (mark,
 * label, summary), the findings under any that failed and the notes under any
 * that has them, the build's summary note, a line when the runner never
 * answered, then the roll-up.
 */
export function renderBuildChecks(label: string, body: BuildChecks, style: Style = out): string[] {
  const lines = [header(label, body, style), ...runnerLine(body, style)];
  const width = Math.max(0, ...body.checks.map((c) => labelOf(c).length));
  for (const check of body.checks) {
    const advisory = check.advisory ? style.dim(" (advisory)") : "";
    const words = `${summaryOf(check)}${seenBy(check)}`;
    lines.push(
      `  ${mark(check.status, style)} ${labelOf(check).padEnd(width)}  ${words}${advisory}`.trimEnd()
    );
    lines.push(
      ...findingLines(check, style),
      ...unlistedLines(check, style),
      ...notesOf(check, style)
    );
  }
  if (body.summary) lines.push(...noteLines(body.summary, "summary", "  ", style));
  lines.push(...stalledLine(body, style), rollUp(body, style));
  return lines;
}
