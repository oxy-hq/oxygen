/**
 * Derive a suggested backfill window from the resources' source contracts.
 *
 * Pure — no React, no clock of its own (`now` is injected) — so the rules
 * are unit-testable the way `contractDisplay` and `airwayReducer` are. The
 * component that renders the result is `BackfillWindowHint`.
 *
 * ## Why `restatement_window`, and why not `rewind`
 *
 * airway never widens a pull from a contract: `rewind()`, `cursor_lag()` and
 * `restatement_window()` have no consumer in 0.1.23 outside its own tests. A
 * contract is a **label**. So this is oxy reading the label and computing an
 * explicit `[from, to)` that it hands to the existing backfill endpoint.
 *
 * The endpoint's window is **not** a cursor window. On the incremental path a
 * source windows on its modification axis (`modifiedStartDate`/
 * `modifiedEndDate` for Toast, `MetaData.LastUpdatedTime` for QuickBooks); a
 * backfill deliberately switches axes to the **business/event** one
 * (`startDate`/`endDate` — clock-in — for Toast, `TxnDate` for QuickBooks)
 * because an operator replaying history names a *period of business*, not a
 * span of the vendor's write activity. A backfill also suppresses the
 * client-side cursor floor entirely, so the persisted cursor plays no part.
 *
 * Two consequences decide the derivation:
 *
 * 1. The span is `restatement_window`, **not** `rewind`. `rewind` is
 *    `cursor_lag + restatement_window` — how far back of *the cursor* a
 *    cursor-driven pull has to start. `cursor_lag` describes index visibility
 *    (written at T, indexed at T+lag) on an axis this request does not use, so
 *    folding it in would be an axis mix-up that also makes the explanation
 *    unreadable ("corrected for up to 3d 30s").
 * 2. `now - restatement_window → now` is right on the event axis for the
 *    reason the arithmetic suggests but not the one it looks like: a row still
 *    open to correction is one whose *event* is at most `restatement_window`
 *    old, and selecting it on the backfill axis means naming its event date.
 *
 * `requires_partition_repull` resources (opaque, or versioned on a cursor that
 * does not move on correction) are exactly the ones a date-range re-read is
 * the right remedy for — but airway is explicit that for them a declared
 * window is "a lower bound on how far back partitions stay open", not a
 * sufficient window. Those contribute, flagged {@link ContractWindow.floorOnly}
 * so the UI can say "at least" rather than "up to".
 */

import type { ResourceContract } from "@/services/api/airway";
import type { ResourceRow } from "@/utils/airwayReducer";

import { formatDurationMs } from "./contractDisplay";

/** Why a resource contributes nothing to the suggestion. Four reasons, not
 *  one: an operator has to act differently on each, and collapsing them is
 *  how "unknown" becomes "zero". */
export type ExclusionReason =
  /** Rows are never corrected — there is nothing to re-pull. */
  | "immutable"
  /** Declared mutable, but the contract names no width. The source can
   *  restate and nobody has said how far back; that is *unknown*, and a
   *  substituted default would read as a vendor fact. */
  | "no_window"
  /** The connector was asked and declared nothing at all. */
  | "undeclared"
  /** The run's event stream carried no contract for this row — a run recorded
   *  before contracts rode on `pipeline_plan`. Distinct from `undeclared`:
   *  nobody was ever asked. */
  | "unreported";

type ContractWindow = {
  resource: string;
  /** Declared `restatement_window_ms`. */
  windowMs: number;
  /** The contract says a cursor window is not sufficient here, so this width
   *  is a floor on how long partitions stay open — not a promise the range is
   *  enough. */
  floorOnly: boolean;
};

type ExcludedResource = { resource: string; reason: ExclusionReason };

type SuggestedWindow = {
  /** `YYYY-MM-DD` UTC, matching the modal's date inputs. `toDate` is
   *  **inclusive** — the modal pushes it to the next UTC midnight for the
   *  half-open backend bound. */
  fromDate: string;
  toDate: string;
  /** The widest declared window this range came from. */
  windowMs: number;
  /** The resource that declared it. */
  resource: string;
  /** True when that resource only treats the width as a lower bound. */
  floorOnly: boolean;
};

