/**
 * What `oxyc promote` says and does about a build's checks, where the app's
 * org holds promotes on them. The plan then carries `checks`; with no gate it
 * is `null` and nothing in this file prints or waits.
 *
 * BEFORE THE MOVE. The plan's lines list every check of the build's newest
 * staging attempt, the required ones marked, with what changed on screen and
 * the summary a model wrote. Three blockers come from the checks, and each
 * reads as what to do next ({@link checksAdvice}). An owner or admin of the
 * app's org may promote past them with `--override-checks "<reason>"`; the
 * reason is kept in the audit log, so an empty one is refused here.
 *
 * AFTER THE MOVE. Production is checked once it serves the build
 * ("Production opens after promote"). {@link awaitProductionCheck} reads that
 * attempt until it has a verdict.
 */

import type { Creds } from "../apps/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { type BuildChecks, type CheckNote, fetchChecks } from "./checks-show.js";
import { CHECK_LABELS, noteLines } from "./checks-show-render.js";

/** One check of the build being promoted, as the plan lists it. */
export interface PlanCheckRow {
  kind: string;
  label?: string | null;
  status: string;
  summary: string | null;
  /** Whether a promote waits on it. */
  required: boolean;
}

/** The plan's `checks`, snake_case as the server sends it. */
export interface PlanChecks {
  required: string[];
  attempt: number | null;
  state: string;
  rows: PlanCheckRow[];
  /** Required kinds that are not satisfied. */
  unmet: string[];
  /** Required kinds still without a verdict. */
  running: string[];
  changes: { changed: number; compared: number; seen: boolean; seen_required: boolean } | null;
  summary: CheckNote | null;
  /** Whether this caller may promote past the checks. */
  overridable: boolean;
  /** Set when the request carried an override that was accepted. */
  overridden: { reason: string } | null;
}

/** The shortest reason an override takes, after trimming: the platform's own floor. */
const MIN_REASON_CHARS = 10;

/** `--override-checks`: the reason, trimmed; refused before any request when it says too little. */
export function parseOverrideReason(raw: string | undefined): string | undefined {
  if (raw === undefined) return undefined;
  const reason = raw.trim();
  if (Array.from(reason).length < MIN_REASON_CHARS) {
    throw usageError(
      `--override-checks needs the reason, in at least ${MIN_REASON_CHARS} characters: it is kept in the audit log`,
      'e.g. --override-checks "hotfix for the checkout outage; the design finding is cosmetic"'
    );
  }
  return reason;
}

function mark(status: string): string {
  switch (status) {
    case "passed":
      return out.green("✓");
    case "failed":
      return out.red("✗");
    case "error":
      return out.yellow("!");
    case "skipped":
      return out.dim("-");
    default:
      return out.cyan("…");
  }
}

function labelOf(row: PlanCheckRow): string {
  return row.label ?? CHECK_LABELS[row.kind] ?? row.kind;
}

/** `4 of 28 screens differ · not seen by a person yet`: nothing when no screen differs. */
function changesLine(changes: PlanChecks["changes"]): string[] {
  if (!changes || changes.changed === 0) return [];
  const seen = changes.seen
    ? "seen by a person"
    : `not seen by a person yet${changes.seen_required ? " (this app needs them seen before a promote)" : ""}`;
  return [`    ${changes.changed} of ${changes.compared} screens differ · ${seen}`];
}

/**
 * The plan's lines about the build's checks: one per check, the required ones
 * marked, then what changed, the summary note and an accepted override.
 */
export function planCheckLines(checks: PlanChecks | null | undefined): string[] {
  if (!checks) return [];
  const rows = Array.isArray(checks.rows) ? checks.rows : [];
  if (checks.attempt === null || rows.length === 0) {
    return ["  checks: none have run on this build", ...overrideLine(checks)];
  }
  const width = Math.max(0, ...rows.map((row) => labelOf(row).length));
  const lines = [`  checks (attempt ${checks.attempt}): ${checks.state}`];
  for (const row of rows) {
    const required = row.required ? "required" : "        ";
    const words = row.summary ?? row.status;
    lines.push(
      `    ${mark(row.status)} ${required}  ${labelOf(row).padEnd(width)}  ${words}`.trimEnd()
    );
  }
  lines.push(...changesLine(checks.changes));
  if (checks.summary) lines.push(...noteLines(checks.summary, "summary", "    ", out));
  lines.push(...overrideLine(checks));
  return lines;
}

