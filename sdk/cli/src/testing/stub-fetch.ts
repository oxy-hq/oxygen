/**
 * `globalThis.fetch`, replaced by a route table. Test support only — nothing
 * in the shipped bundle imports this.
 *
 * A route table rather than a loopback server where the thing being pinned is
 * the WIRE SHAPE of a handful of fixed routes: which path, which header, which
 * body, in which order. The cases that need a real socket (the login loopback,
 * the binary end to end) stand a server up instead.
 */

import { vi } from "vitest";

export interface Call {
  method: string;
  url: string;
  /** The URL with `base` removed — what the route table is keyed on. */
  path: string;
  /** Lower-cased names, as `Headers` reports them. */
  headers: Record<string, string>;
  body?: string;
}

export interface Reply {
  status: number;
  body?: unknown;
  /** Response headers beyond `content-type`, e.g. `retry-after` on a 429. */
  headers?: Record<string, string>;
}

/** Keyed `METHOD path`, the path including its query string. */
export type Routes = Record<string, (call: Call) => Reply>;

/**
 * Install the stub and return the list every call is appended to.
 *
 * An unlisted route answers 404 — which is also exactly what a deployment
 * that predates a route answers, so "the old server" needs no stub at all.
 * Undo with `vi.unstubAllGlobals()`.
 */
export function stubFetch(base: string, routes: Routes): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = String(input);
      const headers: Record<string, string> = {};
      new Headers(init?.headers).forEach((value, name) => {
        headers[name] = value;
      });
      const call: Call = {
        method: (init?.method ?? "GET").toUpperCase(),
        url,
        path: url.replace(base, ""),
        headers,
        body: typeof init?.body === "string" ? init.body : undefined
      };
      calls.push(call);
      const route = routes[`${call.method} ${call.path}`];
      if (!route) {
        return new Response(JSON.stringify({ error: `no stub for ${call.method} ${call.path}` }), {
          status: 404
        });
      }
      const { status, body, headers: extra } = route(call);
      // A 204 may not carry a body — `Response` throws if it is given one.
      if (status === 204) return new Response(null, { status, headers: extra });
      return new Response(JSON.stringify(body ?? {}), {
        status,
        headers: { "content-type": "application/json", ...extra }
      });
    })
  );
  return calls;
}

/**
 * A service account's id: what `--service-account` / `OXY_SERVICE_ACCOUNT` carry.
 * The exchange takes nothing else — never `<org-slug>/<name>`, which is only
 * what the deployment answers with.
 */
export const SERVICE_ACCOUNT_ID = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

/** The two variables GitHub sets in a job granted `id-token: write`. */
export function inGithubActions(base: string): void {
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", `${base}/__gh/token?api-version=2.0`);
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "gh-request-token");
}

/**
 * GitHub minting an id token, one per audience.
 *
 * The general exchange's audience is `oxy:<host>` OF THE TARGET — every test
 * here stubs `https://oxy.test`, so `oxy:oxy.test`. There is deliberately no
 * route for plain `oxy`: oxyc never asks for it, and a test that did would get
 * the stub's 404.
 */
export const GITHUB_ID_TOKEN_ROUTES: Routes = {
  "GET /__gh/token?api-version=2.0&audience=oxy%3Aoxy.test": () => ({
    status: 200,
    body: { value: "gh-jwt-oxy" }
  }),
  "GET /__gh/token?api-version=2.0&audience=oxy-publish": () => ({
    status: 200,
    body: { value: "gh-jwt-publish" }
  })
};
