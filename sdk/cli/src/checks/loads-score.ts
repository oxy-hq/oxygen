/**
 * What one page load showed, and the `loads` verdict over both viewports.
 *
 * A viewport passes when the document loaded as the app, reported the
 * environment and build being checked, sent `oxy-app-ready` and threw no
 * uncaught error. The first failure found is the one named. Failed requests
 * and the requests the check refused to send (a write, or anything to
 * another origin) are recorded as warnings beside it: the check reads, it
 * does not judge the app's data. A page that needed what was refused fails by
 * the rule above, and its failure names what was not sent.
 *
 * Nothing here holds a response body or any page content: a URL, a status, an
 * error's first line, and the two fields of `window.__OXY_APP__` are all a
 * load leaves behind.
 */

import type { LoadsTarget, Viewport } from "./loads-target.js";
import { cap, type Finding, finding, MAX_MESSAGE_CHARS, type Outcome, outcome } from "./outcome.js";

/** The most page errors, failed requests and refused requests of each kind kept per viewport. */
const MAX_KEPT = 20;

/** A request that failed or answered an error status. */
export interface FailedRequest {
  url: string;
  status: number | null;
  error: string | null;
}

/** What one page load showed, before it is scored. */
export interface Observed {
  /** Why the document itself failed: a navigation error, an error status, a redirect off the app. */
  navigationError?: string;
  /** When `oxy-app-ready` arrived, from the navigation; `null` when it never did. */
  readyMs: number | null;
  pageErrors: string[];
  failedRequests: FailedRequest[];
  /** `window.__OXY_APP__`'s environment and build, when the page has it. */
  app?: { environment: string | null; buildId: string | null };
  /** Same-origin calls outside the base that the server refused (401/403). */
  denied: string[];
  /** Requests the check refused to send (`"<METHOD> <path>"`): every write the page attempted. */
  refused: string[];
  /**
   * Requests to another origin (`"<METHOD> <url> (<why>)"`), none of which was
   * sent: a checked page reaches the app's own origin and nothing else.
   */
  offOrigin: string[];
}

export function newObserved(): Observed {
  return {
    readyMs: null,
    pageErrors: [],
    failedRequests: [],
    denied: [],
    refused: [],
    offOrigin: []
  };
}

export function pushPageError(observed: Observed, message: string): void {
  if (observed.pageErrors.length < MAX_KEPT) {
    observed.pageErrors.push(cap(message, MAX_MESSAGE_CHARS));
  }
}

/** One entry per distinct request. */
function pushOnce(list: string[], what: string): void {
  const kept = cap(what, MAX_MESSAGE_CHARS);
  if (list.length < MAX_KEPT && !list.includes(kept)) list.push(kept);
}

/** A write the check did not send. */
export function pushRefused(observed: Observed, what: string): void {
  pushOnce(observed.refused, what);
}

/** A request to another origin the check did not send. */
export function pushOffOrigin(observed: Observed, what: string): void {
  pushOnce(observed.offOrigin, what);
}

export function pushFailed(observed: Observed, request: FailedRequest): void {
  if (observed.failedRequests.length < MAX_KEPT) {
    observed.failedRequests.push({ ...request, url: cap(request.url, MAX_MESSAGE_CHARS) });
  }
}

/** What is wrong with one page load, by what kind of thing it is. */
export interface Problem {
  /**
   * `navigation` the document itself failed; `environment` and `build` it was
   * served by another one than the check holds it to; `not_ready` it never
   * sent `oxy-app-ready`; `page_error` it threw; `no_app` it carries no
   * `window.__OXY_APP__`.
   */
  kind: "navigation" | "environment" | "build" | "not_ready" | "page_error" | "no_app";
  why: string;
}

/** The failures of a page that did not come up as the app: the ones a request it was refused can cause. */
const MOUNTING: readonly Problem["kind"][] = ["not_ready", "page_error", "no_app"];

