/**
 * Warm a custom app's entry chunks before the user commits to opening it.
 *
 * **With `rel=prefetch` the document itself is not the win.** The shell is
 * served `private, no-cache`, so a prefetched copy has to be revalidated before
 * the navigation may reuse it — at best this saves a connection setup, not a
 * round trip. (A Speculation Rules prefetch is different; see below.)
 *
 * What pays either way is the `Link: rel=modulepreload` header the serve path
 * attaches to that response: the browser follows it and pulls the app's entry
 * chunks, which *are* `immutable` and therefore reusable without revalidation.
 * By the time the click lands, the expensive half is already in the HTTP cache.
 * On a repeat visit the app's service worker has them precached and the whole
 * thing is instant regardless; this is the *first* visit's version of it.
 *
 * ## Why `rel=prefetch` and not `fetch()`
 *
 * A `fetch()` of the app's HTML would look exactly like a page load to the
 * serve path: it would mint a tracking session cookie and record a view row for
 * an app the user only hovered over. `rel=prefetch` instead makes the browser
 * announce itself — `Sec-Purpose` on Chrome and Edge, `X-moz` on Firefox, and
 * `Purpose` from older engines and intermediaries. The serve path checks all
 * three (`is_speculative_request`) before recording anything, so a hover stays
 * a hover in the Activity tab.
 *
 * ## Speculation Rules where the browser has them
 *
 * Chrome and Edge get a Speculation Rules prefetch instead. Unlike
 * `rel=prefetch`, the navigation may use that response as it is, so the
 * document's round trip is saved as well as the chunks', which is most of what
 * makes opening an app from the home page feel immediate. It announces itself
 * the same way (`Sec-Purpose: prefetch`), so the serve path still records
 * nothing for the hover; and because the server then never sees the real open,
 * the app's own runtime reports it once the page is shown (`runtime.js`,
 * "an open the server could not see"). That report needs the page to know it
 * came from a prefetch (`deliveryType` on its navigation entry), so a browser
 * with Speculation Rules but without that gets `rel=prefetch` like any other:
 * its open revalidates, and the server counts it. A prefetch, not a prerender:
 * a prerender would run the app, its queries and its functions, for a hover.
 *
 * Idempotent per URL: browsers already de-duplicate prefetches, but the check
 * also keeps `<head>` from accumulating an element per hover for the life of
 * the page.
 */
export function prefetchApp(url: string): void {
  if (typeof document === "undefined" || !url) return;
  // `CSS.escape` because an app URL contains slashes and can contain other
  // selector metacharacters; a hand-built attribute selector would throw on
  // them and take the hover handler down with it.
  const escaped = typeof CSS !== "undefined" && CSS.escape ? CSS.escape(url) : null;
  if (escaped && document.querySelector(`[data-oxy-prefetch="${escaped}"]`)) return;

  if (supportsSpeculationRules()) {
    const rules = document.createElement("script");
    rules.type = "speculationrules";
    rules.textContent = JSON.stringify({ prefetch: [{ source: "list", urls: [url] }] });
    rules.dataset.oxyPrefetch = url;
    document.head.appendChild(rules);
    return;
  }

  const link = document.createElement("link");
  link.rel = "prefetch";
  link.href = url;
  // `as` is what tells the browser this is a navigation target rather than a
  // subresource, which is what makes it honour the response's own preload
  // hints instead of parking the bytes in the prefetch cache alone.
  link.setAttribute("as", "document");
  link.dataset.oxyPrefetch = url;
  document.head.appendChild(link);
}

/** Speculation Rules, in a browser whose opened page can tell it was one. */
function supportsSpeculationRules(): boolean {
  return (
    typeof HTMLScriptElement !== "undefined" &&
    typeof HTMLScriptElement.supports === "function" &&
    HTMLScriptElement.supports("speculationrules") &&
    typeof PerformanceNavigationTiming !== "undefined" &&
    "deliveryType" in PerformanceNavigationTiming.prototype
  );
}
