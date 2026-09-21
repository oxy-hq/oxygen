/**
 * The admin console's route map, and every question asked of it.
 *
 * Pure on purpose — no JSX, no hooks, no `services/env`. It lived inside the sidebar
 * component, which meant the route guard's unit test had to boot jsdom to reach a function
 * that touches neither the DOM nor the network, and that the layout imported a component
 * file to get a predicate.
 */
import {
  Activity,
  AppWindow,
  Building2,
  Database,
  FileCheck,
  Flag,
  Handshake,
  HeartPulse,
  Inbox,
  ScrollText,
  ShieldCheck,
  Telescope,
  Users,
  Warehouse,
  Waypoints
} from "lucide-react";
import type { ComponentType } from "react";
import ROUTES from "@/libs/utils/routes";
import type { PlatformCapability } from "@/types/auth";

/**
 * The rail's groups, in the order they are rendered. Their labels are here rather than
 * in the sidebar because the topbar breadcrumb names them too.
 */
export const ADMIN_NAV_GROUPS = {
  operations: "Operations",
  tenants: "Tenants"
} as const;

export type AdminNavGroup = keyof typeof ADMIN_NAV_GROUPS;

export type AdminNavItem = {
  to: string;
  label: string;
  icon: ComponentType<{ className?: string }>;
  /** When true, render only for Global Owners (the OXY_OWNER env-var allow-list). */
  ownerOnly?: boolean;
  /**
   * The platform capability this page needs — the same one its router gate names in
   * `crates/app/src/server/api/admin/mod.rs`. Keeping the two in step is what stops the
   * nav from offering a room the server will 403; when they drift, the server wins and
   * the user gets a dead link.
   */
  capability?: PlatformCapability;
  /** Which rail group the item sits under. Labels live in [`ADMIN_NAV_GROUPS`]. */
  group: AdminNavGroup;
};

/**
 * Every admin route and the standing it needs. **The one map** — `AdminLayout`'s
 * route guard reads it too, via [`canReachAdminRoute`].
 *
 * It was not the one map: the layout carried its own hardcoded list of path prefixes a
 * non-owner could reach, written before capabilities existed. Adding a capability to a
 * nav item made it appear and then bounce, because the two lists disagreed — which is
 * how `Staff access` shipped visible and unreachable for every Global Admin.
 */
export const ADMIN_NAV: AdminNavItem[] = [
  // Billing queue is strict Global Owner — "billing adjustment" per the
  // server-side route_layer in admin/mod.rs (OXY_OWNER env-var allow-list).
  {
    to: ROUTES.ADMIN.BILLING_QUEUE,
    label: "Billing queue",
    icon: Inbox,
    ownerOnly: true,
    group: "operations"
  },
  {
    to: ROUTES.ADMIN.FEATURE_FLAGS,
    label: "Feature flags",
    icon: Flag,
    capability: "operate_platform",
    group: "operations"
  },
  {
    to: ROUTES.ADMIN.INTERNAL_JOBS,
    label: "Internal jobs",
    icon: Activity,
    capability: "operate_platform",
    group: "operations"
  },
  {
    to: ROUTES.ADMIN.COMPILES,
    label: "Compile revisions",
    icon: FileCheck,
    capability: "operate_platform",
    group: "operations"
  },
  {
    to: ROUTES.ADMIN.OLTP,
    label: "OLTP databases",
    icon: Database,
    // Provisioning creates a billable project, so this matches the route gate
    // (`Action::PlatformOltp` → `operate_platform`) rather than `manage_apps`.
    capability: "operate_platform",
    group: "tenants"
  },
  {
    to: ROUTES.ADMIN.AIRHOUSE,
    label: "Airhouse warehouses",
    icon: Warehouse,
    // Same gate as OLTP: both provision a tenant's data plane, and a grant that
    // could create one but not the other would need a story for why.
    capability: "operate_platform",
    group: "tenants"
  },
  {
    to: ROUTES.ADMIN.EXPLORER,
    label: "Explorer",
    icon: Telescope,
    capability: "view_tenants",
    group: "operations"
  },
  {
    to: ROUTES.ADMIN.AUDIT,
    label: "Audit log",
    icon: ScrollText,
    capability: "view_audit",
    group: "operations"
  },
  // "Global admins" manages the `app_admins` table itself — "promotion /
  // demotion of admin", strict Global Owner only.
  {
    to: ROUTES.ADMIN.APP_ADMINS,
    // Matches the page's own <h1>. "Global admins" predates App Operator and named
    // one of the two roles the page administers, so the nav read as a different
    // surface from the one it opened.
    label: "Staff access",
    icon: ShieldCheck,
    capability: "manage_platform_grants",
    group: "operations"
  },
  {
    to: ROUTES.ADMIN.CUSTOMER_APPS,
    label: "Custom apps",
    icon: AppWindow,
    capability: "manage_apps",
    group: "operations"
  },
  // Publish tokens now lives as a tab inside Custom apps (/admin/apps?view=tokens),
  // not its own nav item — it's part of shipping apps, not a separate surface.
  {
    to: ROUTES.ADMIN.WORKSPACE_HEALTH,
    label: "Workspace health",
    icon: HeartPulse,
    capability: "operate_platform",
    group: "operations"
  },
  // Deployment-wide operational config, beside Workspace health / routing /
  // metrics — the server gates it on exactly that capability
  // (`admin::router()` mounts `airway_config` under
  // `cap(Action::PlatformOperate)`), so this must too.
  //
  // It was `ownerOnly: true`, written when the surface really was mounted
  // under the strict OXY_OWNER guard. When the backend moved to the
  // capability, this entry did not, and the two disagreeing is worse than
  // either alone: `canReachAdminRoute` votes with the same rule, so a
  // `operate_platform` holder was bounced off a page the API would serve them
  // — the link absent, the route redirecting, and `GET /admin/airway/config`
  // returning 200 the whole time.
  //
  // The original rationale (a policy flip can halt every pipeline of a kind)
  // is real and is answered where it belongs: the page previews the blast
  // radius and confirms a save that is not provably clean. It is not an
  // argument for a *narrower audience* than the endpoint has.
  {
    to: ROUTES.ADMIN.AIRWAY,
    label: "Airway",
    icon: Waypoints,
    capability: "operate_platform",
    group: "operations"
  },
  // Tenant management: the unified, relationship-first directory of orgs /
  // partners / users (workspaces live one level down, inside their org). Each
  // entry is a shortcut to one entity type of the SAME surface — clicking
  // "Partners" here is identical to picking Partners in the directory's own
  // header switcher (both just drive `?type=`). Open to owner OR app admin.
  {
    to: `${ROUTES.ADMIN.TENANTS}?type=orgs`,
    label: "Organizations",
    icon: Building2,
    capability: "manage_org_settings",
    group: "tenants"
  },
  {
    to: `${ROUTES.ADMIN.TENANTS}?type=partners`,
    label: "Partners",
    icon: Handshake,
    capability: "manage_partners",
    group: "tenants"
  },
  {
    to: `${ROUTES.ADMIN.TENANTS}?type=users`,
    label: "Users",
    icon: Users,
    capability: "manage_members",
    group: "tenants"
  }
];

