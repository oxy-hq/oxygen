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
  Share2,
  ShieldCheck,
  SlidersHorizontal,
  SunMoon,
  Table2,
  Users,
  UsersRound
} from "lucide-react";
import { AirhouseLogo } from "@/components/icons";
import type { SettingsSection } from "@/components/settings/settingsRoute";
import { API_TOKENS_TABS, SECRETS_TABS } from "./tabs";

type NavIcon = LucideIcon | React.ComponentType<{ className?: string }>;

/**
 * Which authority a nav item needs. The two are independent, and conflating
 * them is what this gate exists to prevent: the backend resolves workspace
 * access as `max(org_derived_role, workspace_member_override)`
 * (`workspace_context.rs::resolve_effective_role`), so an org **Member** can
 * hold workspace **Admin** through an explicit `workspace_members` row.
 *
 * Gating an org-scoped item on workspace role would therefore hide it from
 * someone the server authorizes, and the reverse would show it to someone the
 * server will 403. Name the axis per item and the nav matches what the API will
 * actually allow.
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
  /**
   * One sentence: what this page is for. The shell renders it under the title for every
   * section, so a section never has to draw its own header and none can be missing one —
   * General, Billing and Integration each shipped with no heading at all.
   *
   * House style: sentence case, 20 words or fewer, no file names and no code identifiers.
   */
  description?: string;
  /**
   * Extra words the nav filter matches, for what the label does not say. "Quiet hours" lives
   * inside Notifications and "rotate" inside API tokens, and neither word is on screen until
   * you have already found the page.
   */
  keywords?: readonly string[];
  /** Omit to show the item to every member of the org/workspace. */
  requires?: NavGate;
  /** When set, only render this item if the matching authConfig flag is true. */
  requiresBilling?: boolean;
  /**
   * The section writes the workspace's config (databases, linked repositories,
   * secrets, connections), which is the Oxygen Nexus's: its routes refuse a
   * caller Nexus is not open to. Independent of `requires` — a workspace
   * admin who may not use Nexus is not shown it. See `canUseFactory`.
   */
  requiresFactory?: boolean;
  /**
   * The section reads or writes the org itself, so it is hidden when no org is loaded —
   * staff and app operators outside any org, or someone holding only pending invites.
   */
  needsOrg?: boolean;
  /**
   * The section reads data the server keys by the org's workspace (its routes take a
   * workspace id), so it is hidden while the org has none — an org still being set up.
   * Each org has exactly one workspace and no screen names it, which is why no nav group
   * is called "Workspace" any more.
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
 * The settings nav, grouped by what the user came to do.
 *
 * **It used to be grouped by which server axis authorises the route** — an "Organization"
 * group of 12 beside a "Workspace" group of 10 — which asked the reader to know the storage
 * model to predict where a thing lived. The split had already failed twice on its own terms:
 * API tokens and Secrets were both moved across it (`RENAMED_SECTIONS`), Secrets sat under
 * Organization while gated on workspace role, and Oxy access sat under Workspace with a
 * comment saying it is org-level on purpose. An org has exactly one workspace and no screen
 * names it, so the boundary named nothing the user could see.
 *
 * The gates are unchanged and still per item — only the filing and the labels moved. **No
 * section id changed**, so every `?settings=` link people hold still lands where it did.
 *
 * Ungated items are the ones a plain Member can actually use: the org and workspace rosters
 * are readable by any member server-side, and My activity is filtered to the caller's own
 * history (`thread.rs::get_logs` filters on `user_id`). Everything else needs an admin ring,
 * so it stays out of the nav rather than rendering a panel the API will refuse to fill.
 */
