import { describe, expect, it } from "vitest";
import ROUTES from "@/libs/utils/routes";
import type { PlatformCapability } from "@/types/auth";
import {
  ADMIN_NAV,
  adminPageGroup,
  adminPageTitle,
  adminSettingsReachable,
  canReachAdminRoute,
  firstReachableAdminRoute,
  navItemReachable,
  usageReportRecipientsReachable
} from "./adminNav";

/**
 * The route guard, tested by **behaviour on real pathnames** rather than by inspecting
 * the map.
 *
 * Both bugs this file exists for were invisible to a shape check. The first version used
 * a bare `startsWith` under a comment claiming segment-boundary matching. The second kept
 * the query string in `i.to`, so no tenant entry could ever match a `location.pathname`
 * and the guard silently returned `true` for the largest group in the map — it "worked"
 * in the sense that nobody was wrongly bounced, which is exactly why nothing noticed.
 */

const staff = (...capabilities: PlatformCapability[]) => ({ isOwner: false, capabilities });
const owner = { isOwner: true, capabilities: [] as PlatformCapability[] };
const nobody = staff();

// Paths come from the route table, never typed out here: an invented path matches no
// entry and the guard admits unknown paths, so a typo turns every assertion in its test
// into a vacuous `expect(true).toBe(true)`. `/admin/billing-queue` was such a typo — the
// real constant is `/admin/billing/queue`.

