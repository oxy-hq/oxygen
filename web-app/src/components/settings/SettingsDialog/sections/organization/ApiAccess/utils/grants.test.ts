import { describe, expect, it } from "vitest";
import type { Grant } from "@/types/apiToken";
import {
  type AccessDraft,
  accessDraftError,
  buildGrantInputs,
  ceilingOptions,
  clampCeiling,
  describeAccess,
  draftFromGrants,
  NOTHING_SELECTED,
  ORG_WIDE_ACCESS,
  setWorkspaceCeiling,
  toggleApp,
  toggleWorkspace
} from "./grants";

const grant = (over: Partial<Grant>): Grant => ({
  id: "g1",
  kind: "workspace",
  org_id: "org-1",
  org_name: "Acme",
  workspace_id: "ws-1",
  workspace_name: "Analytics",
  role_ceiling: "member",
  app_id: null,
  app_name: null,
  revoked_at: null,
  ...over
});

describe("ceilingOptions", () => {
  it("never offers a ceiling above the account's own role", () => {
    expect(ceilingOptions("member")).toEqual(["viewer", "member"]);
    expect(ceilingOptions("admin")).toEqual(["viewer", "member", "admin"]);
  });

  it("never offers owner", () => {
    expect(ceilingOptions("admin")).not.toContain("owner");
  });
});

describe("clampCeiling", () => {
  it("lowers a ceiling above the account's role", () => {
    expect(clampCeiling("admin", "member")).toBe("member");
    expect(clampCeiling("owner", "admin")).toBe("admin");
  });

  it("leaves a ceiling at or below the role alone", () => {
    expect(clampCeiling("viewer", "member")).toBe("viewer");
    expect(clampCeiling("admin", "admin")).toBe("admin");
  });
});

describe("editing a draft", () => {
  it("adds a workspace at the given ceiling, and removes it on a second toggle", () => {
    const added = toggleWorkspace(NOTHING_SELECTED, "ws-1", "viewer");
    expect(added.workspaces).toEqual([{ workspace_id: "ws-1", role_ceiling: "viewer" }]);
    expect(toggleWorkspace(added, "ws-1", "viewer").workspaces).toEqual([]);
  });

  it("changes one workspace's ceiling without touching the others", () => {
    const draft: AccessDraft = {
      scope: "selected",
      workspaces: [
        { workspace_id: "ws-1", role_ceiling: "member" },
        { workspace_id: "ws-2", role_ceiling: "member" }
      ],
      appIds: []
    };
    expect(setWorkspaceCeiling(draft, "ws-2", "viewer").workspaces).toEqual([
      { workspace_id: "ws-1", role_ceiling: "member" },
      { workspace_id: "ws-2", role_ceiling: "viewer" }
    ]);
  });

  it("toggles an app", () => {
    const added = toggleApp(NOTHING_SELECTED, "app-1");
    expect(added.appIds).toEqual(["app-1"]);
    expect(toggleApp(added, "app-1").appIds).toEqual([]);
  });

  it("does not mutate the draft it was given", () => {
    toggleWorkspace(NOTHING_SELECTED, "ws-1", "viewer");
    toggleApp(NOTHING_SELECTED, "app-1");
    expect(NOTHING_SELECTED).toEqual({ scope: "selected", workspaces: [], appIds: [] });
  });
});

describe("accessDraftError", () => {
  it("accepts the whole org", () => {
    expect(accessDraftError(ORG_WIDE_ACCESS)).toBeNull();
  });

  it("refuses an empty selection", () => {
    expect(accessDraftError(NOTHING_SELECTED)).toBe("Pick at least one workspace.");
    expect(accessDraftError(NOTHING_SELECTED, true)).toBe(
      "Pick at least one workspace or one app."
    );
  });

  it("counts an app only where apps can be granted", () => {
    const appOnly: AccessDraft = { scope: "selected", workspaces: [], appIds: ["app-1"] };
    expect(accessDraftError(appOnly, true)).toBeNull();
    expect(accessDraftError(appOnly, false)).not.toBeNull();
  });
});