export const CLOUD_NAV: NavGroup[] = [
  // First on purpose: it needs no org, no workspace and no role, so every viewer — including
  // a Member, and someone who holds nothing but a pending invite — lands on a page that is
  // theirs and works. The old first item was Organization → General, which a Member cannot
  // see at all, so they were silently bounced to whatever happened to be next.
  {
    label: "You",
    items: [
      {
        value: "account.notifications",
        label: "Notifications",
        icon: Bell,
        description: "How and when Oxygen tells you about things, on every device you use.",
        keywords: ["quiet hours", "email", "push", "slack", "mute", "pause", "devices", "digest"]
      },
      {
        value: "preferences.appearance",
        label: "Appearance",
        icon: SunMoon,
        description: "Light or dark, carried across Oxygen and every app you open.",
        keywords: ["theme", "dark mode", "light mode", "colour", "color", "system"]
      },
      {
        value: "workspace.activity_logs",
        label: "My activity",
        icon: Activity,
        description: "Everything you have run here, newest first.",
        keywords: ["history", "audit", "logs", "queries", "runs"],
        needsWorkspace: true
      }
    ]
  },
  {
    label: "Organization",
    items: [
      {
        value: "organization.general",
        label: "General",
        icon: SettingsIcon,
        description: "Your organization's name, logo and web address.",
        keywords: ["name", "logo", "slug", "url", "rename", "delete", "danger zone"],
        requires: "orgAdmin",
        needsOrg: true
      },
      {
        value: "organization.billing",
        label: "Billing",
        icon: CreditCard,
        description: "Your plan, payment method and past invoices.",
        keywords: ["plan", "invoice", "card", "payment", "subscription", "seats", "upgrade"],
        requires: "orgAdmin",
        requiresBilling: true,
        needsOrg: true
      },
      {
        // Org-level on purpose: this is the tenant's kill switch for Oxy staff
        // access, and the server only lets a real org owner/admin flip it. It sat in
        // the "Workspace" group with a comment apologising for it.
        value: "workspace.oxy_access",
        label: "Oxy staff access",
        icon: ShieldCheck,
        description: "Whether Oxy staff may enter your organization to help, and when.",
        keywords: ["support", "assume", "impersonate", "oxy", "staff", "kill switch"],
        requires: "orgAdmin",
        needsOrg: true
      }
    ]
  },
  {
    label: "People",
    // Members, then the override, then the groups, then what those groups reach: the order an
    // admin actually works in.
    items: [
      {
        value: "organization.members",
        label: "Members",
        icon: Users,
        description: "Who belongs to this organization, and the role each one holds.",
        keywords: ["invite", "role", "owner", "admin", "remove", "people", "users", "seats"],
        needsOrg: true
      },
      {
        // Was a second item called "Members", in a second group, for the same people. Named
        // for what it does instead: an org role can be raised or lowered for data access
        // without changing the person's standing in the org.
        value: "workspace.members",
        label: "Role overrides",
        icon: SlidersHorizontal,
        description: "Give someone more or less data access than their organization role.",
        keywords: ["override", "workspace role", "elevate", "restrict", "access", "members"],
        needsWorkspace: true
      },
      {
        value: "organization.teams",
        label: "Teams",
        icon: UsersRound,
        description: "Groups of people you can grant apps to in one go.",
        keywords: ["group", "squad", "department"],
        requires: "orgAdmin",
        needsOrg: true
      },
      {
        value: "organization.app_access",
        label: "App access",
        icon: ShieldCheck,
        description: "Which people and teams can open each of your apps.",
        keywords: ["grant", "permission", "share", "apps", "restrict", "custom app"],
        requires: "orgAdmin",
        needsOrg: true
      }
    ]
  },
  {
    // Crew are a different population from members: a PIN and a kiosk, no email and no
    // organization membership. Locations and Positions are the graph they work in — the
    // places work happens and the positions people hold there.
    label: "Frontline",
    items: [
      {
        value: "organization.crew",
        label: "Crew",
        icon: HardHat,
        description: "Frontline workers who sign in with a PIN, and the kiosks they use.",
        keywords: ["worker", "pin", "kiosk", "tablet", "enroll", "shift", "staff"],
        requires: "orgAdmin",
        needsOrg: true
      },
      {
        value: "organization.locations",
        label: "Locations",
        icon: MapPin,
        description: "The places your organization operates, and who can reach each one.",
        keywords: ["site", "store", "branch", "venue", "timezone", "address", "reach"],
        requires: "orgAdmin",
        needsOrg: true
      },
      {
        value: "organization.positions",
        label: "Positions",
        icon: BriefcaseBusiness,
        description: "The jobs people hold at your locations.",
        keywords: ["role", "job", "title", "assignment"],
        requires: "orgAdmin",
        needsOrg: true
      }
    ]
  },
  {
    // Named by what each store is for, not by the engine behind it. These were "Databases",
    // "Airhouse" and "OLTP Database" — two of them sharing one icon, and one of them leading
    // with an acronym from the implementation.
    label: "Data",
    items: [
      {
        // Ungated on purpose. `get_connection` and `get_credentials` accept any
        // org member, and the credential minted is per-user, TTL-bounded and
        // role-scoped — `airhouse_role_for` maps Member to `UserRole::Reader`
        // (broker.rs). Read-only warehouse access for members is a deliberate
        // product decision, and this panel is their only route to the connection
        // string. Only `provision` and the catalog-index routes need an admin,
        // and those are gated inside the section.
        value: "workspace.airhouse",
        label: "Airhouse",
        icon: AirhouseLogo,
        description: "Your warehouse: how to connect to it, and what it holds.",
        keywords: ["warehouse", "connection string", "credentials", "catalog", "duckdb", "sql"],
        needsWorkspace: true
      },
      {
        // Ungated for the same reason as Airhouse, but for a stronger one: this
        // panel returns no credentials at all. `GET /oltp/me/connection` reports
        // which schemas exist and whether analytics can see them — metadata, not
        // access. Queries reach the database only through `postgres_managed`,
        // which resolves the read-only analyst server-side.
        value: "workspace.oltp",
        label: "App database",
        icon: Database,
        description: "Where your apps keep the records people edit.",
        keywords: ["oltp", "postgres", "transactional", "schema", "tables", "records"],
        needsWorkspace: true
      },
      {
        // A connection declared in the workspace repository, read-only here.
        value: "workspace.databases",
        label: "Warehouse connections",
        icon: Table2,
        description: "The outside warehouses this organization reads from.",
        keywords: ["snowflake", "bigquery", "duckdb", "clickhouse", "postgres", "source", "query"],
        requires: "workspaceAdmin",
        requiresFactory: true,
        needsWorkspace: true
      }
    ]
  },
  {
    // Everything someone wiring Oxygen up to something else needs. Three of these were
    // scattered across two groups under three labels that all meant "an outside service is
    // linked" — Integration, Connections and Apps.
    label: "Developer",
    items: [
      {
        // Every token that lets something call Oxygen, in one place: the viewer's own
        // personal access tokens, and for org admins the service accounts, every token
        // reaching the org, and the token policy. Ungated because a personal token is
        // always its owner's to manage; the admin tabs are gated inside the section.
        //
        // No `needsOrg`: a personal access token spans orgs and needs none, so a signed-in
        // person with no current org still reaches their own. That used to be done by
        // splicing this item into the Account group, which is why it is filed in a group
        // that does not depend on an org.
        value: "organization.api_tokens",
        label: "API tokens",
        icon: KeySquare,
        description: "Tokens that let a script, a CI job or an agent call Oxygen as you.",
        keywords: [
          "pat",
          "personal access token",
          "service account",
          "rotate",
          "revoke",
          "expire",
          "oidc",
          "ci",
          "github actions",
          "api key"
        ],
        tabs: API_TOKENS_TABS
      },
      {
        // Values Oxygen keeps for the org's apps, agents and databases. They are
        // stored under the org's one workspace and its routes check the workspace
        // role and the Nexus grant, so the item carries those gates rather than
        // `orgAdmin` — name the axis the server enforces (see `NavGate`).
        value: "organization.secrets",
        label: "Secrets",
        icon: KeyRound,
        description: "Values your apps and agents read at run time, stored encrypted.",
        keywords: ["env", "environment variable", "api key", "credential", "password", "token"],
        requires: "workspaceAdmin",
        requiresFactory: true,
        needsWorkspace: true,
        tabs: SECRETS_TABS
      },
      {
        value: "organization.integration",
        label: "GitHub and Slack",
        icon: Share2,
        description: "The repository Oxygen builds from, and where it posts.",
        keywords: ["github", "slack", "repository", "repo", "git", "notify", "channel", "connect"],
        requires: "orgAdmin",
        needsOrg: true
      },
      {
        // "API credentials", not "Connections" and not "Integrations": the group already
        // holds GitHub and Slack, and two near-identical labels in one dialog is a coin
        // flip for the reader. This one is third-party APIs an app's functions call.
        value: "workspace.connections",
        label: "API credentials",
        icon: Plug,
        description: "Third-party APIs your app functions are allowed to call.",
        keywords: ["oauth", "connect", "quickbooks", "provider", "authorize", "connections"],
        requires: "workspaceAdmin",
        requiresFactory: true,
        needsWorkspace: true
      },
      {
        // Was "Apps", in a product whose headline object is the custom app, sitting two
        // rows from "App access" which is about those. It is neither — it is the outside
        // services that feed data in.
        value: "workspace.apps",
        label: "Data sources",
        icon: AppWindow,
        description: "Outside services that feed data into your dashboards.",
        keywords: ["toast", "unifi", "weather", "besttime", "feed", "ingest", "apps", "source"],
        requires: "workspaceAdmin",
        needsWorkspace: true
      }
    ]
  },
  {
    // Oxy staff only — previews are a staff tool, never the customer's, so a
    // tenant admin does not see this either. See `useCanUsePreviews`. Its own group
    // rather than hidden among the customer's pages, so staff can see at a glance
    // which rows are theirs and not the tenant's.
    label: "Oxy staff",
    items: [
      {
        value: "workspace.previews",
        label: "Previews",
        icon: Eye,
        description: "Pinned workspace previews, for checking a change before it lands.",
        keywords: ["preview", "pin", "revision", "branch", "staff"],
        requires: "staff",
        needsWorkspace: true
      }
    ]
  }
];

