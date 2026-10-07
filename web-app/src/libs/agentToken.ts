import type { AgentTokenLimits, Token, TokenOptions } from "@/types/apiToken";

/**
 * An agent token, as the two places that show one need it: the `/cli-auth` approval that
 * `oxyc tokens create --agent` opens, and the lists a minted one appears in.
 *
 * It is a personal token (`oxy_pat_…`, `kind: "personal"`) and not a kind of its own: what sets
 * it apart is where it came from. It reaches everything its approver does, lives hours, is
 * never stored as a login, and is fixed once minted. That is far more than a sandbox agent
 * token, which reaches the sandboxes of the apps it names and nothing else.
 */

/** The `source` the server stamps on a token minted through the agent approval. */
export const AGENT_TOKEN_SOURCE = "oxyc_agent";

/** What a list calls one, where a personal token is "Personal". */
export const AGENT_TOKEN_LABEL = "Agent";

/** Read off `source`: an older server sends none, and the token is then an ordinary personal one. */
export const isAgentToken = (token: Partial<Pick<Token, "kind" | "source">>): boolean =>
  token.kind === "personal" && token.source === AGENT_TOKEN_SOURCE;

/**
 * The server's limits, or `undefined` from a server that predates agent tokens. There is no
 * fallback on purpose: a server that names no limits can't mint one, so the approval refuses.
 */
export const agentLimits = (
  options: Pick<TokenOptions, "agent"> | undefined
): AgentTokenLimits | undefined => options?.agent;

/** What the approver holds beyond their memberships, in the order it is named. */
export type AgentStanding = "platform" | "partner";

export const heldStanding = (
  options: Pick<TokenOptions, "can_platform" | "can_partner">
): AgentStanding[] => [
  ...(options.can_platform ? (["platform"] as const) : []),
  ...(options.can_partner ? (["partner"] as const) : [])
];

const STANDING_WORDS: Record<AgentStanding, string> = { platform: "staff", partner: "partner" };

const STANDING_ADDS: Record<AgentStanding, string> = {
  platform: "every organization on this deployment",
  partner: "your client organizations"
};

/** "staff", "partner" or "staff and partner". */
export const standingWords = (held: readonly AgentStanding[]): string =>
  held.map((each) => STANDING_WORDS[each]).join(" and ");

/** What including it adds to the token's reach, as one sentence. */
export const standingAdds = (held: readonly AgentStanding[]): string =>
  `Adds ${held.map((each) => STANDING_ADDS[each]).join(", and ")}.`;

/** What a holder may and may not do, one act each, worded to follow "Can" and "Cannot". */
export const AGENT_TOKEN_POWERS: { can: string[]; cannot: string[] } = {
  can: ["Do what you can do through the API", "Open a browser session as itself"],
  cannot: ["Create, extend or revoke tokens", "Be extended", "Last past the time shown"]
};

/** The row title that says what one is, where a list has room for a sentence. */
export const AGENT_TOKEN_HINT =
  "For an AI agent acting as you. It can do what you can do through the API, and it can't be extended.";
