import { tokenState } from "@/pages/admin/components/TokenList/tokenState";
import type { Token } from "@/types/apiToken";

/** Which standing a row must carry to be shown. A token with both is under each. */
export type StandingFilter = "all" | "staff" | "partner";

export interface TokenFilter {
  standing: StandingFilter;
  /** Leave out the tokens that have expired or been revoked. */
  hideEnded: boolean;
}

/** Everything the server sent, ended tokens included: the page is also the record. */
export const NO_FILTER: TokenFilter = { standing: "all", hideEnded: false };

type Filtered = Pick<Token, "platform" | "partner" | "status" | "expires_at">;

const carries = (token: Filtered, standing: StandingFilter): boolean => {
  if (standing === "staff") return token.platform;
  if (standing === "partner") return token.partner;
  return true;
};

const ended = (token: Filtered): boolean => tokenState(token) !== "active";

/** The rows a filter leaves, in the order they came. */
export const filterTokens = <T extends Filtered>(tokens: T[], filter: TokenFilter): T[] =>
  tokens.filter((token) => carries(token, filter.standing) && !(filter.hideEnded && ended(token)));

export interface FilterCounts {
  /** How many rows each standing choice shows, with the ended ones hidden or not as set now. */
  standing: Record<StandingFilter, number>;
  /** How many ended tokens the chosen standing has: what hiding them takes away. */
  ended: number;
}

/** What each control would show, so a choice that selects everything or nothing says so first. */
export const filterCounts = (tokens: Filtered[], filter: TokenFilter): FilterCounts => {
  const shown = (standing: StandingFilter): number =>
    filterTokens(tokens, { standing, hideEnded: filter.hideEnded }).length;
  return {
    standing: { all: shown("all"), staff: shown("staff"), partner: shown("partner") },
    ended: tokens.filter((token) => carries(token, filter.standing) && ended(token)).length
  };
};

const STANDING_WORDS: Record<Exclude<StandingFilter, "all">, string> = {
  staff: "staff",
  partner: "partner"
};

/**
 * What the table says when a filter leaves no row. The list itself is never empty here: the page
 * shows that case before any filter is offered.
 */
export const noMatchMessage = (filter: TokenFilter): string => {
  if (filter.standing === "all") return "Every token listed has expired or been revoked.";
  const standing = STANDING_WORDS[filter.standing];
  return filter.hideEnded
    ? `No token with ${standing} standing works right now.`
    : `No token carries ${standing} standing.`;
};
