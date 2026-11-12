/**
 * What to say when the credential is `OXY_TOKEN=from-proxy` and the deployment
 * turned the request away.
 *
 * Nothing on this machine can be checked: the token is in the environment's
 * egress proxy (a Claude Code cloud environment's network secret), which adds
 * `Authorization` to requests for the hosts it lists and to no others. So the
 * causes are all on the proxy's side, and each is said, because "log in
 * again" — the usual remedy — changes nothing here.
 */

import { STOP_AND_REPORT } from "../apps/sandbox-token.js";

/** Why a request under the proxy sentinel carried no credential the deployment accepts. */
export const PROXY_CAUSES = [
  "no token is held on this machine: the environment's network secret adds it on the way out. It did not, or the deployment refused it:",
  "  - the secret's Allowed websites must cover this host, exactly or by a leading *. (`*.oxy.tech` does not cover `oxy.tech`);",
  "  - its header must be `Authorization` with the prefix `Bearer`;",
  "  - the token it holds may have expired or been revoked, or lack the role."
].join("\n");

/** The steps for a refused request under the proxy sentinel in `variable`. */
export function proxyCredentialSteps(variable: string): string {
  return [
    `${variable}=from-proxy — ${PROXY_CAUSES}`,
    `an agent: ${STOP_AND_REPORT}`,
    `a person: fix the network secret in the cloud environment's settings (to change one, delete it and add it again), or set ${variable} to a token of your own`
  ].join("\n");
}
