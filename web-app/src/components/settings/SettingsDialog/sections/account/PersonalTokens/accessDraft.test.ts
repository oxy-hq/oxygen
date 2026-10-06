import { describe, expect, it } from "vitest";
import type { Grant, TokenOptionOrg } from "@/types/apiToken";
import {
  type AccessAction,
  type AccessDraft,
  accessInputFromDraft,
  accessReducer,
  draftFromToken,
  draftProblem,
  effectiveCeiling,
  emptyDraft,
  grantsFromDraft,
  isRevokedTarget,
  lifetimeCap,
  orgsRefusingAllAccess,
  partnerOrgsWithoutStanding
} from "./accessDraft";

const apply = (actions: AccessAction[], from: AccessDraft = emptyDraft()): AccessDraft =>
  actions.reduce(accessReducer, from);

const pick = (orgId: string, workspaceId: string): AccessAction => ({
  type: "set_workspace",
  orgId,
  workspaceId,
  on: true
});

const org = (over: Partial<TokenOptionOrg>): TokenOptionOrg => ({
  org_id: "o1",
  org_name: "Acme",
  org_slug: "acme",
  role: "member",
  via: "member",
  workspaces: [],
  policy: { max_lifetime_days: null, allow_all_access_tokens: true },
  ...over
});

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

describe("accessReducer", () => {
  it("starts on all access with no standing", () => {
    expect(emptyDraft()).toMatchObject({ mode: "all", platform: false, partner: false });
  });

  it("picks a workspace at the Full ceiling, then narrows it", () => {
    const draft = apply([
      { type: "set_mode", mode: "selected" },
      pick("o1", "w1"),
      { type: "set_workspace_ceiling", orgId: "o1", workspaceId: "w1", ceiling: "viewer" }
    ]);
    expect(grantsFromDraft(draft)).toEqual([
      { kind: "workspace", org_id: "o1", workspace_id: "w1", role_ceiling: "viewer" }
    ]);
  });

  it("drops an org once its last workspace is unpicked", () => {
    const draft = apply([
      pick("o1", "w1"),
      { type: "set_workspace", orgId: "o1", workspaceId: "w1", on: false }
    ]);
    expect(draft.orgs).toEqual({});
  });

  it("ignores a ceiling for a workspace that isn't picked", () => {
    const before = apply([pick("o1", "w1")]);
    const after = accessReducer(before, {
      type: "set_workspace_ceiling",
      orgId: "o1",
      workspaceId: "w2",
      ceiling: "admin"
    });
    expect(after).toBe(before);
  });

  it("sends one org-wide grant instead of the workspaces it already covers", () => {
    const draft = apply([
      pick("o1", "w1"),
      pick("o1", "w2"),
      { type: "set_org_wide", orgId: "o1", on: true },
      { type: "set_org_ceiling", orgId: "o1", ceiling: "admin" }
    ]);
    expect(grantsFromDraft(draft)).toEqual([
      { kind: "workspace", org_id: "o1", workspace_id: null, role_ceiling: "admin" }
    ]);
  });

  it("restores the picked workspaces when org-wide is turned back off", () => {
    const draft = apply([
      pick("o1", "w1"),
      { type: "set_org_wide", orgId: "o1", on: true },
      { type: "set_org_wide", orgId: "o1", on: false }
    ]);
    expect(grantsFromDraft(draft).map((g) => g.workspace_id)).toEqual(["w1"]);
  });

  it("keeps the picks across a switch to all access and back", () => {
    const draft = apply([
      { type: "set_mode", mode: "selected" },
      pick("o1", "w1"),
      { type: "set_mode", mode: "all" },
      { type: "set_mode", mode: "selected" }
    ]);
    expect(Object.keys(draft.orgs)).toEqual(["o1"]);
  });
});

