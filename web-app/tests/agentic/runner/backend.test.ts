import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { applyPathPrefix } from "./backend";

// Regression coverage for a harness bug found while wiring `admin-airhouse-fleet`
// (the flow had never been invoked by any runner, so nothing had exercised this
// path before). Its setup command is
//
//   goto:/dev-login?email=e2e@oxy.tech&next=/admin/airhouse
//
// and `/dev-login` is on the TOP_LEVEL_SURFACES list precisely so the workspace
// prefix stays off it. But the membership test compared the WHOLE target —
// query string included — so `/dev-login?email=…` matched neither `/dev-login`
// nor `/dev-login/`, got prefixed, and the browser navigated to
// `/local/workspaces/<id>/dev-login?email=…`. Captured from the flow's own
// Playwright trace: that URL is the first request in `trace.network`.
//
// The failure mode is the expensive one this whole list exists to prevent: the
// SPA renders its fallback, the flow times out on its first locator after 30s,
// and it reads exactly like a broken admin page rather than a bad URL.

const KEY = "OXY_PATH_PREFIX";
const PREFIX = "/local/workspaces/70787bb2-e11b-5488-b2c3-02e60d5fc7d3";

describe("applyPathPrefix", () => {
  let saved: string | undefined;

  beforeEach(() => {
    saved = process.env[KEY];
    process.env[KEY] = PREFIX;
  });

  afterEach(() => {
    if (saved === undefined) delete process.env[KEY];
    else process.env[KEY] = saved;
  });

  it("prefixes a workspace-scoped surface", () => {
    expect(applyPathPrefix("/automations")).toBe(`${PREFIX}/automations`);
    expect(applyPathPrefix("/ide/observability/traces")).toBe(`${PREFIX}/ide/observability/traces`);
  });

  it("leaves a top-level surface alone", () => {
    expect(applyPathPrefix("/admin/workspace-health")).toBe("/admin/workspace-health");
    expect(applyPathPrefix("/partners")).toBe("/partners");
    expect(applyPathPrefix("/customer-apps/local/oxy-starter/")).toBe(
      "/customer-apps/local/oxy-starter/"
    );
  });

  // The bug. A query string must not change whether a path is top-level.
  it("leaves a top-level surface alone when it carries a query string", () => {
    expect(applyPathPrefix("/dev-login?email=e2e@oxy.tech&next=/admin/airhouse")).toBe(
      "/dev-login?email=e2e@oxy.tech&next=/admin/airhouse"
    );
    expect(applyPathPrefix("/admin/tenants?tab=partners")).toBe("/admin/tenants?tab=partners");
    expect(applyPathPrefix("/login#returnTo=/home")).toBe("/login#returnTo=/home");
    // `/token-login` carries everything in the fragment, never a query string.
    expect(applyPathPrefix("/token-login#ticket=abc&next=%2Fide")).toBe(
      "/token-login#ticket=abc&next=%2Fide"
    );
  });

  // …and a workspace-scoped path with a query string must still be prefixed,
  // so the fix cannot be "give up whenever there is a `?`".
  it("still prefixes a workspace-scoped surface that carries a query string", () => {
    expect(applyPathPrefix("/threads?filter=mine")).toBe(`${PREFIX}/threads?filter=mine`);
  });

  it("does not double-prefix an already-prefixed target, query string or not", () => {
    expect(applyPathPrefix(`${PREFIX}/threads`)).toBe(`${PREFIX}/threads`);
    expect(applyPathPrefix(`${PREFIX}/threads?filter=mine`)).toBe(`${PREFIX}/threads?filter=mine`);
  });

  it("leaves bare / and absolute URLs alone", () => {
    expect(applyPathPrefix("/")).toBe("/");
    expect(applyPathPrefix("http://localhost:3000/threads")).toBe("http://localhost:3000/threads");
  });

  it("is a no-op with no prefix configured", () => {
    delete process.env[KEY];
    expect(applyPathPrefix("/automations")).toBe("/automations");
  });
});
