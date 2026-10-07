// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { UsageReport } from "@/types/usageReport";
import { app, counts, highlight, org, report, summary } from "./testFixtures";

const useUsageReport = vi.fn();
vi.mock("@/hooks/api/usageReport", () => ({
  useUsageReport: () => useUsageReport()
}));

import AdminUsageReport from "./index";

// Routed at the real path: `AdminPage` takes its `<h1>` from the admin route map, which
// is keyed off `useLocation()`.
const mount = () =>
  render(
    <MemoryRouter initialEntries={["/admin/usage-report"]}>
      <AdminUsageReport />
    </MemoryRouter>
  );

const loaded = (latest: UsageReport | null) => {
  useUsageReport.mockReturnValue({
    data: { report: latest },
    isPending: false,
    isError: false,
    error: null
  });
  return mount();
};

const hrefOf = (el: HTMLElement) => within(el).getByRole("link").getAttribute("href");

beforeEach(() => {
  useUsageReport.mockReset();
});
afterEach(cleanup);

describe("AdminUsageReport — the top of the report", () => {
  it("is named by the route map and leads with the server's sentence", () => {
    loaded(report());
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Usage report");
    expect(screen.getByTestId("admin-usage-report-headline").textContent).toBe(
      "42 people opened 9 custom apps across 4 organizations."
    );
    expect(screen.getByTestId("admin-usage-report-comparison").textContent).toBe(
      "That is 7 more people than the week before."
    );
  });

  it("shows the week through its last day, since the end is exclusive", () => {
    loaded(report());
    expect(screen.getByTestId("admin-usage-report-period").textContent).toBe(
      "Sep 28 – Oct 4, 2026"
    );
  });

  it("puts the secondary counts on one line beneath it", () => {
    loaded(report());
    expect(screen.getByTestId("admin-usage-report-count-views").textContent).toBe(
      "1,318 app opens"
    );
    expect(screen.getByTestId("admin-usage-report-count-apps").textContent).toBe(
      "9 of 12 apps in use"
    );
    expect(screen.getByTestId("admin-usage-report-count-orgs").textContent).toBe(
      "4 of 5 organizations active"
    );
  });

  const countsLine = () =>
    within(screen.getByTestId("admin-usage-report-counts"))
      .getAllByRole("listitem")
      .map((li) => li.textContent);

  it("adds function calls, releases and storage to that line when there are any", () => {
    loaded(
      report({
        summary: summary({
          function_calls: 1240,
          function_failures: 31,
          releases: 4,
          storage_bytes: 3.4 * 1024 ** 3
        })
      })
    );
    expect(countsLine()).toEqual([
      "1,318 app opens",
      "9 of 12 apps in use",
      "4 of 5 organizations active",
      "1,240 function calls, 31 failed",
      "4 releases",
      "3.4 GB stored"
    ]);
  });

  it("leaves each of those out at zero or unmeasured, rather than printing a zero", () => {
    loaded(report({ summary: summary({ function_calls: 0, releases: 0, storage_bytes: null }) }));
    expect(countsLine()).toEqual([
      "1,318 app opens",
      "9 of 12 apps in use",
      "4 of 5 organizations active"
    ]);
    expect(screen.queryByTestId("admin-usage-report-count-functions")).toBeNull();
    expect(screen.queryByTestId("admin-usage-report-count-releases")).toBeNull();
    expect(screen.queryByTestId("admin-usage-report-count-storage")).toBeNull();
  });

  it("links to the email settings", () => {
    loaded(report());
    expect(screen.getByTestId("admin-usage-report-email-settings").getAttribute("href")).toBe(
      "/admin/settings"
    );
  });
});