/** The standing a nav rule is evaluated against. */
export type Standing = { isOwner: boolean; capabilities: PlatformCapability[] };

/** The rule for ONE entry. Owner-only rooms are a boolean the capability model
 * deliberately cannot reach; everything else names a capability; an entry with neither is
 * open to any staff member who got through the console door. */
export function itemReachable(item: AdminNavItem, { isOwner, capabilities }: Standing): boolean {
  if (item.ownerOnly) return isOwner;
  // Root satisfies every capability, the same short-circuit `may_delegate` and
  // `platform_grants` apply server-side. Reading only `capabilities` happened to work
  // because `/user` sends the owner `Cap::ALL` — a server implementation detail this
  // file should not be leaning on, and one that would blank the owner's own console the
  // day that read fails and returns an empty list.
  if (item.capability) return isOwner || capabilities.includes(item.capability);
  return true;
}

/** An entry's path, without the query it carries for the directory's `?type=` tabs. */
const navPath = (to: string) => to.split("?")[0];

/**
 * May this principal reach `pathname`? The route-guard half of the same map the sidebar
 * filters on, so a visible item is always a reachable one.
 *
 * **Not the per-item rule.** Three entries — Organizations, Partners, Users — are all
 * `/admin/tenants` with three *different* capabilities, so "longest match wins, then
 * apply its rule" would bounce someone holding `manage_members` but not
 * `manage_org_settings` off a page they can plainly use. The route is reachable if **any**
 * entry pointing at it is. The sidebar still decides per item, which is why the two
 * cannot share one rule verbatim: one asks "may I see this link", the other "may I be on
 * this page".
 *
 * The query string has to come off before matching. With it left on, `i.to` was
 * `/admin/tenants?type=orgs` and `location.pathname` is `/admin/tenants`, so no tenant
 * entry could ever match, `match` was undefined, and the guard returned `true`
 * unconditionally for the largest group in the map — a rule stated in a comment that the
 * code did not apply, which is the defect this function was written to end.
 *
 * Unknown paths return `true`: this is a redirect for a stale bookmark, not an
 * authorization control — the server decides, and guessing "deny" would bounce a route
 * that simply is not in the nav (a detail page, say).
 */
export function canReachAdminRoute(pathname: string, standing: Standing): boolean {
  const candidates = ADMIN_NAV.filter((i) => {
    const p = navPath(i.to);
    // Segment boundary, so `/admin/apps/<id>` inherits `/admin/apps` but a future
    // `/admin/apps-registry` does not.
    return pathname === p || pathname.startsWith(`${p}/`);
  });
  if (candidates.length === 0) return true;

  // Most specific path wins; every entry AT that path gets a vote.
  const longest = Math.max(...candidates.map((i) => navPath(i.to).length));
  return candidates
    .filter((i) => navPath(i.to).length === longest)
    .some((i) => itemReachable(i, standing));
}

