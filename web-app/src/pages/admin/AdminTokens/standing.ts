import { AGENT_TOKEN_SOURCE } from "@/libs/agentToken";
import type { Token } from "@/types/apiToken";

type Carrier = Pick<Token, "platform" | "partner">;

/**
 * The standing a token carries, as its row says it. The server lists no token with neither; one
 * that arrives anyway reads "None", not a standing it does not carry.
 */
export const standingLabel = (token: Carrier): string => {
  if (token.platform && token.partner) return "Staff and partner";
  if (token.platform) return "Staff";
  if (token.partner) return "Partner";
  return "None";
};

/** What that standing lets the token do, for the cell's hover. */
export const standingHint = (token: Carrier): string | undefined => {
  const staff = "Acts with its owner's Oxygen staff standing, such as in the admin console.";
  const partner = "Reaches its owner's client organizations as a partner.";
  if (token.platform && token.partner) return `${staff} ${partner}`;
  if (token.platform) return staff;
  return token.partner ? partner : undefined;
};

/** `oxyc` is what this list sends for `oxyc login`; a personal token's own record spells it out. */
const OXYC_LOGIN_SOURCES = new Set(["oxyc", "oxyc_login"]);

/** `oxyc login` made the token: it is the credential on its owner's machine. */
export const fromOxycLogin = (token: Pick<Token, "source">): boolean =>
  OXYC_LOGIN_SOURCES.has(token.source);

export interface MadeWith {
  label: string;
  /** Where to look for the secret, for the cell's hover. */
  hint: string | undefined;
}

/**
 * How the token was made, which is where its secret is likely to be: `oxyc login` keeps it in
 * the credentials file on its owner's machine, one made in Settings was copied out by hand, and
 * an agent's is held by the agent's process. Any other source is shown as the server spells it.
 */
export const madeWith = (token: Pick<Token, "source">): MadeWith => {
  if (fromOxycLogin(token)) {
    return {
      label: "oxyc login",
      hint: "Made by oxyc login, which keeps it on its owner's machine."
    };
  }
  if (token.source === "ui") {
    return { label: "Settings", hint: "Made in Settings, under Personal access tokens." };
  }
  if (token.source === AGENT_TOKEN_SOURCE) {
    return {
      label: "oxyc agent",
      hint: "Asked for by an AI agent with oxyc tokens create --agent and approved by its owner. It is held by the agent's process, not saved on the machine, and lasts hours."
    };
  }
  return { label: token.source || "Unknown", hint: undefined };
};
