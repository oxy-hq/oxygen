import { useCallback } from "react";
import { useActingSession } from "@/hooks/api/adminAssume/useActingSession";
import { useOrgs } from "@/hooks/api/organizations";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";

/**
 * Whether the caller is Oxy staff the Factory admits by `operate_platform`
 * (`Ring::Factory`): they get the "Open Factory" flow and can move between
 * organizations' Factories. An App Operator is staff (`is_app_admin`) without it.
 * A Global Owner reports every capability; `is_owner` covers a response that omits
 * the list.
 *
 * Presentation only: the Factory's routes and the staff console decide on their own.
 */
export function useOperatesPlatform(): boolean {
  const { data: profile } = useCurrentUser();
  return !!(profile?.is_owner || profile?.platform_capabilities?.includes("operate_platform"));
}

/**
 * Whether opening an org's Factory can skip the assume-role session: the caller is
 * a real member of the org (and not acting as anyone), or is already acting as it.
 * Everyone else needs a session first (`OpenFactoryDialog`).
 *
 * Decided before anything is drawn, so a person who can walk straight in never sees
 * a dialog open and close on the way.
 */
export function useOpensFactoryDirectly(): (orgId: string) => boolean {
  const { session, isActing } = useActingSession();
  const { data: myOrgs } = useOrgs();
  const actingOrgId = session?.org_id;
  return useCallback(
    (orgId: string) =>
      isActing ? actingOrgId === orgId : (myOrgs?.some((o) => o.id === orgId) ?? false),
    [isActing, actingOrgId, myOrgs]
  );
}
