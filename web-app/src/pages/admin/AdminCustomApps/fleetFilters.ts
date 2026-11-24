import type { CustomApp } from "@/types/apps";
import { type AppStatus, type HealthIndex, STATUS_LABEL, statusOf, statusRank } from "./appStatus";

/**
 * Filters that actually partition the fleet.
 *
 * The old toolbar offered "All 3 · Needs attention 3 · Not measured 3". Every chip
 * selected the same three apps, so the row cost a line of screen, invited three clicks,
 * and could not change what was on screen — and it *looked* like a working control,
 * which is worse than having none. That happened because the chips were a fixed list
 * with counts painted on, rather than a question asked of the data.
 *
 * One rule, and it is the whole module:
 *
 *   **A filter is offered iff it selects a proper, non-empty subset** — `0 < n < total`.
 *
 * A chip matching everything is the default by another name. A chip matching nothing is
 * a dead end. Both are removed at the source, so the row cannot regrow: on today's
 * fleet this returns `[]` and no chips render at all, which is the honest answer to
 * "how would you like these three identical apps narrowed?".
 *
 * Note this deliberately does *not* include an "All" chip. Clearing the filter is the
 * absence of a filter, not another filter — and adding one back would reintroduce
 * exactly the chip the rule exists to exclude.
 */
export interface FleetFilter {
  status: AppStatus;
  label: string;
  count: number;
}

export function discriminatingFilters(
  apps: readonly CustomApp[],
  health: HealthIndex | undefined
): FleetFilter[] {
  // No early return for a small fleet, deliberately. `count < total` below already
  // yields `[]` for one app (`1 < 1` is false) and for none (nothing to count), and a
  // mutation test caught the guard that used to sit here: flipping it changed no
  // outcome any test could see, because it never had one. A second spelling of the
  // rule is not a safety net — it is the thing that later gets "fixed" in a way the
  // rule does not survive.
  const total = apps.length;

  const counts = new Map<AppStatus, number>();
  for (const app of apps) {
    const status = statusOf(app, health);
    // `null` is "not known yet", not a status. Counting it would offer a chip for
    // the absence of an answer, and `statusOf` returns null precisely so that an
    // unmeasured app is never given a verdict it has not earned.
    if (status === null) continue;
    counts.set(status, (counts.get(status) ?? 0) + 1);
  }

  return [...counts.entries()]
    .filter(([, count]) => count > 0 && count < total)
    .map(([status, count]) => ({ status, label: STATUS_LABEL[status], count }))
    .sort((a, b) => statusRank(a.status) - statusRank(b.status));
}

/**
 * The filter that is actually in force: the chosen one, or none when this scope does not
 * offer it.
 *
 * **A filter that cannot be shown must not apply.** The rule above decides what is
 * offered *per scope*, and the column browser changes the scope under a chosen filter —
 * which the fleet keeps across the switch, because it is one component that re-renders
 * rather than remounts. Choose "Draft" over the whole deployment, then open an org with
 * no drafts, and `discriminatingFilters` correctly offers no chip while the chosen
 * status still filtered the rows: zero rows, "Nothing matches", and nothing left on
 * screen to switch off. Reproduced on the seeded fleet under `pokehouse-local` before
 * this existed, and the only ways out were a reload or guessing which other org would
 * bring the chip back.
 *
 * Deriving rather than clearing is what makes that unrepresentable instead of merely
 * fixed: there is no moment where the stored choice and the offered chips disagree about
 * what the table shows. It also means the choice survives a trip through a scope that
 * cannot honour it — go back to an org that has drafts and the chip is active again,
 * which is both visible on screen and what someone narrowing a fleet by hand expects.
 */
export function appliedStatus(
  status: AppStatus | null,
  offered: readonly FleetFilter[]
): AppStatus | null {
  if (status === null) return null;
  return offered.some((f) => f.status === status) ? status : null;
}
