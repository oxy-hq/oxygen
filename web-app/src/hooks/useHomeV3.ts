import { useOptionalAuth } from "@/contexts/AuthContext";

/**
 * Whether home page v3 is on: the server's `home_v3` feature flag. On, the home
 * puts the ask box first and the apps big under it, folds Today to one line, and
 * offers a Simple or Full view; the top bar drops its clock for a Today button.
 * Off, the home is the one before it.
 *
 * Read once per page load.
 */
export default function useHomeV3(): boolean {
  return useOptionalAuth()?.authConfig.home_v3 === true;
}
