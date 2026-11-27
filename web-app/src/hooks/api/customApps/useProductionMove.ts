import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CustomAppsService } from "@/services/api/customApps";
import type { MoveOverride, MovePlan, MoveTarget } from "@/types/appPromote";
import { CHECK_BLOCKER_CODES, SCHEDULE_REMOVAL_UNCONFIRMED } from "@/types/appPromote";
import queryKeys from "../queryKey";

/**
 * The dry-run plan of a production move, read while its confirm is open: the same
 * answer `oxyc promote --dry-run` prints. Not retried — the confirm waits on it, and
 * a plan that cannot be read holds the verb rather than letting it act unplanned.
 */
export function useMovePlan(appId: string, target: MoveTarget | null) {
  return useQuery({
    queryKey: queryKeys.customApps.movePlan(appId, target?.act ?? "", target?.buildId ?? ""),
    queryFn: () => CustomAppsService.moveProduction(appId, target as MoveTarget, { dry_run: true }),
    enabled: !!target,
    staleTime: 0,
    retry: false
  });
}

/** The blockers no confirm clears: everything but an unconfirmed schedule removal,
 *  which confirming the move itself confirms. */
export function hardBlockers(plan: MovePlan | undefined) {
  return (plan?.blockers ?? []).filter((b) => b.code !== SCHEDULE_REMOVAL_UNCONFIRMED);
}

/** The blockers a written reason clears (`override_checks`): a required check that
 *  failed or is still running, and changed screens nobody has seen. */
export function checkBlockers(plan: MovePlan | undefined) {
  return hardBlockers(plan).filter((b) => CHECK_BLOCKER_CODES.includes(b.code));
}

/**
 * Make the move the plan described. Sends back what the plan read production as
 * serving (`expect_current`), so a move that landed in between is refused
 * (`target_moved`) instead of acted on a stale plan, and names every schedule the
 * plan said it retires — the operator saw them listed in the confirm. `override`
 * promotes past the check blockers with the reason the operator wrote; without one
 * the request is what it always was.
 */
export function useMoveProduction(appId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      target,
      plan,
      override
    }: {
      target: MoveTarget;
      plan: MovePlan;
      override?: MoveOverride;
    }) =>
      CustomAppsService.moveProduction(appId, target, {
        expect_current: plan.expect_current,
        confirm_schedule_removals: (plan.schedules?.removed ?? []).map((s) => s.function),
        ...(override ? { override_checks: { reason: override.reason.trim() } } : {})
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.customApps.all() });
      queryClient.invalidateQueries({ queryKey: queryKeys.customApps.builds(appId) });
    },
    // A refused act (`target_moved`, a blocker that appeared) re-reads the plan, so the
    // confirm shows what the server now says before the operator tries again. A reset,
    // not an invalidate: the refused plan is dropped, so the confirm holds while the
    // re-read runs and stays held if it fails, instead of offering the plan just refused.
    onError: (_err, { target }) => {
      void queryClient.resetQueries({
        queryKey: queryKeys.customApps.movePlan(appId, target.act, target.buildId)
      });
    }
  });
}