describe("canReachAdminRoute", () => {
  it("admits the tenants directory to any ONE of its three capabilities", () => {
    // Organizations / Partners / Users are three entries at one path, each naming a
    // different capability. Longest-match-then-apply would have bounced the second and
    // third off a page they can plainly use.
    expect(canReachAdminRoute("/admin/tenants", staff("manage_org_settings"))).toBe(true);
    expect(canReachAdminRoute("/admin/tenants", staff("manage_partners"))).toBe(true);
    expect(canReachAdminRoute("/admin/tenants", staff("manage_members"))).toBe(true);
  });

  it("refuses the tenants directory to staff holding none of them", () => {
    // The assertion the query-string bug made unreachable: with `?type=orgs` left on
    // `i.to`, nothing matched and this returned true.
    expect(canReachAdminRoute("/admin/tenants", staff("manage_apps"))).toBe(false);
  });

  it("gates an owner-only room on the boolean, not a capability", () => {
    expect(canReachAdminRoute(ROUTES.ADMIN.BILLING_QUEUE, owner)).toBe(true);
    // Holding every capability is still not being root.
    expect(
      canReachAdminRoute(
        ROUTES.ADMIN.BILLING_QUEUE,
        staff("manage_platform_grants", "operate_platform", "view_tenants")
      )
    ).toBe(false);
  });

  it("gates airway on operate_platform, matching the endpoint", () => {
    // Regression: this entry was `ownerOnly` while the server mounted
    // `airway_config` under `cap(Action::PlatformOperate)`. A holder of that
    // capability got a 200 from `GET /admin/airway/config` and was still
    // bounced off the page — the nav and the API disagreeing about one
    // surface, which is the bug this asserts against.
    expect(canReachAdminRoute(ROUTES.ADMIN.AIRWAY, staff("operate_platform"))).toBe(true);
    expect(canReachAdminRoute(ROUTES.ADMIN.AIRWAY, owner)).toBe(true);
    // Still staff-only: the capability is the door, not the absence of one.
    expect(canReachAdminRoute(ROUTES.ADMIN.AIRWAY, staff("manage_apps"))).toBe(false);
    expect(canReachAdminRoute(ROUTES.ADMIN.AIRWAY, nobody)).toBe(false);
  });

  it("gates OLTP on operate_platform, matching the endpoint", () => {
    // The nav and the route gate must name the same capability. The server
    // mounts these routes under `cap(Action::PlatformOltp)`, which resolves to
    // `operate_platform` — deliberately NOT `manage_apps`, because provisioning
    // creates a billable database and an App Operator ships apps and nothing
    // else. An App Operator seeing the entry and getting a 403 would be the
    // same nav/API disagreement the airway case above regressed on.
    expect(canReachAdminRoute(ROUTES.ADMIN.OLTP, staff("operate_platform"))).toBe(true);
    expect(canReachAdminRoute(ROUTES.ADMIN.OLTP, owner)).toBe(true);
    expect(canReachAdminRoute(ROUTES.ADMIN.OLTP, staff("manage_apps"))).toBe(false);
    expect(canReachAdminRoute(ROUTES.ADMIN.OLTP, nobody)).toBe(false);
  });

  it("gates sandbox agent tokens on operate_platform, matching the endpoint", () => {
    // The server mounts the list under `cap(Action::PlatformOperate)`. An App Operator
    // may mint a sandbox agent token and still does not hold the shared list, so
    // `manage_apps` and `develop_apps` must not open it.
    const route = ROUTES.ADMIN.SANDBOX_AGENT_TOKENS;
    expect(canReachAdminRoute(route, staff("operate_platform"))).toBe(true);
    expect(canReachAdminRoute(route, owner)).toBe(true);
    expect(canReachAdminRoute(route, staff("manage_apps", "develop_apps"))).toBe(false);
    expect(canReachAdminRoute(route, nobody)).toBe(false);
    // The rail asks the same question of the same entry.
    expect(navItemReachable(route, staff("operate_platform"))).toBe(true);
    expect(navItemReachable(route, staff("manage_apps", "develop_apps"))).toBe(false);
    expect(adminPageTitle(route)).toBe("Sandbox agent tokens");
    expect(adminPageGroup(route)).toBe("operations");
  });

  it("gates staff & partner tokens as Staff access is, on manage_platform_grants", () => {
    // Who holds a token with standing is the same question as who holds staff access,
    // and `GET /api/admin/standing-tokens` asks for the same capability. Operating the
    // platform does not open it, and neither does reading the audit log.
    const route = ROUTES.ADMIN.STANDING_TOKENS;
    expect(canReachAdminRoute(route, staff("manage_platform_grants"))).toBe(true);
    expect(canReachAdminRoute(route, owner)).toBe(true);
    expect(canReachAdminRoute(route, staff("operate_platform", "view_audit"))).toBe(false);
    expect(canReachAdminRoute(route, nobody)).toBe(false);
    // One entry answers for the rail, the ⌘K palette and the route guard.
    expect(navItemReachable(route, staff("manage_platform_grants"))).toBe(true);
    expect(navItemReachable(route, staff("operate_platform", "view_audit"))).toBe(false);
    const entry = (to: string) => ADMIN_NAV.find((item) => item.to === to);
    expect(entry(route)?.capability).toBe(entry(ROUTES.ADMIN.APP_ADMINS)?.capability);
    expect(adminPageTitle(route)).toBe("Staff & partner tokens");
    // Beside Sandbox agent tokens, in the same group.
    expect(adminPageGroup(route)).toBe(adminPageGroup(ROUTES.ADMIN.SANDBOX_AGENT_TOKENS));
    const labels = ADMIN_NAV.map((item) => item.label);
    expect(labels.indexOf("Staff & partner tokens")).toBe(
      labels.indexOf("Sandbox agent tokens") + 1
    );
  });

  it("gates the grant console on manage_platform_grants", () => {
    expect(canReachAdminRoute("/admin/app-admins", staff("manage_platform_grants"))).toBe(true);
    expect(canReachAdminRoute("/admin/app-admins", staff("manage_apps"))).toBe(false);
    expect(canReachAdminRoute("/admin/app-admins", owner)).toBe(true);
  });

  it("lets a nested route inherit its parent's rule", () => {
    expect(canReachAdminRoute("/admin/apps/some-app-id", staff("manage_apps"))).toBe(true);
    expect(canReachAdminRoute("/admin/apps/some-app-id", nobody)).toBe(false);
  });

  it("does not let a sibling with a shared prefix inherit that rule", () => {
    // `/admin/apps-registry` is not under `/admin/apps`. A bare `startsWith` says it is,
    // which is what the comment claimed was already handled.
    expect(canReachAdminRoute("/admin/apps-registry", nobody)).toBe(true);
    // And the real neighbour that shares four characters stays on its own rule.
    expect(canReachAdminRoute("/admin/app-admins", staff("manage_apps"))).toBe(false);
  });

  it("admits an unknown path — this is a stale-bookmark redirect, not a control", () => {
    expect(canReachAdminRoute("/admin/not-in-the-nav", nobody)).toBe(true);
  });
});

