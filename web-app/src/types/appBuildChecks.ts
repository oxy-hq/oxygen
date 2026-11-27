/**
 * A build's checks (`GET /customer-apps/{id}/builds/{build}/checks`,
 * `crates/api-custom-apps/src/custom_apps_build_checks/`): what ran on a build once
 * staging served it, and what each check found. With the org's `promote_checks_gate`
 * flag off the checks only report and a staging attempt has the first four kinds; with
 * it on a staging attempt adds `smoke`, `changes` and the model's `design_audit`, and a
 * production attempt (`?environment=production`) has `production_smoke` alone.
 *
 * `kind` and `status` stay open strings beside their known values: a later phase adds
 * kinds, and the console renders one it does not know by its label and findings.
 */

export type BuildCheckKind =
  | "validation"
  | "functions"
  | "loads"
  | "design_system"
  | "smoke"
  | "changes"
  | "production_smoke"
  | "design_audit";
/** Where an attempt ran. Absent on a request, the server reads staging. */
export type BuildChecksEnvironment = "staging" | "production";
export type BuildCheckStatus = "queued" | "running" | "passed" | "failed" | "error" | "skipped";
/** The roll-up over the non-advisory rows. `none`: the build was never checked. */
export type BuildChecksState = "none" | "running" | "passed" | "failed";

export interface BuildCheckFinding {
  severity: "error" | "warning" | "info" | string;
  message: string;
  /** `file:line`, a URL, or nothing. */
  at: string | null;
  rule: string | null;
}

export interface BuildCheckRef {
  kind: "file" | "url" | "screen" | "check" | string;
  label: string;
  href: string | null;
}

/** An advisory note on a check, or the build's summary. `model` is null only on a
 *  summary no model wrote (the workspace has none), whose text says why. */
export interface BuildCheckNote {
  author: "rule" | "model" | string;
  model: string | null;
  text: string;
  refs: BuildCheckRef[];
  created_at: string;
}

export interface BuildCheckFunctionRun {
  name: string;
  run_id: string;
  status: "done" | "failed" | "cancelled" | "timed_out" | "refused" | string;
  passed: boolean;
  error: string | null;
  duration_ms: number | null;
  invocation_id: string | null;
}

export interface BuildCheckFailedRequest {
  url: string;
  status: number | null;
  error: string | null;
}

export interface BuildCheckViewport {
  name: "desk" | "phone" | string;
  width: number;
  height: number;
  ready: boolean;
  ready_ms: number | null;
  environment: string | null;
  build_id: string | null;
  page_errors: string[];
  failed_requests: BuildCheckFailedRequest[];
  /** Writes the checker was refused (it never writes): a warning, never a failure.
   *  The contract names the list and not what one entry holds. */
  refused_writes?: unknown[];
  /** Requests to another origin the checker did not send (`"GET https://…"`): a checked
   *  page reaches only the app's own origin. A warning, never a failure. */
  refused_off_origin?: unknown[];
}

/** One screen of `smoke` / `production_smoke`: an app-relative path (`""` is the
 *  root), opened at each viewport. */
export interface BuildCheckScreen {
  path: string;
  viewports: BuildCheckViewport[];
}

/** One screen at one viewport of `changes`: the two pictures (file names under
 *  `GET …/checks/files/{name}`), the diff when they differ, and the share of pixels
 *  that does. */
export interface BuildCheckChangedScreen {
  path: string;
  viewport: string;
  changed: boolean;
  ratio: number | null;
  staging: string | null;
  production: string | null;
  diff: string | null;
  production_ready: boolean | null;
  production_errors: string[];
}

/** Who marked the changed screens seen (`POST …/checks/changes/seen`). */
export interface BuildCheckSeen {
  by: string;
  at: string;
}

/** An error production shows now and did not before the move. */
export interface BuildCheckNewError {
  path: string;
  viewport: string;
  error: string;
}

