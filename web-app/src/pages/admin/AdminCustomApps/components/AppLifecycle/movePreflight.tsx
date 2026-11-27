import type { useMovePlan } from "@/hooks/api/customApps/useProductionMove";
import { checkBlockers, hardBlockers } from "@/hooks/api/customApps/useProductionMove";
import type { LifecyclePreflight } from "./LifecycleCard";
import { MovePlanSummary } from "./MovePlanSummary";
import { overrideOffer, planChecks } from "./planChecks";

/**
 * What a production move's confirm says, and whether its verb waits: the dry-run
 * plan, and the one hold rule every surface that confirms a move uses — the
 * lifecycle card and the release timeline's Roll back alike.
 *
 * A plan being read or re-read, one whose last read failed, and one with a blocker
 * no confirm clears all hold the verb: Confirm would otherwise act on a plan the
 * server may have refused, or never showed.
 *
 * A required check that did not pass holds the verb like any other blocker. When
 * the check blockers are the only ones standing and the plan says the caller may
 * (`checks.overridable`), the confirm also offers to promote past them with a
 * written reason: `override`.
 */
export function movePreflight(plan: ReturnType<typeof useMovePlan>): LifecyclePreflight {
  const held = hardBlockers(plan.data);
  const ready = !plan.isFetching && !plan.error && !!plan.data;
  return {
    notice: <MovePlanSummary plan={plan.data} isLoading={plan.isLoading} error={plan.error} />,
    hold: !ready || held.length > 0,
    override: ready
      ? overrideOffer(
          held,
          checkBlockers(plan.data).map((b) => b.code),
          planChecks(plan.data)
        )
      : undefined
  };
}
