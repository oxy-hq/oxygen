/**
 * What the promote checks' browser does with ONE request of a page: sign it,
 * pass it as the page sent it, or refuse it and never send it. Every page the
 * checks open (`loads`, `smoke`, `changes`, `production_smoke`) is judged
 * here, by `loads-target.ts` `treat`.
 *
 * NO REQUEST IS "CONTINUED". Playwright follows a redirect of a continued
 * request without calling the handler again, re-applying the headers it was
 * continued with on every hop, wherever it leads. So every request that is
 * sent, signed or passed, is sent from here instead, once, with redirects
 * off, and its answer is handed to the browser whole. Two things follow:
 *
 * - A REDIRECT NEVER CARRIES THE CREDENTIAL. A `3xx` goes back as a `3xx`; the
 *   browser follows it as its own request, with its own headers and none of
 *   the checker's. The next hop is never signed, even one that lands under
 *   the app's base again (the platform answers no such redirect there).
 * - THE NEXT HOP IS JUDGED BEFORE THE BROWSER SEES IT (`treatHop`): a hop to
 *   another origin, to a private address, or one that would repeat a write
 *   is refused here, as the request itself would have been, and the redirect
 *   is never handed over.
 *
 * The browser asks no one about a hop it follows, nor about the hops after
 * it. Those are reads (a hop is handed over only when it is one), and they
 * can find no host but the app's own (`browser-confine.ts`).
 */

import { type Observed, pushOffOrigin, pushRefused } from "./loads-score.js";
import { isReadyBeacon, type LoadsTarget, type Refusal, treat, treatHop } from "./loads-target.js";
import type { PwRoute } from "./playwright.js";

/** Judge one request: sign it, pass it, or refuse it, and note the ready beacon. */
export async function answerRoute(
  route: PwRoute,
  target: LoadsTarget,
  observed: Observed,
  onReady: () => void
): Promise<void> {
  const request = route.request();
  if (isReadyBeacon(request.url(), request.postData())) onReady();
  const treatment = treat(target, request.method(), request.url());
  if (treatment.kind === "refuse") return refuse(route, treatment, observed);
  // Nothing is sent for it: there is no redirect to follow, and nowhere for it to go.
  if (treatment.kind === "local") return route.continue();
  const signature = treatment.kind === "sign" ? treatment.headers : {};
  return sendFromHere(route, signature, target, observed);
}

/** Record a request the check did not send as what it was: a write, or a request to another origin. */
export function recordRefusal(observed: Observed, refusal: Refusal): void {
  if (refusal.why === "off-origin") pushOffOrigin(observed, refusal.what);
  else pushRefused(observed, refusal.what);
}

/** Never send the request, and record it. */
function refuse(route: PwRoute, refusal: Refusal, observed: Observed): Promise<void> {
  recordRefusal(observed, refusal);
  return route.abort("blockedbyclient");
}

/**
 * Send a request from here, once, with `signature` over the page's own
 * headers (none, for a request that is passed), and hand the browser its
 * answer. A redirect is the answer: it is not followed from here, and it is
 * handed over only when the check would let its next hop go (see the header
 * of this file).
 */
async function sendFromHere(
  route: PwRoute,
  signature: Record<string, string>,
  target: LoadsTarget,
  observed: Observed
): Promise<void> {
  const request = route.request();
  let response: Awaited<ReturnType<PwRoute["fetch"]>>;
  try {
    response = await route.fetch({
      headers: { ...request.headers(), ...signature },
      maxRedirects: 0,
      timeout: target.waitMs
    });
  } catch (cause) {
    // It did not answer: for the page the request failed too, and for the same reason.
    return route.abort(abortCodeOf(cause));
  }
  const hop = redirectOf(request.method(), request.url(), response);
  const next = hop && treatHop(target, hop.method, hop.url);
  if (next?.kind === "refuse") return refuse(route, next, observed);
  return route.fulfill({ response });
}

/** Chromium's name for why a request sent from here got no answer. */
function abortCodeOf(cause: unknown): string {
  const text = cause instanceof Error ? cause.message : String(cause);
  if (/ECONNREFUSED/.test(text)) return "connectionrefused";
  if (/ECONNRESET|socket hang up/i.test(text)) return "connectionreset";
  if (/ENOTFOUND|EAI_AGAIN/.test(text)) return "namenotresolved";
  if (/ETIMEDOUT|timed out|timeout/i.test(text)) return "timedout";
  return "failed";
}

/** Where a redirect answer sends the request next, and with which method; `undefined` when it is none. */
function redirectOf(
  method: string,
  url: string,
  response: Awaited<ReturnType<PwRoute["fetch"]>>
): { method: string; url: string } | undefined {
  const status = response.status();
  if (![301, 302, 303, 307, 308].includes(status)) return undefined;
  const location = Object.entries(response.headers()).find(
    ([name]) => name.toLowerCase() === "location"
  )?.[1];
  if (!location) return undefined;
  let next: string;
  try {
    next = new URL(location, url).href;
  } catch {
    return undefined;
  }
  // 307 and 308 repeat the request as it was; the others turn a POST into a GET.
  const repeats = status === 307 || status === 308 || method === "GET" || method === "HEAD";
  return { method: repeats ? method : "GET", url: next };
}