export type BackfillSuggestion = {
  /**
   * `null` when **no** resource declares a restatement window. There is no
   * answer in that case, and the modal must stay empty rather than invent a
   * range that would look authoritative.
   */
  window: SuggestedWindow | null;
  /** Every declared window, widest first — what the disclosure renders. */
  declared: ContractWindow[];
  /** Resources the suggestion does not speak for, in plan order. */
  excluded: ExcludedResource[];
  /** True when the declared widths are not all equal: a single window cannot
   *  be "correct" for all of them and the UI must say so. */
  disagree: boolean;
};

export const EXCLUSION_LABEL: Record<ExclusionReason, string> = {
  immutable: "immutable — nothing to re-pull",
  no_window: "can restate — no window declared",
  undeclared: "undeclared — the connector said nothing",
  unreported: "this run reported no contract"
};

/** `Date` → `YYYY-MM-DD` in UTC, the shape an `<input type='date'>` wants. */
const utcDate = (d: Date): string => d.toISOString().slice(0, 10);

/**
 * One resource's contribution: a window, or the reason it has none.
 *
 * `immutable` is checked before the width. airway's builders refuse
 * `restating_within` on an immutable contract so the two cannot co-occur
 * today, but the ordering means a future contract that carried both would
 * still contribute nothing rather than propose re-pulling a resource whose
 * rows are never corrected.
 */
function classify(contract: ResourceContract): ContractWindow | ExclusionReason {
  if (contract.mutability === "undeclared") return "undeclared";
  if (contract.mutability === "immutable") return "immutable";
  const windowMs = contract.restatement_window_ms;
  if (windowMs == null || !Number.isFinite(windowMs) || windowMs < 0) return "no_window";
  return {
    resource: contract.resource,
    windowMs,
    floorOnly: contract.requires_partition_repull === true
  };
}

/**
 * Fold a run's resource rows into a suggestion.
 *
 * Rows with a `parent` are skipped outright: a normalized child table is not a
 * source resource, has no contract of its own, and listing it as "unknown"
 * would misdescribe it. A parentless row with no contract *is* reported —
 * as `unreported`, which is a real thing an operator can act on (re-run the
 * pipeline once and the contracts arrive).
 *
 * `now` is injected so the derivation is testable against a fixed clock.
 */
export function deriveBackfillSuggestion(
  resources: ResourceRow[],
  now: Date = new Date()
): BackfillSuggestion {
  const declared: ContractWindow[] = [];
  const excluded: ExcludedResource[] = [];

  for (const row of resources) {
    if (row.parent) continue;
    if (!row.contract) {
      excluded.push({ resource: row.table, reason: "unreported" });
      continue;
    }
    const outcome = classify(row.contract);
    if (typeof outcome === "string") {
      excluded.push({ resource: row.table, reason: outcome });
    } else {
      declared.push(outcome);
    }
  }

  // Widest first; ties broken by name so the disclosure order is stable.
  declared.sort((a, b) => b.windowMs - a.windowMs || a.resource.localeCompare(b.resource));

  const widest = declared[0];
  const disagree = declared.some((d) => d.windowMs !== declared[0].windowMs);

  if (!widest || !Number.isFinite(now.getTime())) {
    return { window: null, declared, excluded, disagree };
  }

  // Floor both bounds to the UTC calendar day. That widens the range (the
  // day start is at or before `now - windowMs`) — the safe direction for a
  // re-read, and the only shape a date input can express.
  const from = new Date(now.getTime() - widest.windowMs);
  return {
    window: {
      fromDate: utcDate(from),
      toDate: utcDate(now),
      windowMs: widest.windowMs,
      resource: widest.resource,
      floorOnly: widest.floorOnly
    },
    declared,
    excluded,
    disagree
  };
}

/**
 * The one sentence that makes the pre-filled range explicable rather than
 * magic — e.g. "orders can be corrected for up to 3d after the fact."
 *
 * `null` when there is no suggestion; the caller renders the absence case,
 * which is a different message entirely.
 */
export function suggestionRationale(suggestion: BackfillSuggestion): string | null {
  const w = suggestion.window;
  if (!w) return null;
  const width = formatDurationMs(w.windowMs);
  return w.floorOnly
    ? `${w.resource} is re-pulled whole by partition; its contract keeps partitions open for at least ${width}.`
    : `${w.resource} can be corrected for up to ${width} after the fact.`;
}