describe("AdminUsageReport — highlights", () => {
  const mixed = [
    highlight({
      app_id: "quiet",
      kind: "went_quiet",
      tone: "attention",
      app_name: "Store Ops",
      app_slug: "store-ops",
      org_name: "Rivermark",
      org_slug: "rivermark",
      detail: "5 people → nobody"
    }),
    highlight({
      app_id: "grow",
      kind: "growing",
      tone: "good",
      app_name: "Shift Board",
      app_slug: "shift-board",
      org_name: "Poke House",
      org_slug: "pokehouse",
      detail: "9 → 23 people"
    }),
    highlight({
      app_id: "fail",
      kind: "failing_functions",
      tone: "attention",
      app_name: "Invoices",
      app_slug: "invoices",
      org_name: "Poke House",
      org_slug: "pokehouse",
      detail: "31 of 240 calls failed"
    }),
    highlight({
      app_id: "fresh",
      kind: "first_week",
      tone: "good",
      app_name: "Checklists",
      app_slug: "checklists",
      org_name: "Rivermark",
      org_slug: "rivermark",
      detail: "4 people"
    }),
    highlight({
      app_id: "idle",
      kind: "unused",
      tone: "idle",
      app_name: "Old Dashboard",
      app_slug: "old-dashboard",
      org_name: "Northwind",
      org_slug: "northwind",
      detail: "No opens in two weeks"
    })
  ];

  const idsIn = (group: "attention" | "good" | "idle") =>
    within(screen.getByTestId(`admin-usage-report-highlights-${group}`))
      .getAllByTestId(/^admin-usage-report-highlight-[a-z]+-[a-z_]+$/)
      .map((el) => el.getAttribute("data-testid"));

  it("groups them by tone, keeping the server's order inside each group", () => {
    loaded(report({ highlights: mixed }));
    expect(idsIn("attention")).toEqual([
      "admin-usage-report-highlight-quiet-went_quiet",
      "admin-usage-report-highlight-fail-failing_functions"
    ]);
    expect(idsIn("good")).toEqual([
      "admin-usage-report-highlight-grow-growing",
      "admin-usage-report-highlight-fresh-first_week"
    ]);
    expect(idsIn("idle")).toEqual(["admin-usage-report-highlight-idle-unused"]);
  });

  it("names each group", () => {
    loaded(report({ highlights: mixed }));
    const heading = (group: string) =>
      within(screen.getByTestId(`admin-usage-report-highlights-${group}`)).getByRole("heading", {
        level: 3
      }).textContent;
    expect(heading("attention")).toBe("Needs a look");
    expect(heading("good")).toBe("Going well");
    expect(heading("idle")).toBe("Published but not opened");
  });

  it("shows a row's kind, app, organization, and the server's detail verbatim", () => {
    loaded(report({ highlights: mixed }));
    const row = screen.getByTestId("admin-usage-report-highlight-fail-failing_functions");
    expect(within(row).getByText("Functions failing")).toBeTruthy();
    expect(within(row).getByRole("link").textContent).toBe("Invoices");
    expect(within(row).getByText("Poke House")).toBeTruthy();
    // Exactly as sent — the page adds its own label beside the fragment and changes
    // nothing inside it, the arrow included.
    expect(
      screen.getByTestId("admin-usage-report-highlight-fail-failing_functions-detail").textContent
    ).toBe("31 of 240 calls failed");
    expect(
      screen.getByTestId("admin-usage-report-highlight-quiet-went_quiet-detail").textContent
    ).toBe("5 people → nobody");
  });

  it("reads a row as label, app, organization, then the fragment — nothing else", () => {
    // The detail is a short fragment now, not a sentence. Pinning the whole row's text
    // keeps the page from wrapping it in words of its own ("Detail:", a trailing period).
    loaded(report({ highlights: mixed }));
    const texts = (testId: string) =>
      Array.from(screen.getByTestId(testId).querySelectorAll("span, a, p"))
        .filter((el) => el.children.length === 0)
        .map((el) => el.textContent);
    expect(texts("admin-usage-report-highlight-grow-growing")).toEqual([
      "More people",
      "Shift Board",
      "Poke House",
      "9 → 23 people"
    ]);
  });

  it("labels the first-week highlight 'First week'", () => {
    loaded(report({ highlights: mixed }));
    const row = screen.getByTestId("admin-usage-report-highlight-fresh-first_week");
    expect(within(row).getByText("First week")).toBeTruthy();
  });

  it("links every highlighted app to its console under its organization", () => {
    loaded(report({ highlights: mixed }));
    expect(hrefOf(screen.getByTestId("admin-usage-report-highlight-quiet-went_quiet"))).toBe(
      "/admin/apps/rivermark/store-ops"
    );
    expect(hrefOf(screen.getByTestId("admin-usage-report-highlight-grow-growing"))).toBe(
      "/admin/apps/pokehouse/shift-board"
    );
    expect(hrefOf(screen.getByTestId("admin-usage-report-highlight-idle-unused"))).toBe(
      "/admin/apps/northwind/old-dashboard"
    );
  });

  it("lists an unopened app as a link, without a kind label or the detail sentence", () => {
    loaded(report({ highlights: mixed }));
    const idle = screen.getByTestId("admin-usage-report-highlight-idle-unused");
    expect(within(idle).getByRole("link").textContent).toBe("Old Dashboard");
    expect(idle.textContent).not.toContain("No opens in two weeks");
    expect(idle.textContent).not.toContain("Not opened");
  });

  it("says so when nothing needs a look, and omits the groups with nothing in them", () => {
    loaded(report({ highlights: [] }));
    expect(screen.getByTestId("admin-usage-report-highlights-attention-empty").textContent).toBe(
      "Nothing needs a look this week."
    );
    expect(screen.queryByTestId("admin-usage-report-highlights-good")).toBeNull();
    expect(screen.queryByTestId("admin-usage-report-highlights-idle")).toBeNull();
  });

  it("drops the all-clear line as soon as something does need a look", () => {
    loaded(report({ highlights: [mixed[0]] }));
    expect(screen.queryByTestId("admin-usage-report-highlights-attention-empty")).toBeNull();
  });
});