/**
 * The bounce target must be somewhere the same principal can be.
 *
 * `AdminLayout` sent everyone to Custom apps, which is gated on `manage_apps`. Every role
 * shipping today holds it, so nothing was broken — but the premise of this branch is that
 * a narrower preset is now cheap, and the first one omitting `manage_apps` turns the
 * bounce into a redirect cycle. A cycle is not a wrong answer the guard can report; it is
 * a hung page.
 */
describe("firstReachableAdminRoute", () => {
  it("never returns a route the same standing cannot reach", () => {
    const standings = [
      owner,
      staff("manage_apps", "develop_apps"), // App Operator
      staff("view_audit"), // an audit-only preset that does not exist yet
      staff("manage_platform_grants"), // a grants-only preset that does not exist yet
      staff("manage_members")
    ];
    for (const standing of standings) {
      const target = firstReachableAdminRoute(standing);
      // The two functions take different things and the difference is load-bearing:
      // this returns a `to` (a link target, query included — the tenants directory needs
      // `?type=`), while the guard takes a `location.pathname`, which never has one.
      // React Router does this strip for us at runtime: `<Navigate to="/admin/tenants
      // ?type=users">` lands with `pathname === "/admin/tenants"`.
      //
      // Passing the raw `to` here made the `manage_members` case — the ONLY one that
      // reaches a tenants entry, and so the only one covering the any-of rule — match no
      // candidate and pass through the unknown-path fallback. It asserted nothing, and
      // would have kept passing if that target became genuinely unreachable.
      const landedPath = target.split("?")[0];
      expect(
        canReachAdminRoute(landedPath, standing),
        `bounce target ${target} is itself unreachable — that is a redirect cycle`
      ).toBe(true);
    }
  });

  it("sends a principal with no admin surface home rather than into the console", () => {
    expect(firstReachableAdminRoute(nobody)).toBe("/");
  });
});

describe("adminPageTitle", () => {
  it("names a page by its rail label", () => {
    expect(adminPageTitle(ROUTES.ADMIN.COMPILES)).toBe("Compile revisions");
    expect(adminPageTitle(ROUTES.ADMIN.AIRWAY)).toBe("Airway");
  });

  it("lets a nested route inherit its parent's name", () => {
    expect(adminPageTitle(`${ROUTES.ADMIN.CUSTOMER_APPS}/acme/oxy-starter`)).toBe("Custom apps");
  });

  it("stops at a segment boundary", () => {
    expect(adminPageTitle(`${ROUTES.ADMIN.CUSTOMER_APPS}-registry`)).toBe("Admin");
  });

  it("tells the three tenants entries apart by ?type=, defaulting to organizations", () => {
    expect(adminPageTitle(ROUTES.ADMIN.TENANTS, "?type=partners")).toBe("Partners");
    expect(adminPageTitle(ROUTES.ADMIN.TENANTS, "?type=users")).toBe("Users");
    expect(adminPageTitle(ROUTES.ADMIN.TENANTS)).toBe("Organizations");
  });

  it("names the directories that have no rail entry, and their detail pages", () => {
    expect(adminPageTitle(ROUTES.ADMIN.WORKSPACES)).toBe("Workspaces");
    expect(adminPageTitle(ROUTES.ADMIN.ORG_DETAIL("some-org"))).toBe("Organizations");
    expect(adminPageTitle(ROUTES.ADMIN.USER_DETAIL("some-user"))).toBe("Users");
  });

  it("falls back for a path it has never heard of", () => {
    expect(adminPageTitle("/admin/nowhere")).toBe("Admin");
  });
});

