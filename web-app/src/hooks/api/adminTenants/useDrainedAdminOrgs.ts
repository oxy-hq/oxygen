import { useEffect, useMemo } from "react";
import type { AdminOrgMeta } from "@/services/api/adminTenants";
import { useAllAdminOrgs } from "./useAdminOrgs";

/**
 * Every org on the deployment, all pages drained.
 *
 * `useAdminOrgsList({})` sends no page params and `/admin/orgs-meta` defaults to
 * **50, ordered by name** — so anything built on it silently answers "the orgs
 * alphabetically early enough". That is tolerable for a search box (you typed a name and
 * got nothing, which reads as absent) and wrong for anything that makes a
 * deployment-wide claim.
 *
 * Four call sites had reached for the capped list before this existed: the tenant rail's
 * Managed / Direct / **Empty** chips, the header's selected-org lookup, the
 * assign-to-org picker, and the grant-scope picker. "Empty — provisioned but never used"
 * is the clearest case: its entire value is exhaustiveness, and an operator hunting
 * abandoned tenants past the 50th would be told there are none.
 *
 * The drain loop was copy-pasted in `AccessPane` and nowhere else; this is that loop,
 * extracted, so a fifth caller gets it by default instead of by remembering. `AccessPane`
 * itself was finally migrated later — until then it kept its copy, and so missed the
 * error guard below, which is exactly the cost the extraction was meant to remove.
 */
export function useDrainedAdminOrgs({ enabled = true }: { enabled?: boolean } = {}): {
  orgs: AdminOrgMeta[];
  /** No page has arrived yet — nothing to show. */
  isLoading: boolean;
  /** Pages are still arriving: `orgs` is real but incomplete. */
  isDraining: boolean;
  /**
   * The drain stopped on a failure, so `orgs` is missing an unknown number of pages.
   * Any claim that depends on seeing every org must degrade rather than answer.
   */
  isIncomplete: boolean;
} {
  const { data, isLoading, hasNextPage, isFetchingNextPage, fetchNextPage, error } =
    useAllAdminOrgs(enabled);

  useEffect(() => {
    // Gated on `enabled` too: a disabled query reports `hasNextPage: false`, but a
    // caller that flips enabled mid-render should start draining, not sit on page one.
    //
    // And gated on `!error`, or a failing page is retried forever: React Query gives up
    // after its 3 retries, `isFetchingNextPage` drops to false, and `hasNextPage` is
    // still true — it is recomputed from the last *successful* page, whose length still
    // equals `ORGS_PAGE_SIZE` — so the effect re-fires and restarts the retry cycle.
    // Measured on the sibling app-registry walk: 18 requests in 30 seconds, on a ~7s
    // cycle, with no end, behind a UI that never stops looking busy.
    if (enabled && hasNextPage && !isFetchingNextPage && !error) fetchNextPage();
  }, [enabled, hasNextPage, isFetchingNextPage, fetchNextPage, error]);

  const orgs = useMemo(() => data?.pages.flat() ?? [], [data]);
  // Two states, not one. Collapsing them into `isLoading` meant a large deployment saw
  // a skeleton until the LAST page landed, where before the first 50 painted at once —
  // a first-paint regression for everyone who just wanted to type a name. Callers show
  // `orgs` as soon as `isLoading` clears, and use `isDraining` to disable the controls
  // whose correctness depends on the full set (the Empty / Managed / Direct chips).
  //
  // `isDraining` goes false once the drain has stopped, and a failed page stops it — so
  // it must not stay true on error, or every control it gates would be disabled forever.
  // That leaves `orgs` silently partial, which is why `isIncomplete` exists: a caller
  // whose correctness depends on exhaustiveness ("Empty — provisioned but never used")
  // must say it could not check rather than answer from the pages that did arrive.
  return {
    orgs,
    isLoading,
    isDraining: hasNextPage === true && !error,
    isIncomplete: Boolean(error)
  };
}
