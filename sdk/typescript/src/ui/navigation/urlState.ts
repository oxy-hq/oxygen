// Started from pokehouse-oxy:apps/watchlist/src/lib/urlState.ts (BEYOND-GALLERY row 7: the tested
// copy, and the one that handles the phone kit's sheet entries), which is the reference's
// `src/lib/urlState.ts` (oxy-hq/reference-app@3caad1f) plus `keepWrittenUrl`. Kept: the
// replaceState write that keeps the entry's state (the reference's `null` wiped a phone sheet's
// `oxyPhoneSheetDepth` and a router's own state), the "only write what differs from the
// default" convention, the two parsers. Changed: the popstate listener is installed by the first
// write rather than at import, so an app that never writes a param carries no listener.
//
// Persist a screen's selections in the URL query string, so a refresh, or a link pasted into a
// chat, lands on the same view instead of resetting to the defaults. replaceState: no history
// entries, Back behaves normally.
//
// **Only write what differs from the default.** A URL with no params means "whatever the app
// would show me today", which is what someone sharing a link almost always means. Writing the
// auto-picked month would pin it, so a link shared in August would still open on July in
// October, silently stale.

/** The URL the app last wrote or landed on: what its state says. */
let written: string | null = null;
let listening = false;

export function readUrlParam(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

/** Merge params into the current URL (null or empty deletes the param). */
export function writeUrlParams(patch: Record<string, string | null | undefined>): void {
  const url = new URL(window.location.href);
  for (const [k, v] of Object.entries(patch)) {
    if (v === null || v === undefined || v === "") url.searchParams.delete(k);
    else url.searchParams.set(k, v);
  }
  // The URL changes, the entry does not: a screen writing its defaults on mount must not wipe
  // what the entry knows (a phone sheet's depth, a router's own state).
  window.history.replaceState(window.history.state, "", url);
  written = url.href;
  if (!listening) {
    listening = true;
    window.addEventListener("popstate", keepWrittenUrl);
  }
}

/**
 * On Back or Forward. Same path: the screen stayed mounted and kept its selections (a phone
 * sheet's entry was crossed, written before a filter changed in it), so the entry gets the URL
 * the screen last wrote, keeping its state; a reload then shows what was on screen. Another
 * path: the screen changes and reads its selections from the entry's URL, which is right as it
 * is.
 */
export function keepWrittenUrl(): void {
  const here = window.location.href;
  if (
    written !== null &&
    here !== written &&
    new URL(written).pathname === window.location.pathname
  ) {
    window.history.replaceState(window.history.state, "", written);
  } else {
    written = here;
  }
}

/** A navigation of the app's own: the screen it lands on reads this URL. The router calls it. */
export function noteUrl(): void {
  written = window.location.href;
}

// The parsers are pure, because a URL is untrusted input: hand-edited, pasted, truncated by
// chat clients and carried across months. Anything unreadable is ABSENT, never an error.

/** `2026-07`, or undefined. Anything else is absent, not an error. */
export function parseMonthParam(raw: string | null): string | undefined {
  if (!raw) return undefined;
  return /^\d{4}-(0[1-9]|1[0-2])$/.test(raw) ? raw : undefined;
}

/** One of `allowed`, or `fallback`. */
export function parseOneOf<T extends string>(
  raw: string | null,
  allowed: readonly T[],
  fallback: T
): T {
  return raw != null && (allowed as readonly string[]).includes(raw) ? (raw as T) : fallback;
}
