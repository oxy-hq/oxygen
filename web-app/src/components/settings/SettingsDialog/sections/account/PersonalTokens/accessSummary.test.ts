import { describe, expect, it } from "vitest";
import type { Grant, Token } from "@/types/apiToken";
import { maskedToken, summarizeAccess, toTokenSummary } from "./accessSummary";

const grant = (over: Partial<Grant>): Grant => ({
  id: "g1",
  kind: "workspace",
  org_id: "o1",
  org_name: "Acme",
  workspace_id: "w1",
  workspace_name: "Analytics",
  role_ceiling: "viewer",
  app_id: null,
  app_name: null,
  revoked_at: null,
  ...over
});

const token = (over: Partial<Token>): Token => ({
  id: "t1",
  name: "laptop",
  kind: "personal",
  display_prefix: "oxy_pat_Ab3x",
  last_four: "wxyz",
  all_access: false,
  platform: false,
  partner: false,
  grants: [],
  expires_at: null,
  last_used_at: null,
  created_at: "2026-10-01T00:00:00Z",
  revoked_at: null,
  status: "active",
  source: "ui",
  owner: { type: "user", id: "u1", label: "me@example.com" },
  blocked_orgs: [],
  ...over
});

describe("summarizeAccess", () => {
  it("says All access for an all-access token", () => {
    expect(summarizeAccess(token({ all_access: true })).label).toBe("All access");
  });

  it("counts workspaces and the orgs they are in", () => {
    const summary = summarizeAccess(
      token({
        grants: [
          grant({ id: "a" }),
          grant({ id: "b", workspace_id: "w2", workspace_name: "Finance" }),
          grant({ id: "c", org_id: "o2", org_name: "Globex", workspace_id: "w3" })
        ]
      })
    );
    expect(summary.label).toBe("3 workspaces in 2 orgs");
  });

  it("uses the singular for one workspace in one org", () => {
    expect(summarizeAccess(token({ grants: [grant({})] })).label).toBe("1 workspace in 1 org");
  });

  it("names the org when the only grant covers all of it", () => {
    const summary = summarizeAccess(
      token({
        grants: [grant({ workspace_id: null, workspace_name: null, role_ceiling: "admin" })]
      })
    );
    expect(summary.label).toBe("All workspaces in Acme");
    expect(summary.lines).toEqual(["Acme: every workspace (Admin)"]);
  });

  it("combines picked workspaces with an org-wide grant elsewhere", () => {
    const summary = summarizeAccess(
      token({
        grants: [
          grant({}),
          grant({
            id: "b",
            org_id: "o2",
            org_name: "Globex",
            workspace_id: null,
            workspace_name: null
          })
        ]
      })
    );
    expect(summary.label).toBe("1 workspace in 1 org, all of Globex");
  });

  it("ignores a grant its org revoked, and says so in the detail", () => {
    const summary = summarizeAccess(
      token({ grants: [grant({ revoked_at: "2026-10-02T00:00:00Z" })] })
    );
    expect(summary.label).toBe("No access");
    expect(summary.lines[0]).toContain("removed by the organization");
  });

  it("says which org cut an all-access token off", () => {
    // The server lists an all-access token's grants only where an org ended its reach.
    const summary = summarizeAccess(
      token({
        all_access: true,
        grants: [grant({ workspace_id: null, revoked_at: "2026-10-02T00:00:00Z" })]
      })
    );
    expect(summary.label).toBe("All access");
    expect(summary.lines).toHaveLength(2);
    expect(summary.lines[1]).toContain("Acme: every workspace");
    expect(summary.lines[1]).toContain("removed by the organization");
  });

  it("carries staff and partner standing as badges, and blocked orgs as a warning", () => {
    const summary = summarizeAccess(
      token({
        all_access: true,
        platform: true,
        partner: true,
        blocked_orgs: [{ org_id: "o2", org_name: "Globex", reason: "all-access tokens are off" }]
      })
    );
    expect(summary.standing).toEqual(["platform", "partner"]);
    expect(summary.blocked).toHaveLength(1);
  });

  it("counts an app-publish grant on its own", () => {
    const summary = summarizeAccess(
      token({
        grants: [
          grant({
            kind: "app_publish",
            workspace_id: null,
            role_ceiling: null,
            app_id: "a1",
            app_name: "Store Ops"
          })
        ]
      })
    );
    expect(summary.label).toBe("publish 1 app");
    expect(summary.lines).toEqual(["Acme: publish Store Ops"]);
  });
});

describe("maskedToken", () => {
  it("joins the display prefix and the last four", () => {
    expect(maskedToken({ display_prefix: "oxy_pat_Ab3x", last_four: "wxyz" })).toBe(
      "oxy_pat_Ab3x…wxyz"
    );
  });

  it("does not double an ellipsis the server already sent", () => {
    expect(maskedToken({ display_prefix: "oxy_pat_Ab3x…", last_four: "wxyz" })).toBe(
      "oxy_pat_Ab3x…wxyz"
    );
    expect(maskedToken({ display_prefix: "oxy_Ab3x...", last_four: "" })).toBe("oxy_Ab3x…");
  });
});

describe("toTokenSummary", () => {
  it("reads revoked from status, and leaves expiry to the clock", () => {
    expect(toTokenSummary(token({ status: "revoked" })).is_active).toBe(false);
    // Expired is still "active" here: the badge judges expiry from `expires_at`.
    expect(toTokenSummary(token({ status: "expired" })).is_active).toBe(true);
    expect(toTokenSummary(token({})).masked_key).toBe("oxy_pat_Ab3x…wxyz");
  });
});
