import { apiErrorCode, apiStatus } from "@/libs/apiError";
import type { AdminTone } from "@/pages/admin/components/adminTone";
import type { BuildCheckNote } from "@/types/appBuildChecks";
import {
  CHANGES_NOT_SEEN,
  CHECKS_NOT_PASSED,
  CHECKS_RUNNING,
  type MovePlan,
  type MovePlanCheckRow,
  OVERRIDE_NOT_ALLOWED,
  OVERRIDE_REASON_MIN,
  OVERRIDE_REASON_REQUIRED
} from "@/types/appPromote";
import { readNote } from "../AppConsole/BuildChecks/notesModel";
import { checkBlockerLine } from "../AppConsole/productionGates";

/**
 * The pure half of a promote confirm once checks hold Promote: the plan's `checks`
 * read into what the confirm draws, the words of its three blockers, and the rule of
 * the reason that promotes past them.
 *
 * `plan.checks` is `null` with the org's flag off, on a rollback, and from a server
 * that predates it; `planChecks` then answers `null` and the confirm is what it was.
 * The server writes this shape in parallel with the console, so each field is read
 * with a default rather than trusted.
 */

const list = <T>(v: T[] | null | undefined): T[] => (Array.isArray(v) ? v : []);

export interface PlanChecks {
  /** The rows that hold this promote, in the checks list's order. */
  required: MovePlanCheckRow[];
  /** Labels of the required checks that ended without passing. */
  unmet: string[];
  /** Labels of the required checks still open. */
  running: string[];
  changes: { changed: number; compared: number; seen: boolean; seenRequired: boolean } | null;
  /** The build's summary, written by a model: a note, never a verdict. */
  summary: BuildCheckNote | null;
  /** The caller may promote past the check blockers with a reason. */
  overridable: boolean;
}

export function planChecks(plan: Pick<MovePlan, "checks"> | undefined): PlanChecks | null {
  const checks = plan?.checks;
  if (!checks) return null;
  const rows = list(checks.rows);
  const requiredKinds = list(checks.required);
  const label = (kind: string) => rows.find((r) => r.kind === kind)?.label ?? kind;
  const running = list(checks.running);
  const changes = checks.changes;
  return {
    required: rows.filter((r) => r.required === true || requiredKinds.includes(r.kind)),
    // A check still open is not "failed": it is named once, under running.
    unmet: list(checks.unmet)
      .filter((kind) => !running.includes(kind))
      .map(label),
    running: running.map(label),
    changes: changes
      ? {
          changed: Number(changes.changed) || 0,
          compared: Number(changes.compared) || 0,
          seen: changes.seen === true,
          seenRequired: changes.seen_required === true
        }
      : null,
    summary: readNote(checks.summary),
    overridable: checks.overridable === true
  };
}

/** A check blocker's callout: a few words, then the checks or the screens they are
 *  about. `null` for a blocker that is not one of the three, and for any of them on a
 *  plan that carries no `checks` (the confirm then says the server's own message). */
export function checkBlockerText(code: string, checks: PlanChecks | null): string | null {
  if (!checks) return null;
  switch (code) {
    case CHECKS_NOT_PASSED:
      return checkBlockerLine(code, checks.unmet);
    case CHECKS_RUNNING:
      return checkBlockerLine(code, checks.running);
    case CHANGES_NOT_SEEN: {
      const changed = checks.changes?.changed ?? 0;
      return checkBlockerLine(
        code,
        changed ? [`${changed} screen${changed === 1 ? "" : "s"}`] : []
      );
    }
    default:
      return null;
  }
}

/** What a reason would promote past: the blockers' codes, and one chip for each
 *  check or change they are about. */
export interface OverrideOffer {
  codes: string[];
  past: { label: string; tone: AdminTone }[];
}

/**
 * The offer to promote past the check blockers, or `undefined` when there is none to
 * make: the caller may not (`overridable`), no check blocker stands, or another
 * blocker stands beside them, which no reason clears.
 */
export function overrideOffer(
  held: readonly { code: string }[],
  checkCodes: readonly string[],
  checks: PlanChecks | null
): OverrideOffer | undefined {
  if (!checks?.overridable || checkCodes.length === 0 || checkCodes.length !== held.length) {
    return undefined;
  }
  const past: OverrideOffer["past"] = [];
  if (checkCodes.includes(CHECKS_NOT_PASSED)) {
    for (const label of checks.unmet) past.push({ label, tone: "danger" });
  }
  if (checkCodes.includes(CHECKS_RUNNING)) {
    for (const label of checks.running) past.push({ label: `${label} · running`, tone: "muted" });
  }
  if (checkCodes.includes(CHANGES_NOT_SEEN)) past.push({ label: "Changes not seen", tone: "warn" });
  return { codes: [...checkCodes], past };
}

/** The override step's reason, as its counter and its button read it: `6 / 10` while
 *  it is short of the ten the server asks for, then the count alone. The server counts
 *  the characters left after trimming, so this does too. */
export function reasonState(reason: string): { count: number; ok: boolean; text: string } {
  const count = reason.trim().length;
  const ok = count >= OVERRIDE_REASON_MIN;
  return { count, ok, text: ok ? `${count}` : `${count} / ${OVERRIDE_REASON_MIN}` };
}

/** An act refused because of the override itself, in words for under the reason.
 *  `null` for any other failure, which the verb toasts as it always did. */
export function overrideRefusal(error: unknown): string | null {
  const code = apiErrorCode(error);
  if (apiStatus(error) === 403 && code === OVERRIDE_NOT_ALLOWED) {
    return "Only an org owner or admin can promote past a check.";
  }
  if (code === OVERRIDE_REASON_REQUIRED) {
    return `The reason needs at least ${OVERRIDE_REASON_MIN} characters.`;
  }
  return null;
}
