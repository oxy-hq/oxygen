import { useOptionalAuth } from "@/contexts/AuthContext";
import useAppsFirstExperience from "./useAppsFirstExperience";

/**
 * Whether home page v3 is on: the server's `home_v3` feature flag, on top of the
 * apps-first experience (off whenever that is off). On, the home puts the ask box
 * first and the apps big under it, folds Today to one line, and offers a Simple or
 * Full view; the top bar drops its clock for a Today button. Off, the apps-first
 * home is the one before it.
 *
 * Read once per page load, like `useAppsFirstExperience`.
 */
export default function useHomeV3(): boolean {
  const auth = useOptionalAuth();
  const appsFirst = useAppsFirstExperience();
  return appsFirst && auth?.authConfig.home_v3 === true;
}