/**
 * The first admin route this principal can actually use — where to send someone who
 * landed somewhere they cannot be.
 *
 * `AdminLayout` bounced to Custom apps, which is itself gated on `manage_apps`. Every
 * role shipping today holds it (Global Admin via `Cap::ALL - ManageBilling`, App Operator
 * by definition), so the bounce lands. But the point of this branch is that a narrower
 * preset is now cheap to add, and the first one that omits `manage_apps` — an audit-only
 * or grants-only role — would `Navigate` to a page the guard immediately bounces it off
 * again. A redirect cycle, not a bounce.
 *
 * The sidebar already needed this for its logo link, "so each role lands somewhere it can
 * actually use". Same map, same rule, one definition.
 */
export function firstReachableAdminRoute(standing: Standing): string {
  // `/` rather than a hardcoded admin route: a principal with nothing visible has no
  // admin landing place, and the layout guard above sends them home anyway.
  return ADMIN_NAV.find((i) => itemReachable(i, standing))?.to ?? "/";
}

/**
 * Pages that have a title but no rail entry: the flat directories the tenants surface
 * links down into, and Publish tokens, which lives as a tab inside Custom apps.
 */
const UNLISTED_TITLES: Record<string, string> = {
  // The console home. Deliberately not a rail entry — the rail's logo is its affordance.
  [ROUTES.ADMIN.ROOT]: "Operations",
  // Tenant-side triage. Not a rail entry either: it is reached from the console home,
  // and until then nothing linked to it at all — the route existed, unreferenced.
  [ROUTES.ADMIN.TENANTS_OVERVIEW]: "Operator overview",
  [ROUTES.ADMIN.ORGS]: "Organizations",
  [ROUTES.ADMIN.USERS]: "Users",
  [ROUTES.ADMIN.WORKSPACES]: "Workspaces",
  [ROUTES.ADMIN.PUBLISH_TOKENS]: "Publish tokens"
};

/**
 * What to call the page at `pathname` — read off the same map the rail renders, so a
 * page's name in the rail and its name in the topbar cannot disagree.
 *
 * They did: the layout kept its own `PAGE_TITLES` table, a third copy after the rail label
 * and the page's `<h1>`, and the three drifted ("Compile revisions" / "Compiles",
 * "Tenants overview" / "Operator overview" / "Organizations").
 *
 * `search` matters for exactly one path. Organizations, Partners and Users are three rail
 * entries on `/admin/tenants`, told apart only by `?type=`.
 */
export function adminPageTitle(pathname: string, search = ""): string {
  const within = (p: string) => pathname === p || pathname.startsWith(`${p}/`);
  const type = new URLSearchParams(search).get("type") ?? "orgs";

  // An exactly-registered page wins outright, because two of them sit *under* another
  // entry's path: the home is a prefix of every admin route, and the tenants overview
  // lives beneath `/admin/tenants`. A prefix scan names both after their parent.
  const exact = UNLISTED_TITLES[pathname];
  if (exact) return exact;

  const listed = ADMIN_NAV.filter((i) => within(navPath(i.to)))
    // Most specific path first, so `/admin/apps/<id>` reads "Custom apps".
    .sort((a, b) => navPath(b.to).length - navPath(a.to).length);
  const byType = listed.find(
    (i) => new URLSearchParams(i.to.split("?")[1] ?? "").get("type") === type
  );
  const hit = byType ?? listed[0];
  if (hit) return hit.label;

  const unlisted = Object.keys(UNLISTED_TITLES)
    .filter((p) => p !== ROUTES.ADMIN.ROOT && within(p))
    .sort((a, b) => b.length - a.length)[0];
  return unlisted ? UNLISTED_TITLES[unlisted] : "Admin";
}

/**
 * Which group the page at `pathname` belongs to, or `null` for a page outside the rail.
 * The topbar reads this so the breadcrumb states the whole position — `Admin / Tenants /
 * Organizations` — instead of the page repeating its own group as an eyebrow, which is
 * where the third copy of every page title used to live.
 */
export function adminPageGroup(pathname: string, search = ""): AdminNavGroup | null {
  const within = (p: string) => pathname === p || pathname.startsWith(`${p}/`);
  const type = new URLSearchParams(search).get("type") ?? "orgs";
  const listed = ADMIN_NAV.filter((i) => within(navPath(i.to))).sort(
    (a, b) => navPath(b.to).length - navPath(a.to).length
  );
  const byType = listed.find(
    (i) => new URLSearchParams(i.to.split("?")[1] ?? "").get("type") === type
  );
  const hit = byType ?? listed[0];
  if (hit) return hit.group;
  // The flat directories the tenants surface links down into have no rail entry of their
  // own, but they are plainly tenant pages.
  const unlisted = Object.keys(UNLISTED_TITLES)
    .filter((p) => p !== ROUTES.ADMIN.ROOT && within(p))
    .sort((a, b) => b.length - a.length)[0];
  // Publish tokens is a tab inside Custom apps, not a tenant directory; the home is not
  // a tenant page either, and being every path's prefix it would claim all of them.
  return unlisted && unlisted !== ROUTES.ADMIN.PUBLISH_TOKENS ? "tenants" : null;
}