describe("buildGrantInputs", () => {
  it("leaves grants out for the whole org, so the server applies the account's role", () => {
    expect(buildGrantInputs(ORG_WIDE_ACCESS, "member")).toBeUndefined();
  });

  it("spells the org-wide grant out when asked to", () => {
    expect(buildGrantInputs(ORG_WIDE_ACCESS, "admin", { explicitOrgWide: true })).toEqual([
      { kind: "workspace", workspace_id: null, role_ceiling: "admin" }
    ]);
  });

  it("spells the org-wide grant out when an app grant has to ride along", () => {
    const draft: AccessDraft = { scope: "org", workspaces: [], appIds: ["app-1"] };
    expect(buildGrantInputs(draft, "member")).toEqual([
      { kind: "workspace", workspace_id: null, role_ceiling: "member" },
      { kind: "app_publish", app_id: "app-1" }
    ]);
  });

  it("sends one grant per selected workspace with its ceiling", () => {
    const draft: AccessDraft = {
      scope: "selected",
      workspaces: [
        { workspace_id: "ws-1", role_ceiling: "viewer" },
        { workspace_id: "ws-2", role_ceiling: "member" }
      ],
      appIds: []
    };
    expect(buildGrantInputs(draft, "member")).toEqual([
      { kind: "workspace", workspace_id: "ws-1", role_ceiling: "viewer" },
      { kind: "workspace", workspace_id: "ws-2", role_ceiling: "member" }
    ]);
  });

  it("never sends a ceiling above the account's role", () => {
    const draft: AccessDraft = {
      scope: "selected",
      workspaces: [{ workspace_id: "ws-1", role_ceiling: "admin" }],
      appIds: []
    };
    expect(buildGrantInputs(draft, "member")).toEqual([
      { kind: "workspace", workspace_id: "ws-1", role_ceiling: "member" }
    ]);
  });

  it("never sends an org id: the route implies it", () => {
    const draft: AccessDraft = {
      scope: "selected",
      workspaces: [{ workspace_id: "ws-1", role_ceiling: "member" }],
      appIds: ["app-1"]
    };
    for (const input of buildGrantInputs(draft, "member") ?? []) {
      expect(input).not.toHaveProperty("org_id");
    }
  });

  it("sends app-publish grants without a ceiling", () => {
    const draft: AccessDraft = { scope: "selected", workspaces: [], appIds: ["app-1", "app-2"] };
    expect(buildGrantInputs(draft, "member")).toEqual([
      { kind: "app_publish", app_id: "app-1" },
      { kind: "app_publish", app_id: "app-2" }
    ]);
  });
});

describe("draftFromGrants", () => {
  it("reads an org-wide grant back as the whole org", () => {
    expect(draftFromGrants([grant({ workspace_id: null, workspace_name: null })])).toEqual(
      ORG_WIDE_ACCESS
    );
  });

  it("reads workspace and app grants back as a selection", () => {
    const draft = draftFromGrants([
      grant({ workspace_id: "ws-1", role_ceiling: "viewer" }),
      grant({
        id: "g2",
        kind: "app_publish",
        workspace_id: null,
        role_ceiling: null,
        app_id: "app-1",
        app_name: "Store Ops"
      })
    ]);
    expect(draft).toEqual({
      scope: "selected",
      workspaces: [{ workspace_id: "ws-1", role_ceiling: "viewer" }],
      appIds: ["app-1"]
    });
  });

  it("ignores a grant the org revoked", () => {
    const draft = draftFromGrants([grant({ revoked_at: "2026-09-01T00:00:00Z" })]);
    expect(draft).toEqual(NOTHING_SELECTED);
  });

  it("round-trips through buildGrantInputs", () => {
    const grants = [
      grant({ workspace_id: "ws-1", role_ceiling: "viewer" }),
      grant({ id: "g2", workspace_id: "ws-2", role_ceiling: "member" })
    ];
    expect(buildGrantInputs(draftFromGrants(grants), "member")).toEqual([
      { kind: "workspace", workspace_id: "ws-1", role_ceiling: "viewer" },
      { kind: "workspace", workspace_id: "ws-2", role_ceiling: "member" }
    ]);
  });
});

describe("describeAccess", () => {
  it("names the whole org and the role", () => {
    const access = describeAccess([grant({ workspace_id: null, workspace_name: null })]);
    expect(access.summary).toBe("Every workspace, as Member");
  });

  it("names a single workspace", () => {
    expect(describeAccess([grant({ role_ceiling: "viewer" })]).summary).toBe("Analytics as Viewer");
  });

  it("counts several workspaces and lists each one in the details", () => {
    const access = describeAccess([
      grant({}),
      grant({ id: "g2", workspace_id: "ws-2", workspace_name: "Finance", role_ceiling: "viewer" })
    ]);
    expect(access.summary).toBe("2 workspaces");
    expect(access.details).toEqual(["Analytics as Member", "Finance as Viewer"]);
  });

  it("joins workspace and app access into one phrase", () => {
    const access = describeAccess([
      grant({}),
      grant({
        id: "g2",
        kind: "app_publish",
        workspace_id: null,
        workspace_name: null,
        role_ceiling: null,
        app_id: "app-1",
        app_name: "Store Ops"
      })
    ]);
    expect(access.summary).toBe("Analytics as Member and publish Store Ops");
  });

  it("does not read an app-publish grant as org-wide workspace access", () => {
    const access = describeAccess([
      grant({
        kind: "app_publish",
        workspace_id: null,
        workspace_name: null,
        role_ceiling: null,
        app_id: "app-1",
        app_name: "Store Ops"
      })
    ]);
    expect(access.summary).toBe("Publish Store Ops");
  });

  it("says so when every grant was revoked, and when there never was one", () => {
    expect(describeAccess([grant({ revoked_at: "2026-09-01T00:00:00Z" })]).summary).toBe(
      "Access revoked"
    );
    expect(describeAccess([]).summary).toBe("No access");
  });
});
