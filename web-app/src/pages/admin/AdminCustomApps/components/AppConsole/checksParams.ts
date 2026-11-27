import type { BuildChecksEnvironment } from "@/types/appBuildChecks";

/**
 * Which checks the Releases page's Checks panel shows, as the address says it:
 * `?checks=<build>` names a build that is not staging's, and `?checks_env=production`
 * the attempt run on production after a move (staging's when absent). Kept in one
 * place so a link that opens the panel and the panel that reads it cannot disagree.
 */
export const CHECKS_PARAM = "checks";
export const CHECKS_ENV_PARAM = "checks_env";

/** What a link adds to open the panel on `build` (its `build_id` or row id), and on
 *  its production attempt when asked. */
export function checksLink(
  build: string,
  environment: BuildChecksEnvironment = "staging"
): Record<string, string> {
  return environment === "production"
    ? { [CHECKS_PARAM]: build, [CHECKS_ENV_PARAM]: "production" }
    : { [CHECKS_PARAM]: build };
}

/** The attempt the address asks for. Anything but `production` is staging. */
export function checksEnvironment(params: URLSearchParams): BuildChecksEnvironment {
  return params.get(CHECKS_ENV_PARAM) === "production" ? "production" : "staging";
}

/** The address with the panel on another build (`null`: staging's). It opens on that
 *  build's staging attempt, whichever attempt was showing. */
export function withChecksBuild(prev: URLSearchParams, build: string | null): URLSearchParams {
  const next = new URLSearchParams(prev);
  if (build) next.set(CHECKS_PARAM, build);
  else next.delete(CHECKS_PARAM);
  next.delete(CHECKS_ENV_PARAM);
  return next;
}

/** The address with the panel on the other attempt of the build it shows. */
export function withChecksEnvironment(
  prev: URLSearchParams,
  environment: BuildChecksEnvironment
): URLSearchParams {
  const next = new URLSearchParams(prev);
  if (environment === "production") next.set(CHECKS_ENV_PARAM, environment);
  else next.delete(CHECKS_ENV_PARAM);
  return next;
}