function overrideLine(checks: PlanChecks): string[] {
  return checks.overridden
    ? [
        `    ${out.yellow("overridden")} the checks do not hold this promote: ${checks.overridden.reason}`
      ]
    : [];
}

export const CHECKS_NOT_PASSED = "checks_not_passed";
export const CHECKS_RUNNING = "checks_running";
export const CHANGES_NOT_SEEN = "changes_not_seen";

/** What the advice names: the app as the caller typed it, the build, and whether they may override. */
export interface AdviceFor {
  app: string;
  build: string;
  overridable: boolean;
}

/**
 * What to do next about a blocker the checks raise; `undefined` for any other
 * blocker. The override is offered only to a caller the plan says may use it.
 */
export function checksAdvice(code: string, about: AdviceFor): string | undefined {
  const show = `oxyc checks show ${about.app} --build ${about.build}`;
  const past = about.overridable
    ? '; or promote past them with --override-checks "<why>" (audited)'
    : "";
  switch (code) {
    case CHECKS_NOT_PASSED:
      return `fix what the checks found and publish again (\`${show}\` prints each finding), or run them again from the app's Builds page${past}`;
    case CHECKS_RUNNING:
      return `wait for the checks, then promote again: \`${show} --wait\`${past}`;
    case CHANGES_NOT_SEEN:
      return `open the app's Builds page, look at the screens that differ and mark them seen, then promote again${past}`;
    default:
      return undefined;
  }
}

/** The waits around production's check. The tests shorten them. */
export interface ProductionWaitDeps {
  /** How often the attempt is read. */
  pollMs: number;
  /** How long it is read for. */
  timeoutMs: number;
  /** How long "no check has run" is believed to be "not started yet". */
  graceMs: number;
  sleep: (ms: number) => Promise<void>;
}

export const PRODUCTION_WAIT: ProductionWaitDeps = {
  pollMs: 5_000,
  timeoutMs: 5 * 60_000,
  graceMs: 30_000,
  sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms))
};

/** How the wait ended. */
export interface ProductionWait {
  /**
   * `passed` and `failed` are the verdict. `running`: none within the time.
   * `none`: no production check was started for this build. `unreadable`:
   * the attempt could not be read at all.
   */
  outcome: "passed" | "failed" | "running" | "none" | "unreadable";
  /** The attempt as last read. */
  body?: BuildChecks;
  /** Why it could not be read. */
  error?: CliError;
}

/** A refusal that reading again will not change. */
function isRefusal(cause: unknown): cause is CliError {
  return cause instanceof CliError && cause.code !== ExitCode.UNAVAILABLE;
}

/**
 * Read the production attempt of `build` (its row id) until it has a verdict.
 * Never throws: how it ended is the answer.
 */
export async function awaitProductionCheck(
  creds: Creds,
  appId: string,
  build: string,
  deps: ProductionWaitDeps
): Promise<ProductionWait> {
  const started = Date.now();
  let body: BuildChecks | undefined;
  let error: CliError | undefined;
  for (;;) {
    try {
      body = await fetchChecks(creds, appId, build, { environment: "production" });
      error = undefined;
    } catch (cause) {
      if (isRefusal(cause)) return { outcome: "unreadable", body, error: cause };
      error = cause instanceof CliError ? cause : new CliError(String(cause));
    }
    const ended = endOfWait({ body, error }, started, deps);
    if (ended) return ended;
    await deps.sleep(deps.pollMs);
  }
}

/**
 * How the wait ends on the attempt as last read (and why the newest read
 * failed, when it did), for a wait that began at `started`; `undefined` while
 * it goes on.
 */
function endOfWait(
  last: { body?: BuildChecks; error?: CliError },
  started: number,
  deps: ProductionWaitDeps
): ProductionWait | undefined {
  const { body, error } = last;
  if (body?.state === "passed" || body?.state === "failed") return { outcome: body.state, body };
  const waited = Date.now() - started;
  if (body?.state === "none" && waited >= deps.graceMs) return { outcome: "none", body };
  if (waited < deps.timeoutMs) return undefined;
  return body ? { outcome: "running", body } : { outcome: "unreadable", error };
}