export const LOCAL_NAV: NavGroup[] = [
  {
    label: "You",
    items: [
      {
        value: "preferences.appearance",
        label: "Appearance",
        icon: SunMoon,
        description: "Light or dark, for this browser.",
        keywords: ["theme", "dark mode", "light mode", "colour", "color", "system"]
      },
      {
        value: "workspace.activity_logs",
        label: "My activity",
        icon: Activity,
        description: "Everything you have run here, newest first.",
        keywords: ["history", "audit", "logs", "queries", "runs"]
      }
    ]
  },
  {
    label: "Data",
    items: [
      {
        value: "workspace.airhouse",
        label: "Airhouse",
        icon: AirhouseLogo,
        description: "Your warehouse: how to connect to it, and what it holds.",
        keywords: ["warehouse", "connection string", "credentials", "catalog", "duckdb", "sql"]
      },
      {
        value: "workspace.oltp",
        label: "App database",
        icon: Database,
        description: "Where your apps keep the records people edit.",
        keywords: ["oltp", "postgres", "transactional", "schema", "tables", "records"]
      },
      {
        value: "workspace.databases",
        label: "Warehouse connections",
        icon: Table2,
        description: "The outside warehouses this workspace reads from.",
        keywords: ["snowflake", "bigquery", "duckdb", "clickhouse", "postgres", "source", "query"]
      }
    ]
  },
  {
    label: "Developer",
    items: [
      // Local mode has no sign-in, so no tokens to list or mint: API tokens is cloud only.
      // Secrets is the same section the cloud nav files here too; local mode has no org.
      {
        value: "organization.secrets",
        label: "Secrets",
        icon: KeyRound,
        description: "Values your apps and agents read at run time, stored encrypted.",
        keywords: ["env", "environment variable", "api key", "credential", "password", "token"],
        tabs: SECRETS_TABS
      },
      {
        value: "workspace.connections",
        label: "API credentials",
        icon: Plug,
        description: "Third-party APIs your app functions are allowed to call.",
        keywords: ["oauth", "connect", "quickbooks", "provider", "authorize", "connections"]
      },
      {
        value: "workspace.apps",
        label: "Data sources",
        icon: AppWindow,
        description: "Outside services that feed data into your dashboards.",
        keywords: ["toast", "unifi", "weather", "besttime", "feed", "ingest", "apps", "source"]
      }
    ]
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
 * Nothing was added when the nav was regrouped and relabelled, because **no section id
 * changed**: the ids below are the only ones that ever moved, and the pages they name are
 * still reached by the same id under a new label and a new group heading.
 *
 * - `account.tokens`: Account → Personal access tokens, now a tab of API tokens.
 * - `organization.api_access`: Organization → API access, renamed API tokens. Its first tab
 *   was Service accounts, so that is where the old link lands.
 * - `workspace.api_keys`: Workspace → API tokens, a per-workspace list of tokens. Each org has
 *   one workspace, so the org's list is the same list.
 * - `workspace.secrets`: Workspace → Secrets, now filed under Developer.
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
   * `canUseFactory(workspace)` — whether the server lets the caller into
   * Oxygen Nexus here. Absent reads as allowed, like a response that predates
   * the field.
   */
  canUseFactory?: boolean;
  billingEnabled: boolean;
  hasOrg: boolean;
  hasWorkspace: boolean;
  orgName?: string;
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
 * Every condition is per item, so a group is simply whichever of its items survived;
 * groups left with none are dropped, and a viewer never sees an empty heading. The
 * previous version also filtered whole groups by their label, which is what made
 * "Organization" and "Workspace" load-bearing strings.
 */
export function visibleNavGroups(ctx: NavVisibilityContext): VisibleNavGroup[] {
  return (ctx.isLocalMode ? LOCAL_NAV : CLOUD_NAV)
    .map((g) => ({
      ...g,
      subtitle: g.label === "Organization" ? ctx.orgName : undefined,
      items: g.items.filter(
        (i) =>
          gateSatisfied(i.requires, ctx) &&
          (!i.requiresFactory || ctx.isLocalMode || ctx.canUseFactory !== false) &&
          (!i.needsOrg || ctx.hasOrg) &&
          (!i.needsWorkspace || ctx.hasWorkspace) &&
          (!i.requiresBilling || ctx.billingEnabled)
      )
    }))
    .filter((g) => g.items.length > 0);
}

/** One nav item, flattened out of its group, for the filter and the active-section lookup. */
export interface FlatNavItem extends NavItem {
  group: string;
}

export function flattenNavGroups(groups: VisibleNavGroup[]): FlatNavItem[] {
  return groups.flatMap((g) => g.items.map((i) => ({ ...i, group: g.label })));
}

/**
 * The groups that match a filter query, keeping group order and dropping groups with no match.
 *
 * Matches the label, the group heading, the description and the item's `keywords`, so typing
 * "quiet hours" finds Notifications and "rotate" finds API tokens — neither word appears on
 * screen until you have already found the page. An empty query returns the groups unchanged.
 */
export function filterNavGroups(groups: VisibleNavGroup[], query: string): VisibleNavGroup[] {
  const q = query.trim().toLowerCase();
  if (!q) return groups;
  // Every word has to match something, so "token rotate" narrows rather than widens.
  const words = q.split(/\s+/);
  return groups
    .map((g) => ({
      ...g,
      items: g.items.filter((i) => {
        const haystack = [i.label, g.label, i.description ?? "", ...(i.keywords ?? [])]
          .join(" ")
          .toLowerCase();
        return words.every((w) => haystack.includes(w));
      })
    }))
    .filter((g) => g.items.length > 0);
}

/**
 * One section's nav entry, from the definitions rather than from what the viewer can
 * currently see. The page uses it for the title and description while the org, role and
 * workspace are still arriving — the visible nav is short on the first render of a cold load,
 * and a section is not nameless just because its gate has not resolved.
 */
export function navItemFor(section: string): NavItem | undefined {
  for (const group of [...CLOUD_NAV, ...LOCAL_NAV]) {
    const found = group.items.find((i) => i.value === section);
    if (found) return found;
  }
  return undefined;
}
