/**
 * The agent token (`oxyc tokens create --agent`), where a command has to know
 * it from the other things an `oxy_pat_` can be.
 *
 * IT HAS NO PREFIX OF ITS OWN. It is a personal access token — the same
 * `oxy_pat_` an `oxyc login` stores — that reaches everything its approver
 * does for a few hours, and what marks it is where it came from: the
 * deployment describes it with `source: "oxyc_agent"`. So two things can tell:
 *
 *   - a LIVE token's own description (`GET /api/auth/token`), which `whoami`
 *     reads;
 *   - for a token the deployment no longer accepts, nothing can. A dead token
 *     describes nothing. What is still known is where the credential was read
 *     from, and that is enough for the one thing that matters then.
 *
 * THE RULE FOR A REFUSED CREDENTIAL FROM THE TOKEN VARIABLE: never "log in".
 * The variable wins over the login cache, so a login cannot replace what is in
 * it — and for an agent the advice is worse than useless. It was handed its own
 * credential precisely so that it would not run as the person who approved it;
 * an agent told to log in opens that person's browser and comes back holding
 * their ninety-day login. It stops and reports instead.
 */

import { AUTH_HINT } from "../api/request.js";
import { STOP_AND_REPORT } from "../apps/sandbox-token.js";
import { type CliError, ExitCode } from "../util/errors.js";
import { credentialShape } from "./token-kind.js";

/** The `source` a deployment stamps on a token minted through the agent approval. */
export const AGENT_TOKEN_SOURCE = "oxyc_agent";

/** Whether a token's description is an agent token's. A personal token from anywhere else is not. */
export function isAgentTokenRow(token: { kind?: unknown; source?: unknown }): boolean {
  return token.kind === "personal" && token.source === AGENT_TOKEN_SOURCE;
}

/** The stored bearer this process resolved from the token variable, if it did. */
let fromVariable: { variable: string; token: string } | undefined;

/**
 * Remember that the bearer came from the token variable. Called where the
 * credential is resolved, and read where a failure is reported: the two are
 * far apart, and every `errorForResponse` between them knows only a response.
 */
export function noteVariableCredential(variable: string, token: string): void {
  fromVariable = { variable, token };
}

/** Forget it. For tests. */
export function forgetVariableCredential(): void {
  fromVariable = undefined;
}

/** What to do about a credential in `variable` that the deployment refused. */
export function variableCredentialSteps(variable: string): string {
  return (
    `an agent: ${STOP_AND_REPORT}\n` +
    `a person: put a token that works in ${variable}, or unset it to use your own \`oxyc login\``
  );
}

/**
 * The hint for a `401`/`403` when the bearer is a personal access token read
 * from the token variable, in place of the generic "try `oxyc login` again".
 * `undefined` for every other credential, whose own hint stands.
 */
function variableCredentialAuthHint(): string | undefined {
  if (!fromVariable || credentialShape(fromVariable.token) !== "personal") return undefined;
  const { variable } = fromVariable;
  return (
    `the token in ${variable} may have expired or been revoked, or it lacks the role — a login cannot fix that: ${variable} wins over the login cache\n` +
    `${variableCredentialSteps(variable)}\n` +
    "a staff token on a tenant surface needs `oxyc assume <org> --reason …`"
  );
}

/**
 * The hint a failure is shown with.
 *
 * ONE SWAP, and only of the generic sentence (`AUTH_HINT`, what
 * `errorForResponse` gives every `401`/`403`): under a personal access token
 * from the token variable it becomes the hint above. Every hint a command
 * wrote itself stands, and so does the generic one for a cached login, which a
 * login does fix.
 */
export function hintToShow(cause: CliError): string | undefined {
  if (cause.code === ExitCode.AUTH && cause.hint === AUTH_HINT) {
    return variableCredentialAuthHint() ?? cause.hint;
  }
  return cause.hint;
}
