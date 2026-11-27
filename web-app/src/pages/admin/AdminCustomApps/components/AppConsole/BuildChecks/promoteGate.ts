import type { AdminTone } from "@/pages/admin/components/adminTone";
import type { BuildChecks } from "@/types/appBuildChecks";
import type { ReleaseTimeline } from "@/types/appReleases";
import type { AppBuild } from "@/types/apps";
import { checksRollup, type Rollup } from "./buildChecksModel";
import { newErrors, smokeScreens } from "./screensModel";

/**
 * What the Live build card says once checks hold Promote (the org's
 * `promote_checks_gate` flag): whether Promote waits, the chip a required check that
 * did not pass gets, and the line production's own smoke gets after a move.
 *
 * The card reads the staging build's checks, not the promote plan: the plan is a
 * dry run the confirm makes, and the server refuses the move on the same rows either
 * way. So this only decides what the card shows; it lets nothing through.
 */

const SATISFIED = ["passed", "skipped"];
const OPEN = ["queued", "running"];
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export interface CardGate {
  /** `waiting`: a required check is still open. `unmet`: none is open and one did not
   *  pass. `open`: every required check is satisfied. */
  state: "open" | "waiting" | "unmet";
  /** Required kinds that ended and are not satisfied (or have no row at all). */
  unmet: string[];
  /** Required kinds still queued or running. */
  running: string[];
}

/**
 * Where the staging build stands against the kinds the app requires. A required
 * check is satisfied by `passed` or `skipped`; a build never checked satisfies none.
 * `null` while the checks have not been read: the card then holds nothing back.
 *
 * With `seenRequired`, a comparison with production that is still open makes Promote
 * wait too: nobody can have seen what has not been compared yet.
 */
export function cardGate(
  checks: Pick<BuildChecks, "state" | "checks"> | undefined,
  required: readonly string[],
  seenRequired = false
): CardGate | null {
  if (!checks) return null;
  if (checks.state === "none") {
    return { state: required.length ? "unmet" : "open", unmet: [...required], running: [] };
  }
  const status = (kind: string) => {
    const row = checks.checks.find((c) => c.kind === kind);
    // Closed because staging moved on before it ran: nobody checked it, so it satisfies
    // nothing, as the server's gate reads it (`detail.not_checked`).
    if (row?.status === "skipped" && row.detail?.not_checked === true) return "not_checked";
    return row?.status ?? "";
  };
  const awaited =
    seenRequired && !required.includes("changes") ? [...required, "changes"] : required;
  const running = awaited.filter((kind) => OPEN.includes(status(kind)));
  const unmet = required.filter((kind) => ![...SATISFIED, ...OPEN].includes(status(kind)));
  return { state: running.length ? "waiting" : unmet.length ? "unmet" : "open", unmet, running };
}

/**
 * The card's Checks chip with the gate on: the roll-up it always showed, except that
 * a required check that did not pass says so, in one chip. A build never checked
 * while something is required reads as the problem it is.
 */
export function gatedRollup(
  data: Pick<BuildChecks, "state" | "checks" | "runner">,
  required: readonly string[]
): Rollup {
  const rollup = checksRollup(data);
  const gate = cardGate(data, required);
  if (gate?.state !== "unmet") return rollup;
  if (data.state === "none") return { state: "none", tone: "danger", label: "Not checked" };
  const statuses = gate.unmet.map((kind) => data.checks.find((c) => c.kind === kind)?.status);
  const count = plural(gate.unmet.length, "required check", "required checks");
  const what = statuses.includes("failed")
    ? "failed"
    : statuses.every((s) => s === "error")
      ? "could not run"
      : "not run";
  return { state: "failed", tone: "danger", label: `${count} ${what}` };
}

export interface SmokeLine {
  /** `waiting`: production does not serve the new build yet, or its screens are being
   *  opened. `error`: the smoke could not run. `other`: skipped, or a status a later
   *  phase adds. */
  state: "waiting" | "passed" | "failed" | "error" | "other";
  tone: AdminTone;
  label: string;
  /** The check's own summary, for a hover. */
  title: string | null;
}

/**
 * The card's line for production's smoke after a move, from the production attempt of
 * the build production serves. `null` when there is none to show: the answer is not a
 * production attempt (a server that ignores `environment` answers staging's), or it
 * has no `production_smoke` row.
 */
export function productionSmokeLine(data: BuildChecks | undefined): SmokeLine | null {
  if (data?.environment !== "production") return null;
  const row = data.checks.find((c) => c.kind === "production_smoke");
  if (!row) return null;
  const title = row.summary;
  switch (row.status) {
    case "queued":
    case "running":
      return { state: "waiting", tone: "muted", label: "Waiting for the new build", title };
    case "passed": {
      const screens = smokeScreens(row.detail).length;
      const label = screens
        ? `${plural(screens, "screen", "screens")}, 0 new errors`
        : (row.summary ?? "passed");
      return { state: "passed", tone: "ok", label, title };
    }
    case "failed": {
      const screens = new Set(newErrors(row.detail).map((e) => e.path)).size;
      const label = screens
        ? `${plural(screens, "screen", "screens")} with new errors`
        : (row.summary ?? "Smoke failed");
      return { state: "failed", tone: "danger", label, title };
    }
    case "error":
      return { state: "error", tone: "danger", label: "Could not run", title };
    default:
      return { state: "other", tone: "muted", label: row.status, title };
  }
}

/** Whether the answer is a production attempt that exists: what the Checks panel's
 *  environment switch needs before it offers Production. */
export function hasProductionAttempt(data: BuildChecks | undefined): boolean {
  return data?.environment === "production" && data.attempt != null && data.checks.length > 0;
}

/**
 * The build production served before the one it serves now, when a rollback can still
 * choose it: the release the timeline compares the current one with, among the builds
 * still retained. `null` when the timeline names none or its build was pruned.
 */
export function rollbackTarget(
  timeline: Pick<ReleaseTimeline, "releases"> | undefined,
  builds: readonly AppBuild[]
): AppBuild | null {
  const current = timeline?.releases.find((r) => r.current);
  if (!current || current.previous == null) return null;
  const before = timeline?.releases.find((r) => r.index === current.previous);
  const id = before?.build?.id;
  return id ? (builds.find((b) => b.id === id && !b.is_published) ?? null) : null;
}
