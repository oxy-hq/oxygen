/**
 * One claimed attempt of a build's promote checks, driven to its verdicts.
 *
 * A STAGING attempt. The platform's two checks are already under way when a
 * claim answers: it wrote `validation`, and it started the build's check
 * functions, which this asks about until they have a verdict. The runner's
 * own are computed here and written one at a time, each as soon as it is
 * known, so the console fills in as they finish: `loads`, `design_system`,
 * then `smoke` and `changes` when the attempt has them (`screens-run.ts`).
 *
 * A PRODUCTION attempt has one check, `production_smoke`
 * (`production-run.ts`).
 *
 * Only what the claim left open is run, so an attempt of an app whose org has
 * no gate (the four first checks) is driven exactly as before.
 *
 * NOTHING HERE LEAVES A ROW OPEN BY ACCIDENT. A check that throws is written
 * as `error` with why. A verdict the platform would not take is tried again
 * (`promote-api.ts`), and one that never lands is remembered as what broke
 * the run rather than dropped.
 */

import type { Creds } from "../apps/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { checkDesignSystem, type StyleSource } from "./design-system.js";
import { observeAll } from "./loads-browser.js";
import { scoreLoads } from "./loads-score.js";
import { VIEWPORTS } from "./loads-target.js";
import { errorOutcome, messageOf, type Outcome } from "./outcome.js";
import { runProductionSmoke } from "./production-run.js";
import {
  type Claim,
  fetchStylesheets,
  type PromoteApi,
  type RunnerKind,
  reportCheck,
  settleFunctions
} from "./promote-api.js";
import { type BuildChecksDeps, claimTarget } from "./run-deps.js";
import { runScreenChecks } from "./screens-run.js";

export { type BuildChecksDeps, DEFAULT_DEPS } from "./run-deps.js";

/** One check, as this run left it. */
export interface BuildCheckLine {
  kind: string;
  /** Who computes the verdict. */
  by: "platform" | "runner";
  /** `passed`, `failed`, `error` or `skipped`; `running` for functions that had no verdict in time. */
  status: string;
  summary: string | null;
  /** A runner's check: whether the platform took the verdict. */
  reported?: boolean;
}

/** `loads`: open the build's page at both viewports and score what it showed. */
async function runLoads(claim: Claim, creds: Creds, deps: BuildChecksDeps): Promise<Outcome> {
  if (!claim.url) return errorOutcome("the platform named no page to open for this build");
  const target = claimTarget(claim, creds, deps, claim.environment || "staging", claim.build.id);
  const loads = await observeAll(target, VIEWPORTS, {
    load: deps.loadChromium,
    settleMs: deps.pageSettleMs,
    env: deps.env
  });
  return scoreLoads(target, loads, claim.identity || "app_checker");
}

/** `design_system`: the build's CSS through the theme guard, and its SDK against the floor. */
async function runDesignSystem(
  stylesheets: () => Promise<StyleSource[]>,
  claim: Claim
): Promise<Outcome> {
  let sources: StyleSource[];
  try {
    sources = await stylesheets();
  } catch (cause) {
    return errorOutcome(`the bundle's CSS could not be read: ${messageOf(cause)}`);
  }
  return checkDesignSystem(sources, claim.build.sdk_version ?? null, claim.sdk_floor);
}

function asCliError(cause: unknown): CliError {
  return cause instanceof CliError
    ? cause
    : new CliError(messageOf(cause), { code: ExitCode.FAILURE });
}

/** A refusal that asking again will not change. */
function isRefusal(cause: unknown): cause is CliError {
  return (
    cause instanceof CliError &&
    (cause.code === ExitCode.AUTH ||
      cause.code === ExitCode.NOT_FOUND ||
      cause.code === ExitCode.REQUEST)
  );
}

/** What driving one attempt left behind. */
export interface AttemptRun {
  /** Each check this run drove, in the order it got its verdict. */
  lines: BuildCheckLine[];
  /** The platform's roll-up after the last verdict written; null when none was. */
  state: string | null;
  /** The kinds still without a verdict, as the platform last said. */
  open: string[];
  /** The first request that could not be made: a verdict not written, a settle refused. */
  broke?: CliError;
  /** The build's check functions had no verdict in time. */
  unsettled: boolean;
}