describe("AdminUsageReport — by organization", () => {
  const rivermark = org({
    org_id: "org-r",
    name: "Rivermark",
    slug: "rivermark",
    people: 12,
    prev_people: 9,
    views: 1500,
    apps: [
      app({
        app_id: "app-ops",
        name: "Store Ops",
        slug: "store-ops",
        current: counts({ people: 9, views: 1200 }),
        previous: counts({ people: 11, views: 900 })
      }),
      app({ app_id: "app-check", name: "Checklists", slug: "checklists" })
    ]
  });

  const headers = () => screen.getAllByRole("columnheader").map((th) => th.textContent);

  it("lists each organization, then its apps, in one table", () => {
    loaded(report({ orgs: [rivermark] }));
    expect(screen.getAllByRole("table")).toHaveLength(1);
    const rows = screen.getAllByRole("row").map((r) => r.getAttribute("data-testid"));
    // Header row first (no testid), then the org, then that org's apps in order.
    expect(rows).toEqual([
      null,
      "admin-usage-report-org-org-r",
      "admin-usage-report-app-app-ops",
      "admin-usage-report-app-app-check"
    ]);
  });

  it("links an app row to its console under its organization", () => {
    loaded(report({ orgs: [rivermark] }));
    expect(hrefOf(screen.getByTestId("admin-usage-report-app-app-ops"))).toBe(
      "/admin/apps/rivermark/store-ops"
    );
  });

  it("shows the change in people as a signed count", () => {
    loaded(report({ orgs: [rivermark] }));
    expect(screen.getByTestId("admin-usage-report-org-org-r-change").textContent).toBe("+3");
    expect(screen.getByTestId("admin-usage-report-app-app-ops-change").textContent).toBe("−2");
    expect(screen.getByTestId("admin-usage-report-app-app-check-change").textContent).toBe("0");
  });

  it("leaves out the columns nothing can answer, and says why once", () => {
    loaded(report({ orgs: [rivermark] }));
    expect(headers()).toEqual(["Organization and app", "People", "Change", "App opens"]);
    expect(screen.getByTestId("admin-usage-report-hidden-columns").textContent).toBe(
      "Function calls and failed calls are not shown because no app called a function in either week. " +
        "Sessions with an error are not shown because no app recorded one in either week. " +
        "Releases are not shown because no app had one in either week. " +
        "Storage is not shown because it has not been measured for any app."
    );
  });

  // An org whose own figures are not what its apps add up to, so a cell that summed the
  // rows beneath it would read differently from one that showed the report's number.
  const busy = org({
    org_id: "org-b",
    function_calls: 240,
    function_failures: 31,
    releases: 6,
    storage_bytes: 5 * 1024 ** 3,
    apps: [
      app({
        app_id: "app-busy",
        current: counts({
          function_calls: 40,
          function_failures: 3,
          error_sessions: 2,
          releases: 4
        }),
        storage_bytes: 3.4 * 1024 ** 3
      }),
      app({ app_id: "app-empty", storage_bytes: 0 }),
      app({ app_id: "app-unmeasured", storage_bytes: null })
    ]
  });

  const cell = (row: string, column: string) =>
    screen.getByTestId(`admin-usage-report-${row}-${column}`);

  it("shows them once an app has something to say, with no note left behind", () => {
    loaded(report({ orgs: [busy] }));
    expect(headers()).toEqual([
      "Organization and app",
      "People",
      "Change",
      "App opens",
      "Function calls",
      "Failed calls",
      "Sessions with an error",
      "Releases",
      "Storage"
    ]);
    expect(cell("app-app-busy", "functionFailures").textContent).toBe("3");
    expect(cell("app-app-busy", "releases").textContent).toBe("4");
    expect(cell("app-app-busy", "storage").textContent).toBe("3.4 GB");
    expect(screen.queryByTestId("admin-usage-report-hidden-columns")).toBeNull();
  });

  it("names only the columns that are missing", () => {
    // Releases happened; nothing else did.
    const released = org({ apps: [app({ current: counts({ releases: 1 }) })] });
    loaded(report({ orgs: [released] }));
    expect(headers()).toEqual([
      "Organization and app",
      "People",
      "Change",
      "App opens",
      "Releases"
    ]);
    const note = screen.getByTestId("admin-usage-report-hidden-columns").textContent;
    expect(note).not.toContain("Releases");
    expect(note).toContain("Storage is not shown");
  });

  it("fills an organization's row from the report's own figures for it", () => {
    loaded(report({ orgs: [busy] }));
    expect(cell("org-org-b", "functionCalls").textContent).toBe("240");
    expect(cell("org-org-b", "functionFailures").textContent).toBe("31");
    expect(cell("org-org-b", "releases").textContent).toBe("6");
    expect(cell("org-org-b", "storage").textContent).toBe("5.0 GB");
    // The one column the report has no organization figure for stays blank.
    expect(cell("org-org-b", "errorSessions").textContent).toBe("");
  });

  /**
   * Two different facts, and they must not look alike: an app measured and found empty
   * reads as a number, an app nobody measured reads as words — muted, so the eye running
   * down the column for sizes does not stop on it.
   */
  it("says 'not measured' for an unmeasured app, and 0 B for an empty one", () => {
    loaded(report({ orgs: [busy] }));
    const empty = cell("app-app-empty", "storage");
    expect(empty.textContent).toBe("0 B");
    expect(empty.querySelector(".text-muted-foreground")).toBeNull();

    const unmeasured = cell("app-app-unmeasured", "storage");
    expect(unmeasured.textContent).toBe("not measured");
    expect(unmeasured.querySelector(".text-muted-foreground")?.textContent).toBe("not measured");
  });

  it("says 'not measured' for an organization none of whose apps was", () => {
    const unmeasuredOrg = org({ org_id: "org-u", storage_bytes: null, apps: [] });
    loaded(report({ orgs: [busy, unmeasuredOrg] }));
    expect(cell("org-org-u", "storage").textContent).toBe("not measured");
  });
});

