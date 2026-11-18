import {
  Activity,
  AppWindow,
  Bell,
  BriefcaseBusiness,
  CreditCard,
  Database,
  Eye,
  HardHat,
  KeyRound,
  KeySquare,
  type LucideIcon,
  MapPin,
  Plug,
  Settings as SettingsIcon,
  ShieldCheck,
  SunMoon,
  Users,
  UsersRound
} from "lucide-react";
import { AirhouseLogo } from "@/components/icons";
import type { SettingsSection } from "@/stores/useSettingsDialog";
import { API_TOKENS_TABS, SECRETS_TABS } from "./tabs";

type NavIcon = LucideIcon | React.ComponentType<{ className?: string }>;

/**
 * Which authority a nav item needs. The two are independent, and conflating
 * them is what this gate exists to prevent: the backend resolves workspace
 * access as `max(org_derived_role, workspace_member_override)`
 * (`workspace_context.rs::resolve_effective_role`), so an org **Member** can
 * hold workspace **Admin** through an explicit `workspace_members` row.
 *
 * Gating a Workspace item on org role would therefore hide it from someone the
 * server authorizes; gating an Organization item on workspace role would show
 * it to someone the server will 403. Name the axis per item and the nav matches
 * what the API will actually allow.
 *
 * `staff` is a third, unrelated axis: Oxy platform staff (`is_owner ||
 * is_app_admin`), for tools that are never the customer's whatever their role
 * in the tenant — an org owner does not clear it.
 */
export type NavGate = "orgAdmin" | "workspaceAdmin" | "staff";

interface NavItem {
  value: SettingsSection;
  label: string;
  icon: NavIcon;
  /** Omit to show the item to every member of the org/workspace. */
  requires?: NavGate;
  /** When set, only render this item if the matching authConfig flag is true. */
  requiresBilling?: boolean;
  /**
   * The section writes the workspace's config (databases, linked repositories,
   * secrets, connections), which is the Oxygen Factory's: its routes refuse a
   * caller the Factory is not open to. Independent of `requires` — a workspace
   * admin who may not use the Factory is not shown it. See `canUseFactory`.
   */
  requiresFactory?: boolean;
  /**
   * The section reads data the server still keys by the org's workspace (its routes take a
   * workspace id), so it is hidden while the org has none — an org still being set up. The item
   * can sit in the Organization group all the same: each org has exactly one workspace, and no
   * screen names it.
   */
  needsWorkspace?: boolean;
  /**
   * The section's tabs, which a link can open directly as `<section>.<tab>`. The section itself
   * decides which of them the viewer may see.
   */
  tabs?: readonly string[];
}

export interface NavGroup {
  label: string;
  items: NavItem[];
}

export interface VisibleNavGroup extends NavGroup {
  subtitle?: string;
}

/**
 * Organization → API tokens. Shared by two places in the nav: the Organization group, where it
 * lives, and the Account group when no org is loaded (`visibleNavGroups`). A personal access
 * token spans orgs and needs none, so a signed-in person with no current org — someone with only
 * pending invites, or staff and app operators outside any org — still reaches their own tokens.
 * On that page they see only the Personal access tokens tab.
 */
const API_TOKENS_ITEM: NavItem = {
  value: "organization.api_tokens",
  label: "API tokens",
  icon: KeySquare,
  tabs: API_TOKENS_TABS
};