export interface AttemptInput {
  api: PromoteApi;
  claim: Claim;
  creds: Creds;
  deps: BuildChecksDeps;
  /** Called as each check lands. */
  onLine?: (line: BuildCheckLine) => void;
}

/** One attempt being driven: what it was given, what it has left so far, and how a check lands. */
interface Drive {
  input: AttemptInput;
  run: AttemptRun;
  land: (line: BuildCheckLine) => void;
}

/** Drive what the claim left open. Never throws: what went wrong is in the result. */
export async function driveAttempt(input: AttemptInput): Promise<AttemptRun> {
  const run: AttemptRun = {
    lines: [],
    state: null,
    open: Array.from(input.claim.open),
    unsettled: false
  };
  const land = (line: BuildCheckLine) => {
    run.lines.push(line);
    input.onLine?.(line);
  };
  const drive: Drive = { input, run, land };
  // The functions run on the platform while the pages load here.
  await Promise.all([awaitFunctions(drive), runnerChecks(drive)]);
  return run;
}

/** Write one of the runner's verdicts, and say the check landed. */
async function report(drive: Drive, kind: RunnerKind, verdict: Outcome): Promise<void> {
  const { api, claim } = drive.input;
  let reported = false;
  try {
    const answer = await reportCheck(api, claim.attempt, kind, verdict);
    reported = true;
    drive.run.state = answer.state;
    drive.run.open = answer.open;
  } catch (cause) {
    drive.run.broke ??= asCliError(cause);
  }
  drive.land({ kind, by: "runner", status: verdict.status, summary: verdict.summary, reported });
}

/** Run one of the runner's checks and write its verdict; a throw is the verdict `error`. */
async function runAndReport(
  drive: Drive,
  kind: RunnerKind,
  check: () => Promise<Outcome>
): Promise<void> {
  if (!drive.input.claim.open.includes(kind)) return;
  let verdict: Outcome;
  try {
    verdict = await check();
  } catch (cause) {
    verdict = errorOutcome(messageOf(cause));
  }
  await report(drive, kind, verdict);
}

/** The runner's own checks, one after the other: production's one, or staging's. */
async function runnerChecks(drive: Drive): Promise<void> {
  const { api, claim, creds, deps } = drive.input;
  if (api.environment === "production") {
    await runAndReport(drive, "production_smoke", () => runProductionSmoke({ claim, creds, deps }));
    return;
  }
  // Read once for the run: the design-system check and the CSS digest share it.
  let sheets: Promise<StyleSource[]> | undefined;
  const stylesheets = () => {
    sheets ??= fetchStylesheets(api);
    return sheets;
  };
  await runAndReport(drive, "loads", () => runLoads(claim, creds, deps));
  await runAndReport(drive, "design_system", () => runDesignSystem(stylesheets, claim));
  await runScreenChecks({
    api,
    claim,
    creds,
    deps,
    wants: { smoke: claim.open.includes("smoke"), changes: claim.open.includes("changes") },
    stylesheets,
    land: (kind, verdict) => report(drive, kind, verdict)
  });
}

/** Ask the platform about the build's check functions until they have a verdict. */
async function awaitFunctions(drive: Drive): Promise<void> {
  const { api, claim, deps } = drive.input;
  if (!claim.open.includes("functions")) return;
  const deadline = Date.now() + deps.settleTimeoutMs;
  let summary: string | null = null;
  for (;;) {
    try {
      const settle = await settleFunctions(api, claim.attempt);
      summary = settle.summary;
      if (settle.done) {
        drive.land({ kind: "functions", by: "platform", status: settle.status, summary });
        return;
      }
    } catch (cause) {
      // A refusal ends the asking; anything else is asked again until the deadline.
      if (isRefusal(cause)) {
        drive.run.broke ??= cause;
        return;
      }
    }
    if (Date.now() >= deadline) {
      drive.run.unsettled = true;
      drive.land({ kind: "functions", by: "platform", status: "running", summary });
      return;
    }
    await deps.sleep(deps.settlePollMs);
  }
}
