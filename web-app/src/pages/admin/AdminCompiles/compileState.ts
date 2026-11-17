import type { WorkspaceCompileRow } from "@/services/api/compiles";

/**
 * What a workspace's compiles say, in one word an operator acts on.
 *
 * The row used to lead with the *current* revision's status — "ready", in green, beside
 * "up to date" — which is true of a workspace still serving a revision from weeks ago
 * while every compile since has failed. On dev that read "ready · up to date" over a
 * history of 52 failures. The state below reads the latest main run against the last good
 * one. Draft and staging compiles (custom-app preview builds) are never promoted, so the
 * rollup leaves them out of `latest_*`: a failed preview build is not a failing workspace.
 */
export type CompileState = "failing" | "behind" | "compiling" | "none" | "ok";

const IN_FLIGHT = new Set(["queued", "pending", "compiling", "running"]);

export function compileState(row: WorkspaceCompileRow): CompileState {
  if (row.latest_status === "failed") return "failing";
  if (row.latest_status && IN_FLIGHT.has(row.latest_status)) return "compiling";
  if (!row.current_revision_id) return "none";
  if (!row.current_is_latest_ready) return "behind";
  return "ok";
}

export const STATE_LABEL: Record<CompileState, string> = {
  failing: "Failing",
  behind: "Behind",
  compiling: "Compiling",
  none: "No revision",
  ok: "Up to date"
};

/** Only a problem gets a colour: failing is danger, behind is warn, the rest are text. */
export const STATE_TONE: Record<CompileState, "danger" | "warn" | "muted"> = {
  failing: "danger",
  behind: "warn",
  compiling: "muted",
  none: "muted",
  ok: "muted"
};

/** The sentence above the table: how many are failing or behind, of how many listed. */
export function compilesSentence(rows: WorkspaceCompileRow[]): string {
  const n = rows.length;
  const of = `${n} workspace${n === 1 ? "" : "s"}`;
  const failing = rows.filter((r) => compileState(r) === "failing").length;
  const behind = rows.filter((r) => compileState(r) === "behind").length;
  if (failing === 0 && behind === 0) return `All ${of} compile cleanly.`;
  const parts = [
    failing > 0 ? `${failing} failing to compile` : null,
    behind > 0 ? `${behind} with a newer good revision not yet promoted` : null
  ].filter(Boolean);
  return `${parts.join(", and ")}, of ${of}.`;
}
