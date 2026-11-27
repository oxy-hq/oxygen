import type { BuildCheckNote } from "./appBuildChecks";

/**
 * The plan a production move shows before it acts and answers after
 * (`POST /customer-apps/{id}/environments/{to}/promote|rollback`,
 * `custom_apps_promote::plan::Plan`). The same JSON `oxyc promote --dry-run`
 * prints, so the console and the CLI read one answer.
 */
export interface MovePlan {
  action: "promote" | "rollback";
  app_id: string;
  from: string | null;
  to: string;
  build: { id: string; build_id: string; published_by: string | null; created_at: string };
  current: { id: string; build_id: string } | null;
  /** What `to` serves now, sent back by the act (`null`: nothing). */
  expect_current: string | null;
  served_since: string | null;
  functions: { added: string[]; removed: string[]; changed: string[] };
  /** Production only. */
  schedules: {
    added: MoveCadence[];
    removed: MoveCadence[];
    changed: { function: string; from: MoveCadence; to: MoveCadence }[];
  } | null;
  migrations: { when: "before_move" | "queued_after"; oltp: string[]; airhouse: string[] };
  required_secrets_unset: string[];
  blockers: { code: string; message: string }[];
  warnings: string[];
  dry_run: boolean;
  moved: boolean;
  /** The checks that hold this move. `null` or absent with the org's
   *  `promote_checks_gate` flag off, on a rollback, and from a server predating it. */
  checks?: MovePlanChecks | null;
}

/** One row of the newest staging attempt of the build being promoted. */
export interface MovePlanCheckRow {
  kind: string;
  label: string;
  status: string;
  summary: string | null;
  required: boolean;
}

export interface MovePlanChecks {
  /** The kinds this app requires. */
  required: string[];
  attempt: number | null;
  state: string;
  rows: MovePlanCheckRow[];
  /** Required kinds not satisfied (`passed` and `skipped` satisfy). */
  unmet: string[];
  /** Required kinds still open. */
  running: string[];
  changes: { changed: number; compared: number; seen: boolean; seen_required: boolean } | null;
  /** The build's summary note, written by a model. */
  summary: BuildCheckNote | null;
  /** The caller may send `override_checks`. */
  overridable: boolean;
  /** Set on the act's answer when it carried an accepted override. */
  overridden: { reason: string } | null;
}

export interface MoveCadence {
  function: string;
  cron: string;
  timezone: string;
}

/** The blocker a confirm clears by sending `confirm_schedule_removals`. */
export const SCHEDULE_REMOVAL_UNCONFIRMED = "schedule_removal_unconfirmed";

/** The three blockers `override_checks` clears, and nothing else does. */
export const CHECKS_NOT_PASSED = "checks_not_passed";
export const CHECKS_RUNNING = "checks_running";
export const CHANGES_NOT_SEEN = "changes_not_seen";
export const CHECK_BLOCKER_CODES: readonly string[] = [
  CHECKS_NOT_PASSED,
  CHECKS_RUNNING,
  CHANGES_NOT_SEEN
];

/** The refusals of an override: the reason is too short, or the caller may not. */
export const OVERRIDE_REASON_REQUIRED = "override_reason_required";
export const OVERRIDE_NOT_ALLOWED = "override_not_allowed";
/** The fewest characters a reason has, after trimming. */
export const OVERRIDE_REASON_MIN = 10;

/** A promote past the check blockers, with why. */
export interface MoveOverride {
  reason: string;
}

/** Which production move a plan is for: the draft promoted from staging, or a
 *  rollback to a retained build. `buildId` is the `app_builds.id`. */
export interface MoveTarget {
  act: "promote" | "rollback";
  buildId: string;
}

/** The body of a plan or act request. */
export interface MoveRequest {
  dry_run?: boolean;
  confirm_schedule_removals?: string[];
  expect_current?: string | null;
  /** Promote past `checks_not_passed`, `checks_running` and `changes_not_seen`. */
  override_checks?: MoveOverride;
}
