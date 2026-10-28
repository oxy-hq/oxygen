import { useOptionalAuth } from "@/contexts/AuthContext";

/**
 * Whether the apps-first experience is on: the server's `apps_first_experience`
 * feature flag, mirrored on `GET /auth/config` so the screens without a workspace
 * (an org being set up, a person with no org) can read it too.
 *
 * On, the customer home and onboarding replace today's launcher and setup screens,
 * customers can open the read-only world model page, and the web app shows no
 * workspace management or creation ("Manage sets of apps", the set switcher, the
 * create flows). Off, everything is as it was. Local mode is always off: it has no
 * customers.
 *
 * Presentation only. Factory access, which the same flag enforces, is the server's
 * to decide; the web app reads that from `factory_access` on the workspace.
 *
 * Read once per page load: the auth config is fetched at boot, so flipping the flag
 * reaches an open tab on its next reload. Outside an `AuthProvider` (a component a
 * test renders on its own) it reads as off, today's behaviour.
 */
export default function useAppsFirstExperience(): boolean {
  const auth = useOptionalAuth();
  return !!auth && !auth.isLocalMode && auth.authConfig.apps_first_experience === true;
}
