/**
 * `GET` and `DELETE /api/auth/token`, and the two renderers that sit on them.
 *
 * Both calls are footnotes to a command that has already done its work, so the
 * property pinned here is that NEITHER EVER THROWS — and that each answer an
 * older deployment gives (a bare 404, a 405) reads as "nothing to say", not as
 * a failure.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { stubFetch } from "../testing/stub-fetch.js";
import {
  describeExpiry,
  describeReach,
  type Grant,
  introspectToken,
  revokeCallingToken,
  type Token
} from "./token-api.js";

const TARGET = "https://oxy.test";

const grant = (over: Partial<Grant> = {}): Grant => ({
  id: "g-1",
  kind: "workspace",
  org_id: "org-1",
  org_name: "Acme",
  workspace_id: null,
  workspace_name: null,
  role_ceiling: "owner",
  app_id: null,
  app_name: null,
  revoked_at: null,
  ...over
});

const token = (over: Partial<Token> = {}): Token => ({
  id: "tok-1",
  name: "oxyc on laptop",
  kind: "personal",
  display_prefix: "oxy_pat_ab12",
  last_four: "9f3c",
  all_access: true,
  platform: false,
  partner: false,
  grants: [],
  expires_at: "2099-01-01T00:00:00Z",
  last_used_at: null,
  created_at: "2026-10-01T00:00:00Z",
  revoked_at: null,
  status: "active",
  source: "oxyc_login",
  owner: { type: "user", id: "u-1", label: "ada@acme.test" },
  blocked_orgs: [],
  ...over
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("introspectToken", () => {
  it("returns the calling token, sent as a bearer", async () => {
    const calls = stubFetch(TARGET, {
      "GET /api/auth/token": () => ({ status: 200, body: token() })
    });
    const found = await introspectToken(TARGET, "oxy_pat_secret");
    expect(found).toMatchObject({ kind: "token", token: { id: "tok-1", all_access: true } });
    expect(calls[0]?.headers.authorization).toBe("Bearer oxy_pat_secret");
  });

  it("fills the collections a lenient server omits, so no caller has to guard", async () => {
    const { grants: _g, blocked_orgs: _b, ...bare } = token();
    stubFetch(TARGET, { "GET /api/auth/token": () => ({ status: 200, body: bare }) });
    const found = await introspectToken(TARGET, "oxy_pat_secret");
    expect(found.kind === "token" && found.token.grants).toEqual([]);
    expect(found.kind === "token" && found.token.blocked_orgs).toEqual([]);
  });

  it("reads 404 `no_token` as a browser session, not as a missing route", async () => {
    stubFetch(TARGET, {
      "GET /api/auth/token": () => ({ status: 404, body: { error: "no token", code: "no_token" } })
    });
    expect(await introspectToken(TARGET, "session-jwt")).toEqual({ kind: "session" });
  });

  it("reads a bare 404 as a deployment that predates the route", async () => {
    stubFetch(TARGET, {});
    expect(await introspectToken(TARGET, "session-jwt")).toEqual({
      kind: "unavailable",
      status: 404
    });
  });

  it("does not throw when the network does", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("fetch failed");
      })
    );
    expect(await introspectToken(TARGET, "oxy_pat_secret")).toEqual({
      kind: "unavailable",
      status: 0
    });
  });
});

describe("revokeCallingToken", () => {
  const outcomeFor = async (status: number, body?: unknown) => {
    stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status, body }) });
    return revokeCallingToken(TARGET, "oxy_pat_secret");
  };

  it("204 is revoked, and the token it revokes is the one that made the call", async () => {
    const calls = stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 204 }) });
    expect(await revokeCallingToken(TARGET, "oxy_pat_secret")).toBe("revoked");
    expect(calls).toHaveLength(1);
    expect(calls[0]?.headers.authorization).toBe("Bearer oxy_pat_secret");
  });

  it("409 is a legacy key — only its owner can end one", async () => {
    expect(await outcomeFor(409, { code: "legacy_immutable" })).toBe("legacy");
  });

  it("404 and 405 are a session, or a deployment with no such route", async () => {
    expect(await outcomeFor(404)).toBe("unsupported");
    expect(await outcomeFor(405)).toBe("unsupported");
  });

  it("401 and 403 mean the server already refuses it, which was the goal", async () => {
    expect(await outcomeFor(401)).toBe("already_invalid");
    expect(await outcomeFor(403)).toBe("already_invalid");
  });

  it("a 5xx and a dead network are both `failed`, never a throw", async () => {
    expect(await outcomeFor(503)).toBe("failed");
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("fetch failed");
      })
    );
    expect(await revokeCallingToken(TARGET, "oxy_pat_secret")).toBe("failed");
  });
});

describe("describeExpiry", () => {
  const now = Date.parse("2026-10-01T00:00:00Z");

  it("says `never` for no expiry, which is what null means on the wire", () => {
    expect(describeExpiry(null, now)).toBe("never");
    expect(describeExpiry(undefined, now)).toBe("never");
  });

  it("counts a fifteen-minute CI token in minutes and a login token in days", () => {
    expect(describeExpiry("2026-10-01T00:15:00Z", now)).toBe("2026-10-01T00:15:00Z (in 15 min)");
    expect(describeExpiry("2026-10-01T20:00:00Z", now)).toBe("2026-10-01T20:00:00Z (in 20 h)");
    expect(describeExpiry("2026-12-30T00:00:00Z", now)).toBe("2026-12-30 (in 90 days)");
  });

  it("says a past date expired, and passes an unparseable one through", () => {
    expect(describeExpiry("2026-01-02T00:00:00Z", now)).toBe("expired 2026-01-02");
    expect(describeExpiry("soon", now)).toBe("soon");
  });
});

describe("describeReach", () => {
  it("stops at `all access` rather than listing orgs that would go stale", () => {
    expect(describeReach(token({ grants: [grant()] }))).toEqual(["all access"]);
    expect(describeReach(token({ platform: true, partner: true }))).toEqual([
      "all access (+ platform, partner standing)"
    ]);
  });

  it("lists a narrowed token's live grants, with the ceiling each carries", () => {
    const lines = describeReach(
      token({
        all_access: false,
        grants: [
          grant(),
          grant({
            id: "g-2",
            workspace_id: "ws-1",
            workspace_name: "Sales",
            role_ceiling: "viewer"
          }),
          grant({
            id: "g-3",
            kind: "app_publish",
            role_ceiling: null,
            app_id: "a-1",
            app_name: "store-ops"
          })
        ]
      })
    );
    expect(lines).toEqual([
      "Acme / every workspace — no cap",
      "Acme / Sales — up to viewer",
      "publish Acme / store-ops"
    ]);
  });

  it("leaves out a grant the org revoked, and says so when none is left", () => {
    const revoked = grant({ revoked_at: "2026-09-01T00:00:00Z" });
    expect(describeReach(token({ all_access: false, grants: [revoked] }))).toEqual([
      "nothing — every grant has been revoked"
    ]);
  });

  it("names the orgs whose policy makes the token inert there", () => {
    const lines = describeReach(
      token({
        blocked_orgs: [{ org_id: "org-2", org_name: "Globex", reason: "lifetime over 30 days" }]
      })
    );
    expect(lines).toEqual(["all access", "blocked in Globex: lifetime over 30 days"]);
  });
});