describe("adminPageGroup", () => {
  it("places a page in the group its rail entry sits in", () => {
    expect(adminPageGroup(ROUTES.ADMIN.COMPILES)).toBe("operations");
    expect(adminPageGroup(ROUTES.ADMIN.AIRHOUSE)).toBe("tenants");
  });

  it("tells the three tenants entries apart by ?type=", () => {
    expect(adminPageGroup(ROUTES.ADMIN.TENANTS, "?type=users")).toBe("tenants");
  });

  it("places the flat directories with the tenants, and their detail pages too", () => {
    expect(adminPageGroup(ROUTES.ADMIN.ORGS)).toBe("tenants");
    expect(adminPageGroup(ROUTES.ADMIN.WORKSPACE_DETAIL("w1"))).toBe("tenants");
  });

  it("does not call publish tokens a tenant page just because it has no rail entry", () => {
    expect(adminPageGroup(ROUTES.ADMIN.PUBLISH_TOKENS)).toBeNull();
  });

  it("has no opinion about a path outside the console", () => {
    expect(adminPageGroup("/admin/nowhere")).toBeNull();
  });
});

describe("the console home", () => {
  it("is named by the map, like every other page", () => {
    expect(adminPageTitle(ROUTES.ADMIN.ROOT)).toBe("Operations");
  });

  it("does not claim every admin path just by being their prefix", () => {
    expect(adminPageTitle("/admin/nowhere")).toBe("Admin");
    expect(adminPageTitle(`${ROUTES.ADMIN.CUSTOMER_APPS}-registry`)).toBe("Admin");
    expect(adminPageTitle(ROUTES.ADMIN.COMPILES)).toBe("Compile revisions");
  });

  it("belongs to no rail group, and does not drag other pages into one", () => {
    expect(adminPageGroup(ROUTES.ADMIN.ROOT)).toBeNull();
    expect(adminPageGroup("/admin/nowhere")).toBeNull();
  });
});

describe("pages registered exactly", () => {
  /**
   * Two pages sit *under* another entry's path — the home is a prefix of every admin
   * route, and the overview lives beneath `/admin/tenants`. A prefix scan named both
   * after their parent, so the overview's breadcrumb read "Organizations".
   */
  it("names the tenants overview itself, not its parent directory", () => {
    expect(adminPageTitle(ROUTES.ADMIN.TENANTS_OVERVIEW)).toBe("Operator overview");
  });

  it("still names the directory the overview sits under", () => {
    expect(adminPageTitle(ROUTES.ADMIN.TENANTS, "?type=partners")).toBe("Partners");
  });

  it("puts the overview with the tenants in the rail groups", () => {
    expect(adminPageGroup(ROUTES.ADMIN.TENANTS_OVERVIEW)).toBe("tenants");
  });

  it("is reachable by anyone the tenants directory admits", () => {
    expect(canReachAdminRoute(ROUTES.ADMIN.TENANTS_OVERVIEW, staff("manage_members"))).toBe(true);
  });
});

describe("navItemReachable", () => {
  const operator = {
    isOwner: false,
    capabilities: ["manage_apps", "develop_apps"] as PlatformCapability[]
  };
  it("keeps Workspace health from an App Operator, so nothing fetches its badge", () => {
    expect(navItemReachable(ROUTES.ADMIN.WORKSPACE_HEALTH, operator)).toBe(false);
    expect(navItemReachable(ROUTES.ADMIN.CUSTOMER_APPS, operator)).toBe(true);
  });
  it("shows it to operate_platform and to the owner", () => {
    const platform = { isOwner: false, capabilities: ["operate_platform"] as PlatformCapability[] };
    expect(navItemReachable(ROUTES.ADMIN.WORKSPACE_HEALTH, platform)).toBe(true);
    expect(
      navItemReachable(ROUTES.ADMIN.WORKSPACE_HEALTH, { isOwner: true, capabilities: [] })
    ).toBe(true);
  });
  it("answers false for a path with no rail entry — a fetch nobody may see", () => {
    expect(navItemReachable("/admin/nowhere", { isOwner: true, capabilities: [] })).toBe(false);
  });
});

/**
 * The weekly custom-app usage report. It sits beside Custom apps in the rail but is gated
 * like the other fleet-wide readouts: the server mounts every `/admin/usage-report` route
 * under `operate_platform`, so an App Operator — `manage_apps` and nothing else — must not
 * be offered a link the API answers with a 403.
 */