// Ungated items are the ones a plain Member can actually use: the org and
// workspace rosters are readable by any member server-side, and Activity Logs
// is filtered to the caller's own history (`thread.rs::get_logs` filters on
// `user_id`). Everything else needs an admin ring, so it stays out of the nav
// rather than rendering a panel the API will refuse to fill.
export const CLOUD_NAV: NavGroup[] = [
  {
    label: "Organization",
    items: [
      {
        value: "organization.general",
        label: "General",
        icon: SettingsIcon,
        requires: "orgAdmin"
      },
      { value: "organization.members", label: "Members", icon: Users },
      // Teams then App access, in the order an admin works: group people, then
      // decide what those groups can reach.
      { value: "organization.teams", label: "Teams", icon: UsersRound, requires: "orgAdmin" },
      {
        value: "organization.app_access",
        label: "App access",
        icon: ShieldCheck,
        requires: "orgAdmin"
      },
      // Crew are the org's frontline workers (kiosk + PIN, no email, no
      // membership) and the tablets they sign in on. Unlike the Members roster,
      // which any member may read, every crew route is org-admin only — so this
      // carries the gate the nav comment above promises: no panel the API will
      // refuse to fill.
      { value: "organization.crew", label: "Crew", icon: HardHat, requires: "orgAdmin" },
      // Locations and Positions are the operating graph: the places work
      // happens and the positions people hold there. The same authority as
      // Crew — the shape of the org — so the same gate.
      { value: "organization.locations", label: "Locations", icon: MapPin, requires: "orgAdmin" },
      {
        value: "organization.positions",
        label: "Positions",
        icon: BriefcaseBusiness,
        requires: "orgAdmin"
      },
      // Every token that lets something call Oxygen, in one place: the viewer's own
      // personal access tokens, and for org admins the service accounts, every token
      // reaching the org, and the token policy. Ungated because a personal token is
      // always its owner's to manage; the admin tabs are gated inside the section,
      // by the same org role their routes check.
      API_TOKENS_ITEM,
      // Values Oxygen keeps for the org's apps, agents and databases. They are
      // stored under the org's one workspace and its routes check the workspace
      // role and the Factory grant, so the item carries those gates rather than
      // `orgAdmin` — name the axis the server enforces (see `NavGate`).
      {
        value: "organization.secrets",
        label: "Secrets",
        icon: KeyRound,
        requires: "workspaceAdmin",
        requiresFactory: true,
        needsWorkspace: true,
        tabs: SECRETS_TABS
      },
      {
        value: "organization.billing",
        label: "Billing",
        icon: CreditCard,
        requires: "orgAdmin",
        requiresBilling: true
      },
      {
        value: "organization.integration",
        label: "Integration",
        icon: Plug,
        requires: "orgAdmin"
      }
    ]
  },
  {
    label: "Workspace",
    items: [
      { value: "workspace.members", label: "Members", icon: Users },
      {
        value: "workspace.databases",
        label: "Databases",
        icon: Database,
        requires: "workspaceAdmin",
        requiresFactory: true
      },
      // Ungated on purpose. `get_connection` and `get_credentials` accept any
      // org member, and the credential minted is per-user, TTL-bounded and
      // role-scoped — `airhouse_role_for` maps Member to `UserRole::Reader`
      // (broker.rs). Read-only warehouse access for members is a deliberate
      // product decision, and this panel is their only route to the connection
      // string. Only `provision` and the catalog-index routes need an admin,
      // and those are gated inside the section.
      { value: "workspace.airhouse", label: "Airhouse", icon: AirhouseLogo },
      // Ungated for the same reason as Airhouse, but for a stronger one: this
      // panel returns no credentials at all. `GET /oltp/me/connection` reports
      // which schemas exist and whether analytics can see them — metadata, not
      // access. Queries reach the database only through `postgres_managed`,
      // which resolves the read-only analyst server-side.
      { value: "workspace.oltp", label: "OLTP Database", icon: Database },
      // Oxy staff only — previews are a staff tool, never the customer's, so a
      // tenant admin does not see this either. See `useCanUsePreviews`.
      { value: "workspace.previews", label: "Previews", icon: Eye, requires: "staff" },
      // API tokens and Secrets used to be here. Both moved to Organization; the old
      // ids redirect (`RENAMED_SECTIONS`).
      {
        // "Connections", not "Integrations" — the org-level section above is
        // already called Integration (GitHub + Slack) and two near-identical
        // labels in one dialog is a coin flip for the user. This one is
        // third-party API credentials a workspace's apps read via ctx.env.
        value: "workspace.connections",
        label: "Connections",
        icon: Plug,
        requires: "workspaceAdmin",
        requiresFactory: true
      },
      { value: "workspace.apps", label: "Apps", icon: AppWindow, requires: "workspaceAdmin" },
      {
        // Org-level on purpose: this is the tenant's kill switch for Oxy staff
        // access, and the server only lets a real org owner/admin flip it.
        value: "workspace.oxy_access",
        label: "Oxy access",
        icon: ShieldCheck,
        requires: "orgAdmin"
      },
      { value: "workspace.activity_logs", label: "Activity Logs", icon: Activity }
    ]
  },
  // Customer-apps management used to live here. It now has its own
  // top-level surface at `/admin/apps`, gated by `is_app_admin`.
  // The viewer's own settings, across every org: every
  // `/notifications/preferences` route reads and writes the caller's. Personal
  // access tokens used to be here too; they are a tab of Organization → API
  // tokens now, so every token is in one place.
  {
    label: "Account",
    items: [{ value: "account.notifications", label: "Notifications", icon: Bell }]
  },
  {
    label: "Preferences",
    items: [{ value: "preferences.appearance", label: "Appearance", icon: SunMoon }]
  }
];

export const LOCAL_NAV: NavGroup[] = [
  {
    label: "Workspace",
    items: [
      { value: "workspace.databases", label: "Databases", icon: Database },
      { value: "workspace.airhouse", label: "Airhouse", icon: AirhouseLogo },
      { value: "workspace.oltp", label: "OLTP Database", icon: Database },
      // Local mode has no sign-in, so no tokens to list or mint: API tokens is
      // cloud only. Secrets is the same section the cloud nav files under
      // Organization; local mode has no org, so it sits here.
      { value: "organization.secrets", label: "Secrets", icon: KeyRound, tabs: SECRETS_TABS },
      { value: "workspace.connections", label: "Connections", icon: Plug },
      { value: "workspace.apps", label: "Apps", icon: AppWindow },
      { value: "workspace.activity_logs", label: "Activity Logs", icon: Activity }
    ]
  },
  {
    label: "Preferences",
    items: [{ value: "preferences.appearance", label: "Appearance", icon: SunMoon }]
  }
];