describe("accessInputFromDraft", () => {
  it("sends all_access with an empty grant set, so an edit clears old grants", () => {
    const draft = apply([pick("o1", "w1"), { type: "set_mode", mode: "all" }]);
    expect(accessInputFromDraft(draft)).toEqual({
      all_access: true,
      platform: false,
      partner: false,
      grants: []
    });
  });

  it("sends the standing flags the person ticked", () => {
    const draft = apply([{ type: "set_standing", flag: "platform", on: true }]);
    expect(accessInputFromDraft(draft)).toMatchObject({ platform: true, partner: false });
  });

  it("sends all_access false with the picked grants", () => {
    const draft = apply([{ type: "set_mode", mode: "selected" }, pick("o1", "w1")]);
    expect(accessInputFromDraft(draft)).toMatchObject({
      all_access: false,
      grants: [{ org_id: "o1", workspace_id: "w1", role_ceiling: "owner" }]
    });
  });

  it("drops a standing flag the caller no longer holds, so the edit isn't refused", () => {
    const draft = apply([
      { type: "set_standing", flag: "platform", on: true },
      { type: "set_standing", flag: "partner", on: true }
    ]);
    expect(accessInputFromDraft(draft, { can_platform: false, can_partner: true })).toMatchObject({
      platform: false,
      partner: true
    });
  });
});

describe("draftProblem", () => {
  it("refuses Selected with nothing selected, as the server would", () => {
    expect(draftProblem(apply([{ type: "set_mode", mode: "selected" }]))).toMatch(/at least one/);
    expect(draftProblem(apply([{ type: "set_mode", mode: "selected" }, pick("o1", "w1")]))).toBe(
      null
    );
    expect(draftProblem(emptyDraft())).toBe(null);
  });
});

describe("draftFromToken", () => {
  it("round-trips a token's grants through the picker unchanged", () => {
    const draft = draftFromToken({
      all_access: false,
      platform: false,
      partner: true,
      grants: [
        grant({}),
        grant({ id: "g2", org_id: "o2", workspace_id: null, role_ceiling: "admin" })
      ]
    });
    expect(draft.mode).toBe("selected");
    expect(draft.partner).toBe(true);
    expect(grantsFromDraft(draft)).toEqual([
      { kind: "workspace", org_id: "o1", workspace_id: "w1", role_ceiling: "viewer" },
      { kind: "workspace", org_id: "o2", workspace_id: null, role_ceiling: "admin" }
    ]);
  });

  it("does not re-request a grant the org revoked", () => {
    const draft = draftFromToken({
      all_access: false,
      platform: false,
      partner: false,
      grants: [grant({ revoked_at: "2026-10-02T00:00:00Z" })]
    });
    expect(grantsFromDraft(draft)).toEqual([]);
  });

  it("locks a target its org took away, so it is never asked for again", () => {
    // The server keeps a revoked grant as history and silently refuses the same target, so a
    // tick on it would save as "updated" and change nothing.
    const draft = draftFromToken({
      all_access: false,
      platform: false,
      partner: false,
      grants: [
        grant({ id: "g1", revoked_at: "2026-10-02T00:00:00Z" }),
        grant({ id: "g2", org_id: "o2", workspace_id: null, revoked_at: "2026-10-02T00:00:00Z" }),
        grant({ id: "g3", workspace_id: "w2", workspace_name: "Finance" })
      ]
    });
    expect(draft.revoked).toEqual([
      { orgId: "o1", workspaceId: "w1" },
      { orgId: "o2", workspaceId: null }
    ]);
    expect(isRevokedTarget(draft, "o1", "w1")).toBe(true);
    expect(isRevokedTarget(draft, "o2", null)).toBe(true);
    // Another workspace in the same org, and a single workspace under a revoked org-wide
    // grant, are different targets: both can still be picked.
    expect(isRevokedTarget(draft, "o1", "w2")).toBe(false);
    expect(isRevokedTarget(draft, "o2", "w9")).toBe(false);

    // Even if a pick for a locked target reached the draft, it stays out of the body.
    const forced = accessReducer(draft, {
      type: "set_workspace",
      orgId: "o1",
      workspaceId: "w1",
      on: true
    });
    expect(grantsFromDraft(forced)).toEqual([
      { kind: "workspace", org_id: "o1", workspace_id: "w2", role_ceiling: "viewer" }
    ]);
  });

  it("has nothing locked on a new token", () => {
    expect(emptyDraft().revoked).toEqual([]);
  });

  it("never turns a sandbox agent token's app grant into every workspace in the org", () => {
    // Its `workspace_id` is `null`, which on a workspace grant means org-wide. No row offers
    // Edit access for the kind, and this is what keeps a stray call from widening one.
    const draft = draftFromToken({
      all_access: false,
      platform: true,
      partner: false,
      grants: [
        grant({
          kind: "app_sandbox",
          workspace_id: null,
          workspace_name: null,
          role_ceiling: null,
          app_id: "a1",
          app_name: "Store Ops"
        })
      ]
    });
    expect(draft.orgs).toEqual({});
    expect(grantsFromDraft(draft)).toEqual([]);
  });

  it("carries an app-publish grant through an edit instead of dropping it", () => {
    const draft = draftFromToken({
      all_access: false,
      platform: false,
      partner: false,
      grants: [grant({ kind: "app_publish", workspace_id: null, role_ceiling: null, app_id: "a1" })]
    });
    expect(grantsFromDraft(draft)).toEqual([{ kind: "app_publish", org_id: "o1", app_id: "a1" }]);
    expect(draftProblem(draft)).toBe(null);
  });
});