describe("the usage report", () => {
  it("is offered to operate_platform and to the owner", () => {
    expect(navItemReachable(ROUTES.ADMIN.USAGE_REPORT, staff("operate_platform"))).toBe(true);
    expect(navItemReachable(ROUTES.ADMIN.USAGE_REPORT, owner)).toBe(true);
  });

  it("is not offered to staff holding only manage_apps, and the guard agrees", () => {
    expect(navItemReachable(ROUTES.ADMIN.USAGE_REPORT, staff("manage_apps"))).toBe(false);
    expect(navItemReachable(ROUTES.ADMIN.USAGE_REPORT, nobody)).toBe(false);
    // The rail and the route guard read one map, so a hidden link is also a bounced URL.
    expect(canReachAdminRoute(ROUTES.ADMIN.USAGE_REPORT, staff("manage_apps"))).toBe(false);
    expect(canReachAdminRoute(ROUTES.ADMIN.USAGE_REPORT, staff("operate_platform"))).toBe(true);
  });

  it("is named by the map and sits right after Custom apps in the operations group", () => {
    expect(adminPageTitle(ROUTES.ADMIN.USAGE_REPORT)).toBe("Usage report");
    expect(adminPageGroup(ROUTES.ADMIN.USAGE_REPORT)).toBe("operations");
    const order = ADMIN_NAV.map((i) => i.to);
    expect(order.indexOf(ROUTES.ADMIN.USAGE_REPORT)).toBe(
      order.indexOf(ROUTES.ADMIN.CUSTOMER_APPS) + 1
    );
  });
});

/**
 * Settings holds one person's preferences, so it has a title but no rail entry and no
 * group. An unlisted page used to fall through to "tenants", which would have made its
 * breadcrumb read `Admin / Tenants / Settings`.
 */
describe("the settings page", () => {
  it("is named by the map", () => {
    expect(adminPageTitle(ROUTES.ADMIN.SETTINGS)).toBe("Settings");
  });

  it("belongs to no rail group", () => {
    expect(adminPageGroup(ROUTES.ADMIN.SETTINGS)).toBeNull();
    // The exclusion is for Settings alone: the directories it sits beside keep theirs.
    expect(adminPageGroup(ROUTES.ADMIN.ORGS)).toBe("tenants");
  });

  it("has no rail entry, so it is not in the rail or the palette", () => {
    expect(ADMIN_NAV.some((i) => i.to.split("?")[0] === ROUTES.ADMIN.SETTINGS)).toBe(false);
  });

  it("is offered to whoever can use it: operate_platform, or the owner", () => {
    expect(adminSettingsReachable(staff("operate_platform"))).toBe(true);
    expect(adminSettingsReachable(owner)).toBe(true);
    expect(adminSettingsReachable(staff("manage_apps", "develop_apps"))).toBe(false);
    expect(adminSettingsReachable(nobody)).toBe(false);
  });
});

/**
 * Deciding who *else* is emailed the usage report. The server gates the two `recipients`
 * routes on `manage_platform_grants`, which is narrower than the Settings page those
 * controls sit on — so holding the page's capability must not be enough.
 */
describe("who gets the usage report", () => {
  it("is for manage_platform_grants, and for the owner", () => {
    expect(usageReportRecipientsReachable(staff("manage_platform_grants"))).toBe(true);
    expect(usageReportRecipientsReachable(owner)).toBe(true);
  });

  it("is not for someone who holds only the page's own capability", () => {
    // The assertion this rule exists for: `operate_platform` opens Settings and reads
    // the report, and still does not get to switch off another person's email.
    expect(usageReportRecipientsReachable(staff("operate_platform"))).toBe(false);
    expect(adminSettingsReachable(staff("operate_platform"))).toBe(true);
  });

  it("is not for an App Operator, or for staff holding nothing", () => {
    expect(usageReportRecipientsReachable(staff("manage_apps", "develop_apps"))).toBe(false);
    expect(usageReportRecipientsReachable(nobody)).toBe(false);
  });
});