/** Where a link opens: a section, and optionally one of its tabs. */
export interface SettingsTarget {
  section: SettingsSection;
  tab?: string;
}

/**
 * Section ids that no longer exist, and where each now lives. These are links people hold — in
 * emails already sent, in published `oxyc` versions, in bookmarks — so an entry is never
 * removed, only added.
 *
 * - `account.tokens`: Account → Personal access tokens, now a tab of API tokens.
 * - `organization.api_access`: Organization → API access, renamed API tokens. Its first tab
 *   was Service accounts, so that is where the old link lands.
 * - `workspace.api_keys`: Workspace → API tokens, a per-workspace list of tokens. Each org has
 *   one workspace, so the org's list is the same list.
 * - `workspace.secrets`: Workspace → Secrets, now Organization → Secrets.
 */
export const RENAMED_SECTIONS: Readonly<Record<string, SettingsTarget>> = {
  "account.tokens": { section: "organization.api_tokens", tab: "personal" },
  "organization.api_access": { section: "organization.api_tokens", tab: "service_accounts" },
  "workspace.api_keys": { section: "organization.api_tokens", tab: "all" },
  "workspace.secrets": { section: "organization.secrets" }
};

export interface NavVisibilityContext {
  isLocalMode: boolean;
  /** `useRole().is.orgAdmin` — org owner or admin. */
  isOrgAdmin: boolean;
  /** `useRole().is.workspaceAdmin` — resolved workspace owner or admin. */
  isWorkspaceAdmin: boolean;
  /** `useCanUsePreviews()` — Oxy platform staff. Absent reads as not staff. */
  isStaff?: boolean;
  /**
   * `canUseFactory(workspace)` — whether the server lets the caller into the
   * Oxygen Factory here. Absent reads as allowed, like a response that predates
   * the field.
   */
  canUseFactory?: boolean;
  billingEnabled: boolean;
  hasOrg: boolean;
  hasWorkspace: boolean;
  orgName?: string;
  workspaceName?: string;
}

/**
 * Whether the caller clears one item's gate.
 *
 * The `isLocalMode` exemption is **defensive, and inert today**: no `LOCAL_NAV`
 * item carries a `requires`, so removing the term would not change any current
 * outcome. It earns its place because local mode has no org and therefore no
 * org role, so the first `orgAdmin` gate added to `LOCAL_NAV` would silently
 * hide that item from the single seeded user the server treats as Owner of
 * everything (`local_context` inserts `EffectiveWorkspaceRole(Owner)`).
 *
 * Exported so that property can be asserted against a gated input directly,
 * rather than through a `LOCAL_NAV` that happens to have nothing to gate.
 */
export function gateSatisfied(gate: NavGate | undefined, ctx: NavVisibilityContext): boolean {
  if (!gate || ctx.isLocalMode) return true;
  if (gate === "staff") return !!ctx.isStaff;
  return gate === "orgAdmin" ? ctx.isOrgAdmin : ctx.isWorkspaceAdmin;
}

/**
 * The settings nav the caller should actually see. Pure so the role→visibility
 * matrix can be asserted directly — it is the whole permission surface of the
 * dialog, and a wrong entry here is invisible until someone with that role
 * opens Settings.
 *
 * Groups with no visible items are dropped, so a plain Member never sees an
 * empty "Workspace" heading.
 */
export function visibleNavGroups(ctx: NavVisibilityContext): VisibleNavGroup[] {
  const groupAvailable = (label: string): boolean => {
    if (label === "Organization") return ctx.hasOrg;
    if (label === "Workspace") return ctx.hasWorkspace;
    return true;
  };

  const groupSubtitle = (label: string): string | undefined => {
    if (label === "Organization") return ctx.orgName;
    if (label === "Workspace") return ctx.workspaceName;
    return undefined;
  };

  // With no org loaded the Organization group is dropped, and API tokens with it; its Personal
  // access tokens tab needs no org, so the item moves to the Account group instead.
  const groupItems = (g: NavGroup): NavItem[] =>
    !ctx.isLocalMode && !ctx.hasOrg && g.label === "Account"
      ? [API_TOKENS_ITEM, ...g.items]
      : g.items;

  return (ctx.isLocalMode ? LOCAL_NAV : CLOUD_NAV)
    .filter((g) => groupAvailable(g.label))
    .map((g) => ({
      ...g,
      subtitle: groupSubtitle(g.label),
      items: groupItems(g).filter(
        (i) =>
          gateSatisfied(i.requires, ctx) &&
          (!i.requiresFactory || ctx.isLocalMode || ctx.canUseFactory !== false) &&
          (!i.needsWorkspace || ctx.hasWorkspace) &&
          (!i.requiresBilling || ctx.billingEnabled)
      )
    }))
    .filter((g) => g.items.length > 0);
}
