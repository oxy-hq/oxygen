import { describe, expect, it } from "vitest";
import {
  CLOUD_NAV,
  gateSatisfied,
  LOCAL_NAV,
  type NavVisibilityContext,
  RENAMED_SECTIONS,
  visibleNavGroups
} from "./nav";

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
        // Their own personal access tokens; the admin tabs are gated inside.
        "organization.api_tokens",
        "workspace.members",
        // Their own read-only `Reader` credential — see the note in nav.ts.
        "workspace.airhouse",
        "workspace.oltp",
        "workspace.activity_logs",
        "account.notifications",
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
        "organization.secrets",
        "organization.billing",
        "organization.integration",
        "workspace.databases",
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
        // Personal tokens need no workspace: an org still being set up can mint one.
        "organization.api_tokens",
        "account.notifications",
        "preferences.appearance"
      ]);
    });

    it("offers their own tokens to anyone signed in, with no org or workspace loaded", () => {
      // A token belongs to its owner, not to a tenant: no role gates it, and it must be
      // reachable from the "you're not in an organization yet" state too. With no org it moves
      // to the Account group, since the Organization group is dropped.
      expect(sectionsFor({ hasOrg: false, hasWorkspace: false })).toEqual([
        "organization.api_tokens",
        "account.notifications",
        "preferences.appearance"
      ]);
      const account = visibleNavGroups({ ...CLOUD_BASE, hasOrg: false, hasWorkspace: false }).find(
        (g) => g.label === "Account"
      );
      expect(account?.items.map((i) => i.value)).toContain("organization.api_tokens");
    });

    it("lists API tokens once, under Organization, when an org is loaded", () => {
      const groups = visibleNavGroups(CLOUD_BASE);
      const holders = groups.filter((g) =>
        g.items.some((i) => i.value === "organization.api_tokens")
      );
      expect(holders.map((g) => g.label)).toEqual(["Organization"]);
    });

    it("keeps Secrets out of an org with no workspace yet, whose routes it could not call", () => {
      expect(
        sectionsFor({ isOrgAdmin: true, isWorkspaceAdmin: true, hasWorkspace: false })
      ).not.toContain("organization.secrets");
    });

    it("has no API tokens and no Account group in local mode, which has no sign-in", () => {
      expect(sectionsFor({ isLocalMode: true, hasOrg: false })).not.toContain(
        "organization.api_tokens"
      );
      // Nor anyone to notify.
      expect(sectionsFor({ isLocalMode: true, hasOrg: false })).not.toContain(
        "account.notifications"
      );
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
      // Filed under Organization, gated on the workspace role its routes check.
      expect(sections).toContain("organization.secrets");
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
        "organization.api_tokens",
        "organization.secrets",
        "organization.billing",
        "organization.integration",
        "workspace.members",
        "workspace.databases",
        "workspace.airhouse",
        "workspace.oltp",
        "workspace.connections",
        "workspace.apps",
        "workspace.oxy_access",
        "workspace.activity_logs",
        "account.notifications",
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
      expect(sections).not.toContain("organization.secrets");
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
      expect(sections).toContain("organization.secrets");
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

  describe("unloaded context", () => {
    it("drops the workspace group before the workspace resolves", () => {
      expect(groupsFor({ hasWorkspace: false })).not.toContain("Workspace");
    });

    it("drops the organization group before the org resolves", () => {
      expect(groupsFor({ hasOrg: false })).not.toContain("Organization");
    });
  });
});

describe("sections that write the workspace's config are the Factory's", () => {
  const FACTORY_SECTIONS = ["workspace.databases", "organization.secrets", "workspace.connections"];

  /// A workspace admin of the customer's org the server will not let into the
  /// Factory would open each of these and meet a refusal; they are not offered.
  it("are hidden from a workspace admin the server does not let into the Factory", () => {
    const sections = sectionsFor({
      isOrgAdmin: true,
      isWorkspaceAdmin: true,
      canUseFactory: false
    });
    for (const hidden of FACTORY_SECTIONS) expect(sections).not.toContain(hidden);
    // The rest of what a workspace admin has is untouched.
    expect(sections).toContain("organization.api_tokens");
    expect(sections).toContain("workspace.oxy_access");
  });

  it("are shown to one the server lets in, and while the answer is absent", () => {
    for (const canUseFactory of [true, undefined]) {
      const sections = sectionsFor({
        isOrgAdmin: true,
        isWorkspaceAdmin: true,
        canUseFactory
      });
      for (const shown of FACTORY_SECTIONS) expect(sections).toContain(shown);
    }
  });

  it("are not hidden in local mode, where there is no Factory to gate", () => {
    const sections = sectionsFor({ isLocalMode: true, canUseFactory: false });
    expect(sections).toContain("workspace.databases");
  });
});

describe("sections that moved", () => {
  const everySection = (): string[] =>
    [...CLOUD_NAV, ...LOCAL_NAV].flatMap((g) => g.items.map((i) => i.value));

  it("sends every old id to a section the nav still has", () => {
    // An old id is a link someone holds: it must land on a real section, never on nothing.
    for (const [old, target] of Object.entries(RENAMED_SECTIONS)) {
      expect(everySection(), old).toContain(target.section);
      expect(everySection(), old).not.toContain(old);
    }
  });

  it("names only tabs the target section has", () => {
    for (const [old, target] of Object.entries(RENAMED_SECTIONS)) {
      if (!target.tab) continue;
      const item = CLOUD_NAV.flatMap((g) => g.items).find((i) => i.value === target.section);
      expect(item?.tabs, old).toContain(target.tab);
    }
  });

  it("puts every token in one section, and every secret in another", () => {
    expect(RENAMED_SECTIONS).toEqual({
      "account.tokens": { section: "organization.api_tokens", tab: "personal" },
      "organization.api_access": { section: "organization.api_tokens", tab: "service_accounts" },
      "workspace.api_keys": { section: "organization.api_tokens", tab: "all" },
      "workspace.secrets": { section: "organization.secrets" }
    });
  });
});
