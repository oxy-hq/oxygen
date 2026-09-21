import { describe, expect, it } from "vitest";
import ROUTES from "@/libs/utils/routes";
import type { PlatformCapability } from "@/types/auth";
import {
  adminPageGroup,
  adminPageTitle,
  canReachAdminRoute,
  firstReachableAdminRoute
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
