import { apiErrorCode, apiErrorMessage, apiStatus } from "@/libs/apiError";
import type { AdminTone } from "@/pages/admin/components/adminTone";
import type {
  BuildCheck,
  BuildChecks,
  BuildChecksRunner,
  BuildCheckViewport
} from "@/types/appBuildChecks";
import type { AppBuild } from "@/types/apps";

/**
 * The pure half of a build's checks: the card's one-line roll-up, the run's progress
 * steps, each row's status words, and the evidence lines. Phase 1 only reports, so
 * nothing here may say "required", "blocked" or "waiting": it says what ran and what
 * it found.
 *
 * The checks run on a runner outside the platform (a GitHub Actions run), so an
 * attempt has a stretch where it is queued and nobody has answered yet. `runner` and
 * `stalled` say where that stands; an older server sends neither, and everything
 * here reads that as "no runner named" rather than as a fault.
 */

export interface Rollup {
  state: "none" | "running" | "passed" | "failed" | "unknown";
  tone: AdminTone;
  label: string;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** No runner has claimed the attempt and every row still waits. */
function nothingStarted(data: Pick<BuildChecks, "checks" | "runner">): boolean {
  return !data.runner && data.checks.every((c) => c.status === "queued");
}

type RollupRows = readonly Pick<BuildCheck, "status">[];

/** How many of `rows` are in one of `statuses`. */
const countIn = (rows: RollupRows, ...statuses: string[]) =>
  rows.filter((c) => statuses.includes(c.status)).length;

/** A run still open: how far it is. */
function runningRollup(data: Pick<BuildChecks, "checks" | "runner">, rows: RollupRows): Rollup {
  if (rows.length === 0) return { state: "running", tone: "muted", label: "Queued" };
  // The runner is not up yet, so "0 of 4 done" would count work nobody is doing.
  if (nothingStarted(data)) return { state: "running", tone: "muted", label: "Starting" };
  const done = rows.length - countIn(rows, "queued", "running");
  return { state: "running", tone: "muted", label: `${done} of ${rows.length} done` };
}

/** A run that failed: how many checks did, or could not run. */
function failedRollup(rows: RollupRows): Rollup {
  const failed = countIn(rows, "failed");
  const errored = countIn(rows, "error");
  if (failed + errored === 0) return { state: "failed", tone: "danger", label: "Checks failed" };
  // An `error` counts as a failure; when every failure is one, nothing found a fault:
  // the check could not run.
  const label =
    failed === 0
      ? `${plural(errored, "check", "checks")} could not run`
      : `${plural(failed + errored, "check", "checks")} failed`;
  return { state: "failed", tone: "danger", label };
}

/** A run that passed: how many checks did, and how many were skipped. */
function passedRollup(rows: RollupRows): Rollup {
  const passed = countIn(rows, "passed");
  const skipped = countIn(rows, "skipped");
  if (passed === 0) return { state: "passed", tone: "muted", label: `${skipped} skipped` };
  if (skipped === 0) {
    return {
      state: "passed",
      tone: "ok",
      label: passed === 1 ? "1 passed" : `All ${passed} passed`
    };
  }
  return { state: "passed", tone: "ok", label: `${passed} passed, ${skipped} skipped` };
}

/** The Live build card's Checks row, over the rows that are not advisory (the
 *  server's roll-up counts the same rows). */
export function checksRollup(data: Pick<BuildChecks, "state" | "checks" | "runner">): Rollup {
  const rows = data.checks.filter((c) => !c.advisory);
  switch (data.state) {
    case "none":
      return { state: "none", tone: "muted", label: "Not checked" };
    case "running":
      return runningRollup(data, rows);
    case "failed":
      return failedRollup(rows);
    case "passed":
      return passedRollup(rows);
    default:
      return { state: "unknown", tone: "muted", label: data.state };
  }
}

/** `0:19` / `1:38`: minutes and seconds from one instant to a later one, the way a
 *  stopwatch reads. Empty when either instant is missing or unreadable. */
export function clockText(from: string | null | undefined, to: string | null | undefined): string {
  if (!from || !to) return "";
  const ms = Date.parse(to) - Date.parse(from);
  if (!Number.isFinite(ms)) return "";
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** The latest of these instants, or null when none is readable. */
function latest(times: (string | null)[]): string | null {
  let best: string | null = null;
  for (const time of times) {
    if (!time || !Number.isFinite(Date.parse(time))) continue;
    if (best === null || Date.parse(time) > Date.parse(best)) best = time;
  }
  return best;
}

export type StepState = "done" | "active" | "todo" | "failed";

/** One step of a run: a glyph (its state), one word, one short value. */
export interface ProgressStep {
  key: "queued" | "runner" | "checking" | "done";
  label: string;
  state: StepState;
  /** A time since the attempt was queued, a count, or one or two words; empty while
   *  the step is still to do. */
  value: string;
}

type StepReading = [state: StepState, value: string];

/**
 * Where a run stands, as four steps: Queued, Runner, Checking, Done. `null` for a
 * build never checked: nothing was queued. Times count from the newest attempt's
 * `created_at`.
 *
 * A stall ends the strip at the step that failed: with `no_runner` nothing was
 * checked, so Checking and Done stay to do although the rows read `error`.
 */
export function checksProgress(
  data: Pick<BuildChecks, "state" | "checks" | "attempts" | "runner" | "stalled">
): ProgressStep[] | null {
  if (data.state === "none") return null;
  const rows = data.checks.filter((c) => !c.advisory);
  const judged = rows.filter((c) => c.status !== "queued" && c.status !== "running").length;
  const count = `${judged} of ${rows.length}`;
  const queuedAt = data.attempts[0]?.created_at;
  const noRunner = data.stalled === "no_runner";
  const stopped = data.stalled === "runner_stopped";
  const started = !nothingStarted(data);

  const runner = (): StepReading => {
    if (noRunner) return ["failed", "no answer"];
    if (data.runner) return ["done", clockText(queuedAt, data.runner.started_at)];
    // Rows that moved with no runner named: an older server, which names none.
    return started ? ["done", ""] : ["active", "starting"];
  };
  const checking = (): StepReading => {
    if (stopped) return ["failed", "stopped"];
    if (noRunner) return ["todo", ""];
    if (rows.length > 0 && judged === rows.length) return ["done", count];
    return data.state === "running" && started ? ["active", count] : ["todo", ""];
  };
  const done = (): StepReading => {
    if (noRunner || stopped) return ["todo", ""];
    const elapsed = clockText(queuedAt, latest(rows.map((c) => c.finished_at)));
    if (data.state === "passed") return ["done", elapsed];
    return data.state === "failed" ? ["failed", elapsed] : ["todo", ""];
  };

  const step = (key: ProgressStep["key"], label: string, [state, value]: StepReading) => ({
    key,
    label,
    state,
    value
  });
  return [
    step("queued", "Queued", ["done", "0:00"]),
    step("runner", "Runner", runner()),
    step("checking", "Checking", checking()),
    step("done", "Done", done())
  ];
}

/** The one line a stalled attempt gets above its rows; the rows carry their own
 *  summary from the server. */
export function stalledLine(stalled: BuildChecks["stalled"]): string | null {
  switch (stalled) {
    case "no_runner":
      return "The runner did not answer. Nothing was checked.";
    case "runner_stopped":
      return "The runner stopped before it finished.";
    default:
      return null;
  }
}

/** Where the runner's label links: its URL when that is a web address, else nowhere
 *  (a run by hand has none). The runner reports the URL itself, so anything that is
 *  not http(s) is shown as plain text rather than made clickable. */
export function runnerHref(runner: Pick<BuildChecksRunner, "url">): string | null {
  return runner.url && /^https?:\/\//i.test(runner.url) ? runner.url : null;
}

/** The heading's `queued 8 seconds ago` / `started a minute ago` / `checked 2 minutes
 *  ago`: what last happened to the newest attempt, and when. Nothing for a build that
 *  was never checked, which includes an attempt no runner was started for: the server
 *  answers `none` for it and still lists the attempt and its skipped rows. */
export function checkedWhen(
  data: Pick<BuildChecks, "state" | "attempts" | "runner" | "stalled">
): { verb: "queued" | "started" | "checked"; at: string } | null {
  const queuedAt = data.attempts[0]?.created_at;
  if (!queuedAt || data.state === "none") return null;
  // A settled attempt was checked; one still open, or one that stalled, only got so far.
  if (data.state !== "running" && stalledLine(data.stalled) === null) {
    return { verb: "checked", at: queuedAt };
  }
  return data.runner
    ? { verb: "started", at: data.runner.started_at }
    : { verb: "queued", at: queuedAt };
}

/** One row's status, in words and a tone. Only a failure is coloured red. */
export function checkStatus(status: string): { label: string; tone: AdminTone } {
  switch (status) {
    case "passed":
      return { label: "passed", tone: "ok" };
    case "failed":
      return { label: "failed", tone: "danger" };
    case "error":
      return { label: "could not run", tone: "danger" };
    default:
      // queued, running, skipped, and any status a later phase adds.
      return { label: status, tone: "muted" };
  }
}

/** Rows a failure opened, so the evidence that matters is on screen first. On a
 *  stalled attempt a row that could not run has no evidence to open to (the stall
 *  answered for it, and the line above the rows says so), so it stays closed. */
export function openByDefault(
  check: Pick<BuildCheck, "status">,
  stalled?: BuildChecks["stalled"]
): boolean {
  if (check.status === "error" && stalledLine(stalled) !== null) return false;
  return check.status === "failed" || check.status === "error";
}

export function findingTone(severity: string): AdminTone {
  if (severity === "error") return "danger";
  if (severity === "warning") return "warn";
  return "muted";
}

/** `850ms` / `1.2s`, the way the console's invocation rows say a duration. */
export function durationText(ms: number): string {
  return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
}

/** `Desk 1440` / `Phone 390`. */
export function viewportName(v: Pick<BuildCheckViewport, "name" | "width">): string {
  return `${v.name.charAt(0).toUpperCase()}${v.name.slice(1)} ${v.width}`;
}

/** `ready in 1.2s`, or `never ready` when the app never said it was. */
export function readyText(v: Pick<BuildCheckViewport, "ready" | "ready_ms">): string {
  if (!v.ready) return "never ready";
  return v.ready_ms == null ? "ready" : `ready in ${durationText(v.ready_ms)}`;
}

/** Whether `version` is older than `floor`, comparing the numeric parts. A missing
 *  version is below any floor; a missing floor holds nothing. */
export function belowFloor(version: string | null | undefined, floor: string | null | undefined) {
  if (!floor) return false;
  if (!version) return true;
  const parts = (v: string) =>
    v
      .split(/[.+-]/)
      .slice(0, 3)
      .map((p) => Number.parseInt(p, 10) || 0);
  const [a, b] = [parts(version), parts(floor)];
  for (let i = 0; i < 3; i++) {
    if ((a[i] ?? 0) !== (b[i] ?? 0)) return (a[i] ?? 0) < (b[i] ?? 0);
  }
  return false;
}

/** What "Run again" was told, in the console's words. The two 409s are expected
 *  answers, not faults; anything else carries the server's own message. */
export function rerunRefusal(error: unknown): string {
  if (apiStatus(error) === 409) {
    const code = apiErrorCode(error);
    if (code === "build_not_on_staging") {
      return "Staging doesn't serve this build, so its checks can't run again.";
    }
    if (code === "checks_running") return "Its checks are still running.";
  }
  return apiErrorMessage(error, "Couldn't run the checks again.");
}

/** Which build the Checks panel shows: the one `?checks=` names (by row id or
 *  `build_id`), else the build staging serves. `ref` is what the routes are called
 *  with — the row id when the build is retained, so every surface shares one cache
 *  entry. `null` when nothing is on staging and nothing was asked for. */
export interface ChecksTarget {
  ref: string;
  build: AppBuild | null;
  staging: boolean;
}

export function checksTarget(
  builds: readonly AppBuild[],
  requested: string | null
): ChecksTarget | null {
  if (requested) {
    const build = builds.find((b) => b.id === requested || b.build_id === requested) ?? null;
    return { ref: build?.id ?? requested, build, staging: build?.is_draft ?? false };
  }
  const staging = builds.find((b) => b.is_draft) ?? null;
  return staging ? { ref: staging.id, build: staging, staging: true } : null;
}

/** Which build's production attempt the panel can show: the one `?checks=` names,
 *  else the build production serves. `null` when nothing is live and nothing was
 *  asked for. */
export function productionTarget(
  builds: readonly AppBuild[],
  requested: string | null
): ChecksTarget | null {
  if (requested) return checksTarget(builds, requested);
  const live = builds.find((b) => b.is_published) ?? null;
  return live ? { ref: live.id, build: live, staging: live.is_draft } : null;
}