describe("AdminUsageReport — before there is a report, and when the fetch fails", () => {
  it("shows the empty state when no report has been written yet", () => {
    loaded(null);
    const empty = screen.getByTestId("admin-usage-report-empty");
    expect(empty.textContent).toContain("No report yet.");
    expect(empty.textContent).toContain(
      "The first one is written on a Monday, once a full week has ended."
    );
    expect(screen.queryByTestId("admin-usage-report-body")).toBeNull();
    expect(screen.queryByTestId("admin-async-error")).toBeNull();
  });

  /**
   * The assertion this file most exists for. A failed fetch has no data, and "no data"
   * is one `?? null` away from "no report yet" — which would tell an operator the first
   * report simply has not been written, on a console whose API is down.
   */
  it("shows the error state for a failed fetch, never the empty one", () => {
    useUsageReport.mockReturnValue({
      data: undefined,
      isPending: false,
      isError: true,
      error: new Error("usage report store unavailable"),
      refetch: vi.fn()
    });
    mount();
    expect(screen.getByTestId("admin-async-error").textContent).toContain(
      "Couldn’t load the usage report."
    );
    expect(screen.getByTestId("admin-async-error-detail").textContent).toBe(
      "usage report store unavailable"
    );
    expect(screen.queryByTestId("admin-usage-report-empty")).toBeNull();
    expect(screen.queryByText("No report yet.")).toBeNull();
  });

  it("shows neither while the report is still loading", () => {
    useUsageReport.mockReturnValue({ data: undefined, isPending: true, isError: false });
    mount();
    expect(screen.getByTestId("admin-async-loading")).toBeTruthy();
    expect(screen.queryByTestId("admin-usage-report-empty")).toBeNull();
    expect(screen.queryByTestId("admin-async-error")).toBeNull();
  });
});
