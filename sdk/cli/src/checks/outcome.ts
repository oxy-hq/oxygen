/**
 * What one promote check concluded, in the shape the platform stores it: a
 * status, one summary line, and a detail object that always carries
 * `findings`.
 *
 * The runner reports `loads` and `design_system` in this shape
 * (`POST …/checks/report`), and the console renders it as it came. The caps
 * are the platform's own, applied here too so a row stays a verdict and never
 * becomes a copy of what a page printed.
 */

export type Severity = "error" | "warning" | "info";

/** One thing a check found, as every kind's `detail.findings` lists it. */
export interface Finding {
  severity: Severity;
  message: string;
  /** Where: a `file:line`, a URL, or nothing. */
  at: string | null;
  /** The rule that found it, when one did (`restates-sdk-token`). */
  rule: string | null;
}

/** The verdicts a runner may report. */
export type VerdictStatus = "passed" | "failed" | "error" | "skipped";

export interface Outcome {
  status: VerdictStatus;
  summary: string;
  detail: { findings: Finding[] } & Record<string, unknown>;
}

/** The longest finding message, page error or URL kept. */
export const MAX_MESSAGE_CHARS = 400;
/** The most findings one check reports; `findings_omitted` counts the rest. */
export const MAX_FINDINGS = 50;
/** The longest summary line. */
export const MAX_SUMMARY_CHARS = 200;

/** `text` cut to `max` characters, marked when cut. */
export function cap(text: string, max: number): string {
  // By character, not UTF-16 unit, so a cut never splits one.
  const chars = Array.from(text);
  if (chars.length <= max) return text;
  return `${chars.slice(0, Math.max(0, max - 1)).join("")}…`;
}

export function finding(
  severity: Severity,
  message: string,
  where: { at?: string; rule?: string } = {}
): Finding {
  return {
    severity,
    message: cap(message, MAX_MESSAGE_CHARS),
    at: where.at === undefined ? null : cap(where.at, MAX_MESSAGE_CHARS),
    rule: where.rule ?? null
  };
}

/** An outcome whose detail is `extra` with `findings` set over it. */
export function outcome(
  status: VerdictStatus,
  summary: string,
  findings: readonly Finding[],
  extra: Record<string, unknown> = {}
): Outcome {
  const detail: Outcome["detail"] = { ...extra, findings: findings.slice(0, MAX_FINDINGS) };
  if (findings.length > MAX_FINDINGS) detail.findings_omitted = findings.length - MAX_FINDINGS;
  return { status, summary: cap(summary, MAX_SUMMARY_CHARS), detail };
}

/** A check that could not reach a verdict, and why. */
export function errorOutcome(summary: string, extra: Record<string, unknown> = {}): Outcome {
  return outcome("error", summary, [finding("error", summary)], extra);
}

/** One line of an exception, for a summary: its message, first line only. */
export function messageOf(cause: unknown): string {
  const text = cause instanceof Error ? cause.message : String(cause);
  return (text.split("\n")[0] ?? "").trim() || "the check stopped without saying why";
}
