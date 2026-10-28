import { Navigate } from "react-router-dom";

/**
 * The catch-all of a workspace's routes: an address nothing matches goes to the
 * workspace home.
 *
 * `to='.'` looked like "stay here" but is not "go home": inside a splat route it
 * resolves to the current address, so the redirect landed where it started and
 * the page stayed blank. The old Data App address `…/apps` is the case that showed
 * it. `..` leaves the splat route for the workspace it is mounted under.
 */
export default function UnknownWorkspacePath() {
  return <Navigate to='..' replace />;
}