describe("lifetimeCap", () => {
  const options = {
    orgs: [
      org({ policy: { max_lifetime_days: 30, allow_all_access_tokens: true } }),
      org({
        org_id: "o2",
        org_name: "Globex",
        policy: { max_lifetime_days: 7, allow_all_access_tokens: false }
      }),
      org({ org_id: "o3", org_name: "Initech" })
    ]
  };

  it("takes the tightest cap among the orgs the token is narrowed to", () => {
    const draft = apply([
      { type: "set_mode", mode: "selected" },
      pick("o1", "w1"),
      pick("o2", "w9")
    ]);
    expect(lifetimeCap(draft, options)).toEqual({ days: 7, orgName: "Globex" });
  });

  it("ignores the policy of an org the token doesn't name", () => {
    const draft = apply([{ type: "set_mode", mode: "selected" }, pick("o1", "w1")]);
    expect(lifetimeCap(draft, options)).toEqual({ days: 30, orgName: "Acme" });
    const uncapped = apply([{ type: "set_mode", mode: "selected" }, pick("o3", "w1")]);
    expect(lifetimeCap(uncapped, options)).toBe(null);
  });

  it("does not cap an all-access token, even with picks left over from Selected", () => {
    const draft = apply([pick("o2", "w9"), { type: "set_mode", mode: "all" }]);
    expect(lifetimeCap(draft, options)).toBe(null);
  });

  it("names the orgs that turn all-access tokens away", () => {
    expect(orgsRefusingAllAccess(options)).toEqual(["Globex"]);
    expect(orgsRefusingAllAccess(undefined)).toEqual([]);
  });
});

describe("partnerOrgsWithoutStanding", () => {
  const options = { orgs: [org({ via: "partner" }), org({ org_id: "o2", org_name: "Globex" })] };
  const picked = apply([
    { type: "set_mode", mode: "selected" },
    pick("o1", "w1"),
    pick("o2", "w2")
  ]);

  it("flags a partner-reached org picked without partner access", () => {
    expect(partnerOrgsWithoutStanding(picked, options)).toEqual(["Acme"]);
  });

  it("is quiet once partner access is included", () => {
    const withPartner = accessReducer(picked, { type: "set_standing", flag: "partner", on: true });
    expect(partnerOrgsWithoutStanding(withPartner, options)).toEqual([]);
  });
});

describe("effectiveCeiling", () => {
  it("is the lower of the grant's ceiling and the caller's role", () => {
    expect(effectiveCeiling("owner", "member")).toBe("member");
    expect(effectiveCeiling("viewer", "admin")).toBe("viewer");
    expect(effectiveCeiling("admin", "admin")).toBe("admin");
  });
});
