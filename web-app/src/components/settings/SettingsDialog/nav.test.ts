import { describe, expect, it } from "vitest";
import {
  CLOUD_NAV,
  filterNavGroups,
  flattenNavGroups,
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
  orgName: "Acme"
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
        // Theirs, whatever their role — which is why this group is first.
        "account.notifications",
        "preferences.appearance",
        "workspace.activity_logs",
        "organization.members",
        "workspace.members",
        // Their own read-only `Reader` credential — see the note in nav.ts.
        "workspace.airhouse",
        "workspace.oltp",
        // Their own personal access tokens; the admin tabs are gated inside.
        "organization.api_tokens"
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

    it("lands on a section every role can open, before anything has loaded", () => {
      // The dialog opens on the first visible item. It used to default to
      // Organization → General, which a member cannot see at all, so a member was
      // opened onto a gated page and bounced to whatever happened to follow it.
      for (const ctx of [
        {},
        { hasOrg: false, hasWorkspace: false },
        { hasWorkspace: false },
        { isOrgAdmin: true, isWorkspaceAdmin: true },
        { isStaff: true }
      ]) {
        expect(sectionsFor(ctx)[0], JSON.stringify(ctx)).toBe("account.notifications");
      }
    });

    it("keeps what is theirs before a workspace loads", () => {
      expect(groupsFor({ hasWorkspace: false })).toEqual(["You", "People", "Developer"]);
      expect(sectionsFor({ hasWorkspace: false })).toEqual([
        "account.notifications",
        "preferences.appearance",
        "organization.members",
        // Personal tokens need no workspace: an org still being set up can mint one.
        "organization.api_tokens"
      ]);
    });

    it("offers their own tokens to anyone signed in, with no org or workspace loaded", () => {
      // A token belongs to its owner, not to a tenant: no role gates it, and it must be
      // reachable from the "you're not in an organization yet" state too.
      expect(sectionsFor({ hasOrg: false, hasWorkspace: false })).toEqual([
        "account.notifications",
        "preferences.appearance",
        "organization.api_tokens"
      ]);
    });

    it("lists API tokens once, in a group that does not need an org", () => {
      // It used to be spliced from Organization into Account whenever no org was loaded,
      // because its home group was dropped with the org. Filed under Developer it simply
      // never moves — the same group, the same position, org or no org.
      for (const ctx of [{}, { hasOrg: false, hasWorkspace: false }]) {
        const holders = visibleNavGroups({ ...CLOUD_BASE, ...ctx }).filter((g) =>
          g.items.some((i) => i.value === "organization.api_tokens")
        );
        expect(
          holders.map((g) => g.label),
          JSON.stringify(ctx)
        ).toEqual(["Developer"]);
      }
    });

    it("keeps Secrets out of an org with no workspace yet, whose routes it could not call", () => {
      expect(
        sectionsFor({ isOrgAdmin: true, isWorkspaceAdmin: true, hasWorkspace: false })
      ).not.toContain("organization.secrets");
    });

    it("has no API tokens and nothing to notify in local mode, which has no sign-in", () => {
      const local = sectionsFor({ isLocalMode: true, hasOrg: false });
      expect(local).not.toContain("organization.api_tokens");
      expect(local).not.toContain("account.notifications");
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
    // really exists — gating workspace-axis items on org role would hide sections
    // the server authorizes.
    it("grants workspace-axis sections to an org member holding workspace admin", () => {
      const sections = sectionsFor({ isWorkspaceAdmin: true });
      expect(sections).toContain("workspace.databases");
      expect(sections).toContain("organization.secrets");
      expect(sections).toContain("workspace.apps");
      expect(sections).toContain("workspace.airhouse");
    });

    it("withholds org-axis sections from that same user", () => {
      const sections = sectionsFor({ isWorkspaceAdmin: true });
      expect(sections).not.toContain("organization.general");
      expect(sections).not.toContain("organization.billing");
      // Oxy staff access is the tenant's kill switch, so it stays on the org axis
      // however its section id is spelled.
      expect(sections).not.toContain("workspace.oxy_access");
    });

    it("gives an ordinary admin every section", () => {
      // The shape nearly every real admin has: org admin derives workspace
      // admin server-side, so both flags arrive true together.
      const sections = sectionsFor({ isOrgAdmin: true, isWorkspaceAdmin: true });
      expect(sections).toEqual([
        "account.notifications",
        "preferences.appearance",
        "workspace.activity_logs",
        "organization.general",
        "organization.billing",
        "workspace.oxy_access",
        "organization.members",
        "workspace.members",
        "organization.teams",
        "organization.app_access",
        "organization.crew",
        "organization.locations",
        "organization.positions",
        "workspace.airhouse",
        "workspace.oltp",
        "workspace.databases",
        "organization.api_tokens",
        "organization.secrets",
        "organization.integration",
        "workspace.connections",
        "workspace.apps"
      ]);
      expect(groupsFor({ isOrgAdmin: true, isWorkspaceAdmin: true })).toEqual([
        "You",
        "Organization",
        "People",
        "Frontline",
        "Data",
        "Developer"
      ]);
    });

    it("grants org-axis sections to an org admin", () => {
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
      expect(sectionsFor({ isOrgAdmin: true })).not.toContain("organization.secrets");
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
    it("shows every section despite carrying no role", () => {
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
      expect(groupsFor({ isLocalMode: true, hasOrg: false })).toEqual(["You", "Data", "Developer"]);
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

    it("shows Previews to Oxy staff in a group of its own, whatever their tenant role", () => {
      expect(sectionsFor({ isStaff: true })).toContain("workspace.previews");
      expect(sectionsFor({ isStaff: true, isOrgAdmin: true })).toContain("workspace.previews");
      // Named as staff's rather than mixed in among the customer's pages.
      expect(groupsFor({ isStaff: true })).toContain("Oxy staff");
      expect(groupsFor({})).not.toContain("Oxy staff");
    });

    it("keeps Previews out of an org with no workspace loaded", () => {
      expect(sectionsFor({ isStaff: true, hasWorkspace: false })).not.toContain(
        "workspace.previews"
      );
    });
  });

  describe("unloaded context", () => {
    it("drops the org-scoped items before the org resolves", () => {
      const sections = sectionsFor({ hasOrg: false });
      expect(sections).not.toContain("organization.members");
      expect(sections).not.toContain("organization.general");
      expect(groupsFor({ hasOrg: false })).not.toContain("Organization");
    });

    it("drops the workspace-scoped items before the workspace resolves", () => {
      const sections = sectionsFor({ isOrgAdmin: true, hasWorkspace: false });
      expect(sections).not.toContain("workspace.airhouse");
      expect(sections).not.toContain("workspace.activity_logs");
      expect(groupsFor({ isOrgAdmin: true, hasWorkspace: false })).not.toContain("Data");
    });
  });
});

describe("the taxonomy itself", () => {
  const cloudItems = flattenNavGroups(
    visibleNavGroups({ ...CLOUD_BASE, isOrgAdmin: true, isWorkspaceAdmin: true, isStaff: true })
  );

  it("names no group after the workspace", () => {
    // An org has exactly one workspace and no screen names it, so a group heading could
    // not tell a reader which of two groups held a thing. Twelve items sat under
    // "Organization" and ten under "Workspace", split by which table the server keys the
    // route by — and Secrets and Oxy access were each filed on the wrong side of it.
    const groups = [...CLOUD_NAV, ...LOCAL_NAV].map((g) => g.label);
    expect(groups).not.toContain("Workspace");
  });

  it("gives every item a label of its own", () => {
    // Two items were both called "Members", for the same people, told apart only by a
    // small-caps group heading above them.
    const labels = cloudItems.map((i) => i.label);
    expect(new Set(labels).size, labels.join(", ")).toBe(labels.length);
  });

  it("gives every item a description, because the shell renders one for every section", () => {
    // The shell draws the header from these. A missing one is a section that opens with a
    // bare title — which is how General, Billing and Integration shipped with no heading.
    for (const group of [...CLOUD_NAV, ...LOCAL_NAV]) {
      for (const item of group.items) {
        expect(item.description, `${group.label} → ${item.label}`).toBeTruthy();
      }
    }
  });

  it("keeps descriptions to one short sentence", () => {
    for (const group of [...CLOUD_NAV, ...LOCAL_NAV]) {
      for (const item of group.items) {
        const words = (item.description ?? "").split(/\s+/).length;
        expect(words, `${item.label}: ${words} words`).toBeLessThanOrEqual(20);
      }
    }
  });

  it("writes every label in sentence case", () => {
    // "Activity Logs" and "OLTP Database" used to sit beside "App access" and "API tokens".
    // Proper nouns and acronyms are the exceptions, so only words after the first are
    // checked, and a word that is all-caps or a known name is allowed.
    const PROPER = new Set(["Oxygen", "Airhouse", "GitHub", "Slack", "Oxy", "Nexus"]);
    for (const item of cloudItems) {
      for (const word of item.label.split(" ").slice(1)) {
        if (PROPER.has(word) || word === word.toUpperCase()) continue;
        expect(word, `${item.label}`).toBe(word.toLowerCase());
      }
    }
  });
});

describe("filterNavGroups", () => {
  const groups = visibleNavGroups({
    ...CLOUD_BASE,
    isOrgAdmin: true,
    isWorkspaceAdmin: true
  });
  const found = (q: string): string[] =>
    flattenNavGroups(filterNavGroups(groups, q)).map((i) => i.value);

  it("returns everything for an empty query", () => {
    expect(filterNavGroups(groups, "")).toEqual(groups);
    expect(filterNavGroups(groups, "   ")).toEqual(groups);
  });

  it("finds a page by a word that is nowhere on its label", () => {
    // The point of the filter: these are the words people actually search for, and none
    // of them is visible until you have already found the page.
    expect(found("quiet hours")).toEqual(["account.notifications"]);
    expect(found("rotate")).toEqual(["organization.api_tokens"]);
    expect(found("dark mode")).toEqual(["preferences.appearance"]);
    expect(found("snowflake")).toEqual(["workspace.databases"]);
    expect(found("kiosk")).toEqual(["organization.crew"]);
    expect(found("invoice")).toEqual(["organization.billing"]);
  });

  it("matches the label and the group heading too", () => {
    expect(found("billing")).toContain("organization.billing");
    expect(found("frontline")).toEqual([
      "organization.crew",
      "organization.locations",
      "organization.positions"
    ]);
  });

  it("narrows on every word rather than widening", () => {
    const token = found("token");
    expect(token.length).toBeGreaterThan(1);
    expect(found("token github")).toEqual(["organization.api_tokens"]);
  });

  it("ignores case and drops groups left with nothing", () => {
    expect(found("QUIET HOURS")).toEqual(["account.notifications"]);
    expect(filterNavGroups(groups, "zzzz")).toEqual([]);
  });
});

describe("sections that write the workspace's config are Nexus's", () => {
  const NEXUS_SECTIONS = ["workspace.databases", "organization.secrets", "workspace.connections"];

  /// A workspace admin of the customer's org the server will not let into
  /// Nexus would open each of these and meet a refusal; they are not offered.
  it("are hidden from a workspace admin the server does not let into Nexus", () => {
    const sections = sectionsFor({
      isOrgAdmin: true,
      isWorkspaceAdmin: true,
      canUseFactory: false
    });
    for (const hidden of NEXUS_SECTIONS) expect(sections).not.toContain(hidden);
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
      for (const shown of NEXUS_SECTIONS) expect(sections).toContain(shown);
    }
  });

  it("are not hidden in local mode, where there is no Nexus to gate", () => {
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

  it("kept every section id the regroup touched, so no link needed a new entry", () => {
    // The nav was regrouped and relabelled, not renumbered. Every id a link can name is
    // still in the nav under its old spelling — that is what made the taxonomy change
    // safe to ship without a redirect for each moved page.
    for (const id of [
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
      "workspace.previews",
      "workspace.airhouse",
      "workspace.oltp",
      "workspace.connections",
      "workspace.apps",
      "workspace.activity_logs",
      "workspace.oxy_access",
      "account.notifications",
      "preferences.appearance"
    ]) {
      expect(everySection(), id).toContain(id);
    }
  });
});
