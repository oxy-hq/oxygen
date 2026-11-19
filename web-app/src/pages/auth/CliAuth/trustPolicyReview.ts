import type { CliTrustPolicyMintRequest } from "@/types/apiToken";
import type { TrustPolicyAsk } from "./cliAuthRequest";

/**
 * The command that opened this request, for the org and app the server resolved: whoever runs it
 * names the app, and may be a member asking an admin, so the page shows which app.
 */
export const trustPolicyCommand = (org: string, app: string): string =>
  `oxyc init-ci --app ${org}/${app}`;

/**
 * `oxyc init-ci`'s request for a trust policy, as the `mint` the server is asked about and then
 * asked to approve. Whether it can be approved is the server's to say, and the page shows its
 * answer (`useTrustPolicyPreview`). The repository's GitHub ids are never part of it: a run is
 * matched on them and this page shows only the name, so the server reads them from GitHub.
 */
export const trustPolicyMint = (ask: TrustPolicyAsk): CliTrustPolicyMintRequest => ({
  kind: "trust_policy",
  org: ask.org,
  account: ask.account,
  create_account: ask.create,
  app: ask.app,
  repository: ask.repository,
  workflow_path: ask.workflow,
  ...(ask.environment !== null ? { environment: ask.environment } : {})
});
