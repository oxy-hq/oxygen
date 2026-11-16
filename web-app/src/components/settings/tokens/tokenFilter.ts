import { daysLeft, tokenState } from "@/libs/tokenState";
import type { Grant, Token, TokenOwner } from "@/types/apiToken";

/**
 * Which tokens a list shows. `expiring` is a slice of `active`, not a fourth
 * state: it answers the one question a long list is usually opened for — what
 * is about to stop working — without making anyone read every row's date.
 */
export type TokenFilter = "all" | "active" | "expiring" | "ended";

/** How close to lapsing counts as expiring. Matches the expiry notice. */
export const EXPIRING_DAYS = 7;

export const FILTER_LABELS: Record<TokenFilter, string> = {
  all: "All",
  active: "Working",
  expiring: "Expiring soon",
  ended: "Expired or revoked"
};

/**
 * What these helpers need of a row, so the caller's own shape decides the rest.
 * Both token lists this serves carry different columns — the account's rows
 * have `grants`, the workspace inventory has a ceiling instead — and neither
 * should have to be converted to the other to be searched.
 */
export interface Searchable extends Pick<Token, "status" | "expires_at" | "name"> {
  display_prefix?: string;
  last_four?: string;
  owner?: TokenOwner;
  source?: string;
  grants?: Grant[];
}

export const matchesFilter = (
  token: Pick<Searchable, "status" | "expires_at">,
  filter: TokenFilter,
  now = new Date()
): boolean => {
  const state = tokenState(token);
  switch (filter) {
    case "all":
      return true;
    case "active":
      return state === "active";
    case "expiring": {
      const left = daysLeft(token, now);
      return left !== null && left <= EXPIRING_DAYS;
    }
    case "ended":
      return state !== "active";
  }
};

/**
 * Everything the search box matches a token on: its name, the non-secret ends
 * of the secret itself, who owns it, where it came from, and the name of every
 * org, workspace and app it reaches.
 *
 * The last part is what makes the box worth having. A list of twenty tokens is
 * mostly `oxyc on <someone>'s-laptop`, and the question people actually arrive
 * with is "which of these can touch Platform Pulse?" — which is a search for
 * the app, not for the token.
 */
export const searchText = (token: Searchable): string =>
  [
    token.name,
    token.display_prefix ?? "",
    token.last_four ?? "",
    token.owner?.label ?? "",
    token.source ?? "",
    ...(token.grants ?? []).flatMap((grant) => [
      grant.org_name,
      grant.workspace_name ?? "",
      grant.app_name ?? "",
      grant.app_slug ?? ""
    ])
  ]
    .join(" ")
    .toLowerCase();

/** The tokens a filter and a query leave, in the order they came. */
export const filterTokens = <T extends Searchable>(
  tokens: T[],
  filter: TokenFilter,
  query: string,
  now = new Date()
): T[] => {
  const q = query.trim().toLowerCase();
  return tokens.filter(
    (token) => matchesFilter(token, filter, now) && (!q || searchText(token).includes(q))
  );
};

/** How many rows each filter would show — for the chips' labels. */
export const filterCounts = (
  tokens: Searchable[],
  query: string,
  now = new Date()
): Record<TokenFilter, number> => {
  const q = query.trim().toLowerCase();
  const matching = q ? tokens.filter((t) => searchText(t).includes(q)) : tokens;
  return {
    all: matching.length,
    active: matching.filter((t) => matchesFilter(t, "active", now)).length,
    expiring: matching.filter((t) => matchesFilter(t, "expiring", now)).length,
    ended: matching.filter((t) => matchesFilter(t, "ended", now)).length
  };
};

/** What the table says when a filter and a query leave no row. */
export const noMatchMessage = (filter: TokenFilter, query: string): string => {
  if (query.trim()) return "No token matches that. Try a shorter search, or clear the filter.";
  switch (filter) {
    case "active":
      return "Every token here has expired or been revoked.";
    case "expiring":
      return `No token expires in the next ${EXPIRING_DAYS} days.`;
    case "ended":
      return "Every token here still works.";
    case "all":
      return "No token matches.";
  }
};