/** Every kind's detail carries `findings`; the rest is per kind, all optional so an
 *  older or newer row still reads. */
export interface BuildCheckDetail {
  findings?: BuildCheckFinding[];
  // validation
  validation_status?: string | null;
  validation_detail?: string | null;
  // functions
  functions?: BuildCheckFunctionRun[];
  // loads
  url?: string | null;
  identity?: string | null;
  viewports?: BuildCheckViewport[];
  // design_system
  sdk_version?: string | null;
  sdk_floor?: string | null;
  files?: string[];
  // smoke, production_smoke
  /** The runner read the screens off the root page; false: the manifest declared them. */
  /** The platform's mark on a row it closed as `skipped` because the check was never
   *  run (staging moved on first): such a skip satisfies no requirement. */
  not_checked?: boolean;
  discovered?: boolean;
  screens_omitted?: number;
  /** `smoke`: screens of the app the opened ones link to that the list did not hold, so
   *  no check opened them. The runner names the first fifty; the total counts them all.
   *  Read through `screensModel.unlistedScreens`. */
  screens_unlisted?: unknown;
  screens_unlisted_total?: number;
  /** `smoke` / `production_smoke`: `BuildCheckScreen[]`. `changes`:
   *  `BuildCheckChangedScreen[]`. Two shapes under one name, so it is read through
   *  `screensModel`, never directly. */
  screens?: unknown;
  // production_smoke
  baseline_at?: string | null;
  /** `BuildCheckNewError[]`, read through `screensModel`. */
  new_errors?: unknown;
  /** The move the check followed (`{action, by, at, previous}`), written once it failed. */
  move?: unknown;
  /** What the platform did about a failed check, where the app rolls back by itself:
   *  `{rolled_back: true, to: {id, build_id}, at}` or `{rolled_back: false, reason, at}`.
   *  Read through `screensModel.rollbackOf`. */
  rollback?: unknown;
  // changes
  production_build_id?: string | null;
  compared?: number;
  changed?: number;
  seen?: BuildCheckSeen | null;
}

export interface BuildCheck {
  kind: BuildCheckKind | string;
  label: string;
  status: BuildCheckStatus | string;
  /** Informs only; never part of the roll-up. */
  advisory: boolean;
  /** One line; null while queued. */
  summary: string | null;
  detail: BuildCheckDetail | null;
  notes: BuildCheckNote[];
  attempt: number;
  run_id: string;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  duration_ms: number | null;
}

export interface BuildChecksCounts {
  total: number;
  queued: number;
  running: number;
  passed: number;
  failed: number;
  error: number;
  skipped: number;
}

/** Who runs an attempt's checks: a GitHub Actions run, or the same command run by
 *  hand (then there is no URL). */
export interface BuildChecksRunner {
  label: string;
  url: string | null;
  started_at: string;
}

/** Why an attempt got no answer in time: no runner claimed it, or the one that did
 *  stopped before it finished. */
export type BuildChecksStalled = "no_runner" | "runner_stopped";

export interface BuildChecks {
  app_id: string;
  build: { id: string; build_id: string; created_at: string };
  attempt: number | null;
  environment: string;
  state: BuildChecksState | string;
  counts: BuildChecksCounts;
  /** In the fixed order: validation, functions, loads, design_system, then the kinds
   *  the gate adds. */
  checks: BuildCheck[];
  summary: BuildCheckNote | null;
  /** Newest first. */
  attempts: { attempt: number; state: BuildChecksState | string; created_at: string }[];
  /** Null until a runner claims the attempt. Optional: an older server sends none. */
  runner?: BuildChecksRunner | null;
  /** Set when the attempt got no answer in time: its open rows then read `error` and
   *  `state` is `failed`. Optional: an older server sends none. */
  stalled?: BuildChecksStalled | null;
}

/** `POST …/checks/rerun`, 202. */
export interface BuildChecksRerun {
  attempt: number;
  run_id: string;
  state: BuildChecksState | string;
}