/** The first thing wrong with one page load, or `undefined` when nothing is. */
export function problemOf(target: LoadsTarget, observed: Observed): Problem | undefined {
  // A call the app repeats in a row is one cause.
  const denied = observed.denied.filter((call, i, all) => i === 0 || all[i - 1] !== call);
  // A failure the withheld credential may have caused names it, rather than
  // leaving the app to take the blame for a call the check refused to sign.
  const cause =
    denied.length === 0
      ? ""
      : `; the page's ${denied.slice(0, 3).join(", ")} — the check sends no credential outside the app's base path (${target.basePath}), so the app's own /api calls run as an anonymous visitor's`;
  // And so does one a refused request may have caused: an app that will not come
  // up without a script of another origin did not crash by itself.
  const unsent =
    observed.offOrigin.length === 0
      ? ""
      : `; not sent: ${observed.offOrigin.slice(0, 3).join(", ")} — a checked page reaches only the app's own origin (${target.origin})`;
  const fail = (kind: Problem["kind"], why: string): Problem => ({
    kind,
    why: `${why}${cause}${MOUNTING.includes(kind) ? unsent : ""}`
  });
  if (observed.navigationError) return fail("navigation", observed.navigationError);
  const served = observed.app?.environment;
  if (typeof served === "string" && served !== target.environment) {
    return fail(
      "environment",
      `the page was served by ${served}, not ${target.environment}: the environment header did not reach the document`
    );
  }
  if (observed.app && observed.app.buildId !== target.expectedBuild) {
    return fail(
      "build",
      `the page was served build ${observed.app.buildId ?? "(none)"}, but ${target.environment} serves ${target.expectedBuild}`
    );
  }
  if (observed.readyMs === null) {
    return fail(
      "not_ready",
      `the app never sent oxy-app-ready within ${Math.floor(target.waitMs / 1000)}s — a blank page, a crash on boot, or a sign-in page`
    );
  }
  const first = observed.pageErrors[0];
  if (first !== undefined) return fail("page_error", `uncaught page error: ${first}`);
  if (!observed.app) return fail("no_app", "the page carries no window.__OXY_APP__");
  return undefined;
}

/** One viewport's verdict: why it failed, or `undefined` when it passed. */
export function verdict(target: LoadsTarget, observed: Observed): string | undefined {
  return problemOf(target, observed)?.why;
}

/**
 * One viewport's load, as a check's detail lists it. `keep` is the most of
 * each list kept, for a check that lists many loads in one detail.
 */
export function viewportDetail(
  viewport: Viewport,
  observed: Observed,
  keep: number = MAX_KEPT
): Record<string, unknown> {
  return {
    name: viewport.name,
    width: viewport.width,
    height: viewport.height,
    ready: observed.readyMs !== null,
    ready_ms: observed.readyMs,
    environment: observed.app?.environment ?? null,
    build_id: observed.app?.buildId ?? null,
    page_errors: observed.pageErrors.slice(0, keep),
    failed_requests: observed.failedRequests.slice(0, keep),
    refused_writes: observed.refused.slice(0, keep),
    refused_off_origin: observed.offOrigin.slice(0, keep)
  };
}

/** What a failed request did, in a few words. */
function failedHow(request: FailedRequest): string {
  if (request.status !== null) return `answered ${request.status}`;
  return request.error ?? "failed";
}

/**
 * What a load showed that is no failure by itself: each failed request, each
 * write the check refused to send, and each request to another origin, none
 * of which it sends. `where` names the load (`desk`).
 */
export function warningsOf(where: string, observed: Observed): Finding[] {
  const warnings: Finding[] = [];
  for (const request of observed.failedRequests) {
    warnings.push(
      finding("warning", `${where}: a request ${failedHow(request)}`, { at: request.url })
    );
  }
  for (const what of observed.refused) {
    warnings.push(
      finding("warning", `${where}: not sent: ${what} (the checker reads, and sends no write)`, {
        rule: "write-refused"
      })
    );
  }
  for (const what of observed.offOrigin) {
    warnings.push(
      finding(
        "warning",
        `${where}: not sent: ${what} — a checked page reaches only the app's own origin`,
        { rule: "off-origin-refused" }
      )
    );
  }
  return warnings;
}

/** The check's outcome over every viewport's load. */
export function scoreLoads(
  target: LoadsTarget,
  loads: readonly (readonly [Viewport, Observed])[],
  identity: string
): Outcome {
  const findings: Finding[] = [];
  const failures: string[] = [];
  const ready: string[] = [];
  for (const [viewport, observed] of loads) {
    const why = verdict(target, observed);
    if (why === undefined) {
      ready.push(`${((observed.readyMs ?? 0) / 1000).toFixed(1)}s ${viewport.name}`);
    } else {
      findings.push(finding("error", `${viewport.name}: ${why}`));
      failures.push(`${viewport.name}: ${why}`);
    }
    findings.push(...warningsOf(viewport.name, observed));
  }
  const extra = {
    url: target.url,
    identity,
    viewports: loads.map(([viewport, observed]) => viewportDetail(viewport, observed))
  };
  const first = failures[0];
  return first === undefined
    ? outcome("passed", `ready in ${ready.join(", ")}`, findings, extra)
    : outcome("failed", first, findings, extra);
}
