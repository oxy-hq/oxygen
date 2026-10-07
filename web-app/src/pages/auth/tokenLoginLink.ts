/** What a `/token-login#…` link carries, read out of its fragment. */
export interface TokenLoginLink {
  /** The one-time ticket to redeem; null when the link has none. */
  ticket: string | null;
  /** Same-origin path to land on, e.g. `/ide`. */
  next: string | null;
  /** Cross-origin destination; validated server-side before it is followed. */
  returnTo?: string;
}

/**
 * Parse a link's fragment (`location.hash`, leading `#` included). The
 * parameters ride in the fragment rather than the query string because a
 * fragment is never sent to a server: the ticket stays out of access logs and
 * out of any `Referer`.
 */
export const readTokenLoginLink = (hash: string): TokenLoginLink => {
  const params = new URLSearchParams(hash.slice(1));
  return {
    ticket: params.get("ticket") || null,
    next: params.get("next"),
    returnTo: params.get("return_to") ?? undefined
  };
};

/**
 * Drop the fragment from the address bar, keeping path and query, without
 * adding a history entry or a navigation. `history.state` is passed back
 * untouched — the router keeps its own bookkeeping there.
 */
export const stripFragment = (): void => {
  const { pathname, search } = window.location;
  window.history.replaceState(window.history.state, "", `${pathname}${search}`);
};
