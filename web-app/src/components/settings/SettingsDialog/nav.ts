import {
  Activity,
  AppWindow,
  BriefcaseBusiness,
  CreditCard,
  Database,
  Eye,
  GitBranch,
  HardHat,
  Key,
  KeyRound,
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
import { FEATURES } from "@/libs/features";
import type { SettingsSection } from "@/stores/useSettingsDialog";

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

export interface NavItem {
  value: SettingsSection;
  label: string;
  icon: NavIcon;
  /** Omit to show the item to every member of the org/workspace. */
  requires?: NavGate;
  featureFlag?: keyof typeof FEATURES;
  /** When set, only render this item if the matching authConfig flag is true. */
  requiresBilling?: boolean;
}

export interface NavGroup {
  label: string;
  items: NavItem[];
}

export interface VisibleNavGroup extends NavGroup {
  subtitle?: string;
}

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
        requires: "workspaceAdmin"
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
      {
        value: "workspace.repositories",
        label: "Repositories",
        icon: GitBranch,
        featureFlag: "LINKED_REPOS",
        requires: "workspaceAdmin"
      },
      // Oxy staff only — previews are a staff tool, never the customer's, so a
      // tenant admin does not see this either. See `useCanUsePreviews`.
      { value: "workspace.previews", label: "Previews", icon: Eye, requires: "staff" },
      { value: "workspace.api_keys", label: "API Keys", icon: Key, requires: "workspaceAdmin" },
      { value: "workspace.secrets", label: "Secrets", icon: KeyRound, requires: "workspaceAdmin" },
      {
        // "Connections", not "Integrations" — the org-level section above is
        // already called Integration (GitHub + Slack) and two near-identical
        // labels in one dialog is a coin flip for the user. This one is
        // third-party API credentials a workspace's apps read via ctx.env.
        value: "workspace.connections",
        label: "Connections",
        icon: Plug,
        requires: "workspaceAdmin"
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
      { value: "workspace.api_keys", label: "API Keys", icon: Key },
      { value: "workspace.secrets", label: "Secrets", icon: KeyRound },
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

export interface NavVisibilityContext {
  isLocalMode: boolean;
  /** `useRole().is.orgAdmin` — org owner or admin. */
  isOrgAdmin: boolean;
  /** `useRole().is.workspaceAdmin` — resolved workspace owner or admin. */
  isWorkspaceAdmin: boolean;
  /** `useCanUsePreviews()` — Oxy platform staff. Absent reads as not staff. */
  isStaff?: boolean;
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

  return (ctx.isLocalMode ? LOCAL_NAV : CLOUD_NAV)
    .filter((g) => groupAvailable(g.label))
    .map((g) => ({
      ...g,
      subtitle: groupSubtitle(g.label),
      items: g.items.filter(
        (i) =>
          (!i.featureFlag || FEATURES[i.featureFlag]) &&
          gateSatisfied(i.requires, ctx) &&
          (!i.requiresBilling || ctx.billingEnabled)
      )
    }))
    .filter((g) => g.items.length > 0);
}
