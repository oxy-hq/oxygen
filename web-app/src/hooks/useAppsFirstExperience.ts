import { useOptionalAuth } from "@/contexts/AuthContext";
import { useOrgFeatureFlags } from "@/hooks/api/featureFlags";
import useCurrentOrg from "@/stores/useCurrentOrg";

/** The server's flag key. */
export const APPS_FIRST_EXPERIENCE = "apps_first_experience";

/**
 * Whether the apps-first experience is on for an org: the server's
 * `apps_first_experience` feature flag, which is targeted per org.
 *
 * On, the customer home and onboarding replace today's launcher and setup screens,
 * customers can open the read-only world model page, and the web app shows no
 * workspace management or creation ("Manage sets of apps", the set switcher, the
 * create flows). Off, everything is as it was. Local mode is always off: it has no
 * customers.
 *
 * The org is `orgId`, else the current org. With one, the value is the org's own
 * (`GET /orgs/{org_id}/feature-flags`); until that answers, and on the screens with
 * no org (a person with no org yet), it is what an org with no override gets, from
 * `GET /auth/config`. Pass `orgId` where the org is known before the current-org
 * store is primed (the dispatchers at `/` and `/:orgSlug`).
 *
 * Presentation only. Factory access, which the same flag enforces, is the server's
 * to decide; the web app reads that from `factory_access` on the workspace.
 *
 * Outside an `AuthProvider` (a component a test renders on its own) it reads as off,
 * today's behaviour.
 */
export default function useAppsFirstExperience(orgId?: string): boolean {
  return useAppsFirstExperienceState(orgId).enabled;
}

/**
 * The same value, and whether the org's own answer is still on its way. A screen that
 * redirects on the flag waits while `isPending`: until then `enabled` is only what an
 * org with no override gets, and a redirect taken on it would stick (`replace`).
 */
export function useAppsFirstExperienceState(orgId?: string): {
  enabled: boolean;
  isPending: boolean;
} {
  const auth = useOptionalAuth();
  const currentOrgId = useCurrentOrg((s) => s.org?.id);
  const cloud = !!auth && !auth.isLocalMode;
  const id = cloud ? (orgId ?? currentOrgId) : undefined;
  const { data, isPending } = useOrgFeatureFlags(id);
  if (!cloud) return { enabled: false, isPending: false };
  const flag = data?.flags?.[APPS_FIRST_EXPERIENCE];
  return {
    enabled: flag ? flag.enabled : auth.authConfig.apps_first_experience === true,
    // A disabled query (no org) also reports pending; only an asked one is waited on.
    isPending: !!id && isPending === true
  };
}
