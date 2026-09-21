import { useQuery } from "@tanstack/react-query";
import { CustomAppsService } from "@/services/api/customApps";
import type { OxyAccessRow } from "@/types/apps";
import queryKeys from "../queryKey";

/**
 * Platform-wide list of workspaces that granted Oxy access, for the admin
 * Orgs / Projects browser. App-admin gated server-side (403 otherwise).
 */
export const useOxyAccessGrants = () =>
  useQuery<OxyAccessRow[]>({
    queryKey: queryKeys.oxyAccess.grants(),
    queryFn: CustomAppsService.listOxyAccess,
    // This is a full workspace scan, and the fleet landing now reads it for its
    // locked-workspace count — so a staff member bouncing in and out of /admin/apps
    // would re-run the scan each time.
    //
    // The cost is that `/admin/apps/access` ALSO serves cached inside the window:
    // React Query's default `refetchOnMount: true` only refetches a *stale* query, so
    // this is precisely what suppresses the audit page's refetch. That is accepted, not
    // overlooked — lockdown moves on human timescales and the page offers no manual
    // refresh whose result this could contradict. `refetchOnMount: "always"` would fix
    // the audit page and un-fix the landing, since both mount the same hook.
    staleTime: 60_000
  });
