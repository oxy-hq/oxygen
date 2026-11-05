import { useMemo } from "react";
import { useDrainedAdminOrgs } from "../adminTenants/useDrainedAdminOrgs";

/**
 * Every org (id + name) for an admin to pick from, all pages drained. `isLoading` holds
 * until the last page lands; `isIncomplete` means a page failed and `orgs` is partial, so
 * a name missing from it may still exist.
 */
export const useAdminOrgOptions = () => {
  const { orgs, isLoading, isDraining, isIncomplete, refetch } = useDrainedAdminOrgs();
  const options = useMemo(() => orgs.map((o) => ({ id: o.id, name: o.name })), [orgs]);
  return { orgs: options, isLoading: isLoading || isDraining, isIncomplete, refetch };
};
