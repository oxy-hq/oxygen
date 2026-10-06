import { describe, expect, it } from "vitest";
import { gateSatisfied, type NavVisibilityContext, visibleNavGroups } from "./nav";
import { sectionFromParam } from "./useSettingsDeepLink";

/**
 * The settings nav is the dialog's entire permission surface, and a wrong gate
 * is invisible until someone holding that exact role opens Settings. These
 * assert the matrix directly.
 */

const CLOUD_BASE: NavVisibilityContext = {
  isLocalMode: false,
  isOrgAdmin: false,
  isWorkspaceAdmin: false,
  billingEnabled: true,
  hasOrg: true,
  hasWorkspace: true,
  orgName: "Acme",
  workspaceName: "Production"
};

const sectionsFor = (ctx: Partial<NavVisibilityContext>): string[] =>
  visibleNavGroups({ ...CLOUD_BASE, ...ctx }).flatMap((g) => g.items.map((i) => i.value));

const groupsFor = (ctx: Partial<NavVisibilityContext>): string[] =>
  visibleNavGroups({ ...CLOUD_BASE, ...ctx }).map((g) => g.label);

describe("visibleNavGroups", () => {
  describe("a plain org member", () => {
    // The reported bug: a member was shown General (blank), Airhouse and API
    // Keys (denial states), because only 5 of 15 items carried a gate.
    it("sees only the sections they can actually use", () => {
      expect(sectionsFor({})).toEqual([
        "organization.members",
        "workspace.members",
        // Their own read-only `Reader` credential — see the note in nav.ts.
        "workspace.airhouse",
        "workspace.oltp",
        // Their own legacy API keys: the server lists, extends and shows the
        // activity of a key to its owner with no admin requirement.
        "workspace.legacy_api_keys",
        "workspace.activity_logs",
        "account.tokens",
        "preferences.appearance"
      ]);
    });

    it("is not shown a section that renders a denial state", () => {
      const sections = sectionsFor({});
      for (const denied of [
        "organization.general",
        "organization.crew",
        "organization.locations",
        "organization.positions",
        "organization.api_access",
        "organization.billing",
        "organization.integration",
        "workspace.databases",
        "workspace.api_keys",
        "workspace.secrets",
        "workspace.apps",
        "workspace.oxy_access"
      ]) {
        expect(sections).not.toContain(denied);
      }
    });

    it("keeps the Organization, Account and Preferences groups before a workspace loads", () => {
      expect(groupsFor({ hasWorkspace: false })).toEqual([
        "Organization",
        "Account",
        "Preferences"
      ]);
      expect(sectionsFor({ hasWorkspace: false })).toEqual([
        "organization.members",
        "account.tokens",
        "preferences.appearance"
      ]);
    });

    it("offers their own tokens to anyone signed in, with no org or workspace loaded", () => {
      // A token belongs to its owner, not to a tenant: no role gates it, and it
      // must be reachable from the "you're not in an organization yet" state too.
      expect(sectionsFor({ hasOrg: false, hasWorkspace: false })).toEqual([
        "account.tokens",
        "preferences.appearance"
      ]);
    });

    it("has no Account group in local mode, which has no sign-in to mint a token for", () => {
      expect(sectionsFor({ isLocalMode: true, hasOrg: false })).not.toContain("account.tokens");
    });

    it("never renders an empty group heading", () => {
      // Every group still has at least one member-visible item today; the
      // guarantee is that a group with zero visible items is dropped entirely.
      for (const group of visibleNavGroups(CLOUD_BASE)) {
        expect(group.items.length).toBeGreaterThan(0);
      }
    });
  });

  describe("the two authority axes", () => {
    // The load-bearing case. The backend resolves workspace access as
    // `max(org_derived_role, workspace_member_override)`, so this population
    // really exists — gating workspace items on org role would hide sections
    // the server authorizes.
    it("grants workspace sections to an org member holding workspace admin", () => {
      const sections = sectionsFor({ isWorkspaceAdmin: true });
      expect(sections).toContain("workspace.databases");
      expect(sections).toContain("workspace.api_keys");
      expect(sections).toContain("workspace.legacy_api_keys");
      expect(sections).toContain("workspace.secrets");
      expect(sections).toContain("workspace.apps");
      expect(sections).toContain("workspace.airhouse");
    });

    it("withholds org sections from that same user", () => {
      const sections = sectionsFor({ isWorkspaceAdmin: true });
      expect(sections).not.toContain("organization.general");
      expect(sections).not.toContain("organization.billing");
      // Oxy access lives under the Workspace heading but is the tenant's
      // staff-access kill switch, so it stays on the org axis.
      expect(sections).not.toContain("workspace.oxy_access");
    });

    it("gives an ordinary admin every section", () => {
      // The shape nearly every real admin has: org admin derives workspace
      // admin server-side, so both flags arrive true together.
      const sections = sectionsFor({ isOrgAdmin: true, isWorkspaceAdmin: true });
      expect(sections).toEqual([
        "organization.general",
        "organization.members",
        "organization.teams",
        "organization.app_access",
        "organization.crew",
        "organization.locations",
        "organization.positions",
        "organization.api_access",
        "organization.billing",
        "organization.integration",
        "workspace.members",
        "workspace.databases",
        "workspace.airhouse",
        "workspace.oltp",
        "workspace.api_keys",
        "workspace.legacy_api_keys",
        "workspace.secrets",
        "workspace.connections",
        "workspace.apps",
        "workspace.oxy_access",
        "workspace.activity_logs",
        "account.tokens",
        "preferences.appearance"
      ]);
    });

    it("grants org sections to an org admin", () => {
      const sections = sectionsFor({ isOrgAdmin: true });
      expect(sections).toContain("organization.general");
      expect(sections).toContain("organization.teams");
      expect(sections).toContain("organization.crew");
      expect(sections).toContain("organization.locations");
      expect(sections).toContain("organization.positions");
      expect(sections).toContain("organization.integration");
      expect(sections).toContain("workspace.oxy_access");
    });

    it("does not let org admin alone imply workspace admin", () => {
      // An org admin resolves to workspace admin server-side, so in practice
      // both flags arrive true together; the gate must still read the axis it
      // was given rather than inferring one from the other.
      const sections = sectionsFor({ isOrgAdmin: true });
      expect(sections).not.toContain("workspace.secrets");
    });
  });

  describe("billing", () => {
    it("is hidden when the deployment has billing disabled", () => {
      expect(sectionsFor({ isOrgAdmin: true, billingEnabled: false })).not.toContain(
        "organization.billing"
      );
    });

    it("is shown to an org admin when billing is enabled", () => {
      expect(sectionsFor({ isOrgAdmin: true })).toContain("organization.billing");
    });
  });

  describe("local mode", () => {
    // NB: LOCAL_NAV carries no `requires` today, so this passes because there
    // is nothing to gate — not because of the isLocalMode exemption. The
    // exemption itself is asserted directly below.
    it("shows every workspace section despite carrying no role", () => {
      const sections = sectionsFor({ isLocalMode: true, hasOrg: false });
      expect(sections).toContain("workspace.secrets");
      expect(sections).toContain("workspace.apps");
      expect(sections).toContain("workspace.databases");
      expect(sections).toContain("preferences.appearance");
    });

    it("exempts a gated item from both axes", () => {
      // Local has no org and therefore no org role, so the first `orgAdmin`
      // gate added to LOCAL_NAV would otherwise hide that item from the one
      // seeded user the server treats as Owner of everything.
      const local = { ...CLOUD_BASE, isLocalMode: true, hasOrg: false };
      expect(gateSatisfied("orgAdmin", local)).toBe(true);
      expect(gateSatisfied("workspaceAdmin", local)).toBe(true);
    });

    it("still enforces both axes outside local mode", () => {
      expect(gateSatisfied("orgAdmin", CLOUD_BASE)).toBe(false);
      expect(gateSatisfied("workspaceAdmin", CLOUD_BASE)).toBe(false);
      expect(gateSatisfied(undefined, CLOUD_BASE)).toBe(true);
    });

    it("omits the organization group entirely", () => {
      expect(groupsFor({ isLocalMode: true, hasOrg: false })).toEqual(["Workspace", "Preferences"]);
    });
  });

  describe("the staff axis", () => {
    // Previews are an Oxy-staff tool, never the customer's: no role inside the
    // tenant — not even owning the org — reaches them.
    it("hides Previews from every tenant role", () => {
      for (const ctx of [
        {},
        { isWorkspaceAdmin: true },
        { isOrgAdmin: true, isWorkspaceAdmin: true }
      ]) {
        expect(sectionsFor(ctx)).not.toContain("workspace.previews");
      }
      expect(
        gateSatisfied("staff", { ...CLOUD_BASE, isOrgAdmin: true, isWorkspaceAdmin: true })
      ).toBe(false);
    });

    it("shows Previews to Oxy staff, whatever their role in the tenant", () => {
      expect(sectionsFor({ isStaff: true })).toContain("workspace.previews");
      expect(sectionsFor({ isStaff: true, isOrgAdmin: true })).toContain("workspace.previews");
    });

    it("keeps Previews out of an org with no workspace loaded", () => {
      expect(sectionsFor({ isStaff: true, hasWorkspace: false })).not.toContain(
        "workspace.previews"
      );
    });
  });

  describe("legacy API keys", () => {
    const labelsFor = (ctx: Partial<NavVisibilityContext>): string[] =>
      visibleNavGroups({ ...CLOUD_BASE, ...ctx })
        .filter((g) => g.label === "Workspace")
        .flatMap((g) => g.items.map((i) => i.label));

    it("sit in their own entry, right after API tokens, for a workspace admin", () => {
      const labels = labelsFor({ isWorkspaceAdmin: true });
      expect(labels.indexOf("Legacy API keys")).toBe(labels.indexOf("API tokens") + 1);
    });

    it("are shown to a member who is no workspace admin: a key belongs to its owner", () => {
      // The split that matters. The inventory of everyone's tokens stays an
      // admin's; a person's own legacy keys do not need the role.
      const sections = sectionsFor({});
      expect(sections).toContain("workspace.legacy_api_keys");
      expect(sections).not.toContain("workspace.api_keys");
      expect(labelsFor({})).toContain("Legacy API keys");
      expect(labelsFor({})).not.toContain("API tokens");
    });

    it("carry no gate on any axis", () => {
      for (const ctx of [
        {},
        { isWorkspaceAdmin: true },
        { isOrgAdmin: true },
        { isOrgAdmin: true, isWorkspaceAdmin: true },
        { isStaff: true }
      ]) {
        expect(sectionsFor(ctx)).toContain("workspace.legacy_api_keys");
      }
    });

    it("go with the Workspace group when no workspace is loaded", () => {
      expect(sectionsFor({ hasWorkspace: false })).not.toContain("workspace.legacy_api_keys");
    });

    it("keep their entry in local mode, where the keys used to share API tokens", () => {
      const labels = visibleNavGroups({ ...CLOUD_BASE, isLocalMode: true, hasOrg: false })
        .filter((g) => g.label === "Workspace")
        .flatMap((g) => g.items.map((i) => i.label));
      expect(labels.indexOf("Legacy API keys")).toBe(labels.indexOf("API tokens") + 1);
    });

    it("are reachable by the link a legacy key's expiry email carries, admin or not", () => {
      // `?settings=workspace.legacy_api_keys` is written by the backend: the id is a contract.
      expect(sectionFromParam("workspace.legacy_api_keys")).toBe("workspace.legacy_api_keys");
      expect(sectionFromParam("workspace.api_keys")).toBe("workspace.api_keys");
      // The dialog opens a linked section only if it is in the viewer's nav; anything else falls
      // to the first visible one. A non-admin owner's link must therefore find it there.
      expect(sectionsFor({})).toContain(sectionFromParam("workspace.legacy_api_keys"));
    });
  });

  describe("unloaded context", () => {
    it("drops the workspace group before the workspace resolves", () => {
      expect(groupsFor({ hasWorkspace: false })).not.toContain("Workspace");
    });

    it("drops the organization group before the org resolves", () => {
      expect(groupsFor({ hasOrg: false })).not.toContain("Organization");
    });
  });
});
