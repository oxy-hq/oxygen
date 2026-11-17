import { describe, expect, it } from "vitest";
import type { UsageApp, UsageOrg } from "@/types/usageReport";
import { app, counts, org } from "./testFixtures";
import { type UsageColumnId, usageColumns } from "./usageColumns";

/**
 * The specification for "a column nothing can answer is absent, and named once".
 *
 * Written against the reason a column is absent as well as its absence, because the
 * failure being prevented is a column of `0` on every row that looks as if it might one
 * day say something.
 */

const ids = (orgs: UsageOrg[]): UsageColumnId[] => usageColumns(orgs).shown.map((c) => c.id);
const hiddenLabels = (orgs: UsageOrg[]): string[] => usageColumns(orgs).hidden.map((h) => h.label);

const BASE: UsageColumnId[] = ["people", "change", "views"];
const FUNCTIONS: UsageColumnId[] = ["functionCalls", "functionFailures"];

/** One org whose single app has these counts this week and those the week before. */
const orgWith = (current = counts(), previous = counts()): UsageOrg =>
  org({ apps: [app({ current, previous })] });

/** One org whose single app has this storage measurement. */
const orgStoring = (storage: Pick<UsageApp, "storage_bytes" | "storage_bytes_before">): UsageOrg =>
  org({ apps: [app(storage)] });

describe("usageColumns", () => {
  it("always shows people, change and opens, even for an empty report", () => {
    // The report is about these three. A table without them has stopped being it.
    expect(ids([])).toEqual(BASE);
    expect(ids([org({ apps: [] })])).toEqual(BASE);
  });

  it("names every column it left out of a report with nothing to say in them", () => {
    expect(usageColumns([orgWith(counts({ views: 40, people: 6 }))]).hidden).toEqual([
      {
        label: "Function calls and failed calls",
        verb: "are",
        why: "no app called a function in either week"
      },
      { label: "Sessions with an error", verb: "are", why: "no app recorded one in either week" },
      { label: "Releases", verb: "are", why: "no app had one in either week" },
      { label: "Storage", verb: "is", why: "it has not been measured for any app" }
    ]);
  });

  it("keeps the columns in one order however many are shown", () => {
    const everything = org({
      apps: [
        app({
          current: counts({ function_calls: 5, error_sessions: 1, releases: 2 }),
          storage_bytes: 1024
        })
      ]
    });
    expect(ids([everything])).toEqual([
      ...BASE,
      ...FUNCTIONS,
      "errorSessions",
      "releases",
      "storage"
    ]);
    expect(usageColumns([everything]).hidden).toEqual([]);
  });

  it("finds the one app that can answer, wherever in the report it is", () => {
    // Second org, second app: a scan that stops at the first org or the first app of
    // each would miss it.
    const orgs = [
      orgWith(),
      org({
        org_id: "org-2",
        apps: [
          app({ app_id: "a" }),
          app({
            app_id: "b",
            current: counts({ function_calls: 1, error_sessions: 1, releases: 1 }),
            storage_bytes: 0
          })
        ]
      })
    ];
    expect(usageColumns(orgs).hidden).toEqual([]);
  });
});

describe("usageColumns — functions and errors", () => {
  it("drops both function columns when no app called a function in either week", () => {
    const orgs = [orgWith(counts({ views: 40, people: 6 }))];
    expect(ids(orgs)).not.toContain("functionCalls");
    expect(ids(orgs)).not.toContain("functionFailures");
    expect(hiddenLabels(orgs)).toContain("Function calls and failed calls");
  });

  it("shows both function columns once any app called one this week", () => {
    const orgs = [orgWith(), orgWith(counts({ function_calls: 3 }))];
    expect(ids(orgs)).toEqual([...BASE, ...FUNCTIONS]);
    expect(hiddenLabels(orgs)).not.toContain("Function calls and failed calls");
  });

  it("keeps them when the calls were all the week before — that zero is real", () => {
    // An app that called functions last week and none this week stopped. Hiding the
    // column would turn "it stopped" into "this page does not report that".
    const orgs = [orgWith(counts(), counts({ function_calls: 12 }))];
    expect(ids(orgs)).toEqual([...BASE, ...FUNCTIONS]);
  });

  it("never hides a recorded failure, even beside a call count of zero", () => {
    expect(ids([orgWith(counts({ function_failures: 1 }))])).toEqual([...BASE, ...FUNCTIONS]);
  });

  it("drops the error column when no app recorded a session with an error", () => {
    const orgs = [orgWith(counts({ sessions: 30 }))];
    expect(ids(orgs)).not.toContain("errorSessions");
    expect(hiddenLabels(orgs)).toContain("Sessions with an error");
  });

  it("shows the error column for an error in either week", () => {
    expect(ids([orgWith(counts({ error_sessions: 2 }))])).toEqual([...BASE, "errorSessions"]);
    expect(ids([orgWith(counts(), counts({ error_sessions: 2 }))])).toEqual([
      ...BASE,
      "errorSessions"
    ]);
  });

  it("decides the two groups independently", () => {
    // Functions without errors, and errors without functions: neither drags the other in.
    const functionsOnly = [orgWith(counts({ function_calls: 5 }))];
    expect(ids(functionsOnly)).toEqual([...BASE, ...FUNCTIONS]);
    expect(hiddenLabels(functionsOnly)).toContain("Sessions with an error");

    const errorsOnly = [orgWith(counts({ error_sessions: 1 }))];
    expect(ids(errorsOnly)).toEqual([...BASE, "errorSessions"]);
    expect(hiddenLabels(errorsOnly)).toContain("Function calls and failed calls");
  });
});

describe("usageColumns — releases", () => {
  it("is absent when no app had a release in either week", () => {
    const orgs = [orgWith(counts({ views: 40, function_calls: 9 }))];
    expect(ids(orgs)).not.toContain("releases");
    expect(hiddenLabels(orgs)).toContain("Releases");
  });

  it("is shown for a release this week", () => {
    const orgs = [orgWith(counts({ releases: 1 }))];
    expect(ids(orgs)).toEqual([...BASE, "releases"]);
    expect(hiddenLabels(orgs)).not.toContain("Releases");
  });

  it("is shown when the only release was the week before", () => {
    // Shipped last week, nothing this week: a real zero, like a function that stopped.
    expect(ids([orgWith(counts(), counts({ releases: 2 }))])).toEqual([...BASE, "releases"]);
  });

  it("is not brought in by anything else being busy", () => {
    const busy = orgWith(counts({ function_calls: 50, error_sessions: 4, views: 900 }));
    expect(ids([busy])).not.toContain("releases");
  });
});

describe("usageColumns — storage", () => {
  it("is absent when no app has a measurement at either end of the week", () => {
    const orgs = [
      orgStoring({ storage_bytes: null, storage_bytes_before: null }),
      // Busy in every other way. Storage is decided by measurements and nothing else.
      orgWith(counts({ views: 900, function_calls: 50, releases: 3 }))
    ];
    expect(ids(orgs)).not.toContain("storage");
    expect(usageColumns(orgs).hidden).toContainEqual({
      label: "Storage",
      verb: "is",
      why: "it has not been measured for any app"
    });
  });

  it("is shown once one app has been measured", () => {
    const orgs = [
      orgStoring({ storage_bytes: null, storage_bytes_before: null }),
      orgStoring({ storage_bytes: 5 * 1024 * 1024, storage_bytes_before: null })
    ];
    expect(ids(orgs)).toEqual([...BASE, "storage"]);
    expect(hiddenLabels(orgs)).not.toContain("Storage");
  });

  it("is shown for an app measured and found empty — 0 B is an answer", () => {
    // The rule is "was it measured", not "is it bigger than nothing". Testing for a size
    // above zero would hide the column for a fleet of apps that store no files, and the
    // footnote would then claim they had never been measured.
    expect(ids([orgStoring({ storage_bytes: 0, storage_bytes_before: null })])).toEqual([
      ...BASE,
      "storage"
    ]);
  });

  it("is shown when the only measurement is from the start of the week", () => {
    expect(ids([orgStoring({ storage_bytes: null, storage_bytes_before: 2048 })])).toEqual([
      ...BASE,
      "storage"
    ]);
    // Zero counts here too: measured at the start and found empty.
    expect(ids([orgStoring({ storage_bytes: null, storage_bytes_before: 0 })])).toEqual([
      ...BASE,
      "storage"
    ]);
  });
});

describe("what each column says", () => {
  const cell = (id: UsageColumnId, orgs: UsageOrg[]) => {
    const column = usageColumns(orgs).shown.find((c) => c.id === id);
    if (!column) throw new Error(`column ${id} is not shown`);
    return column;
  };

  const busy = app({
    current: counts({
      views: 1200,
      people: 9,
      function_calls: 40,
      function_failures: 3,
      error_sessions: 2,
      releases: 4
    }),
    previous: counts({ views: 900, people: 11, function_calls: 10, releases: 1 }),
    storage_bytes: 3.4 * 1024 ** 3,
    storage_bytes_before: 3.1 * 1024 ** 3
  });
  // The org's own figures are deliberately not what its one app adds up to, so a cell
  // that summed the app rows would read differently from one that used these.
  const rivermark = org({
    people: 12,
    prev_people: 9,
    views: 1500,
    prev_views: 1000,
    function_calls: 240,
    function_failures: 31,
    releases: 6,
    storage_bytes: 5 * 1024 ** 3,
    apps: [busy]
  });

  it("reads an app's row from this week, and its change against the week before", () => {
    expect(cell("people", [rivermark]).app(busy)).toBe("9");
    expect(cell("change", [rivermark]).app(busy)).toBe("−2");
    expect(cell("views", [rivermark]).app(busy)).toBe("1,200");
    expect(cell("functionCalls", [rivermark]).app(busy)).toBe("40");
    expect(cell("functionFailures", [rivermark]).app(busy)).toBe("3");
    expect(cell("errorSessions", [rivermark]).app(busy)).toBe("2");
    expect(cell("releases", [rivermark]).app(busy)).toBe("4");
  });

  it("shows an app's storage as its size at the end of the week", () => {
    expect(cell("storage", [rivermark]).app(busy)).toBe("3.4 GB");
  });

  it("reads an organization's row from its own figures, not a sum of its apps", () => {
    // Adding up the app rows would count someone who opened two apps twice, so the page
    // shows what the report gives for the organization and never its own arithmetic.
    expect(cell("people", [rivermark]).org?.(rivermark)).toBe("12");
    expect(cell("change", [rivermark]).org?.(rivermark)).toBe("+3");
    expect(cell("views", [rivermark]).org?.(rivermark)).toBe("1,500");
    expect(cell("functionCalls", [rivermark]).org?.(rivermark)).toBe("240");
    expect(cell("functionFailures", [rivermark]).org?.(rivermark)).toBe("31");
    expect(cell("releases", [rivermark]).org?.(rivermark)).toBe("6");
    expect(cell("storage", [rivermark]).org?.(rivermark)).toBe("5.0 GB");
  });

  it("has no organization figure for sessions with an error, because the report has none", () => {
    expect(cell("errorSessions", [rivermark]).org).toBeUndefined();
  });

  /**
   * "Not measured" and "0 B" are different facts: one app nobody has looked at, one app
   * looked at and found empty. A cell that printed `0 B` for both — or "not measured" for
   * both — would be wrong about one of them on every row it appears.
   */
  it("tells an app that was never measured from one measured at nothing", () => {
    const unmeasured = app({ app_id: "u", storage_bytes: null, storage_bytes_before: null });
    const empty = app({ app_id: "e", storage_bytes: 0, storage_bytes_before: null });
    const column = cell("storage", [org({ apps: [unmeasured, empty] })]);

    expect(column.app(unmeasured)).toBeNull();
    expect(column.app(empty)).toBe("0 B");
    // What the table prints, muted, in place of the missing figure.
    expect(column.missing).toBe("not measured");
  });

  it("says not measured for an app last measured at the start of the week", () => {
    // The column is about the end of the week. An older reading is why the column is
    // there, but it is not this row's figure.
    const lapsed = app({ storage_bytes: null, storage_bytes_before: 4096 });
    expect(cell("storage", [org({ apps: [lapsed] })]).app(lapsed)).toBeNull();
  });

  it("does the same for an organization none of whose apps was measured", () => {
    const measuredElsewhere = orgStoring({ storage_bytes: 2048, storage_bytes_before: null });
    const unmeasuredOrg = org({ org_id: "org-u", storage_bytes: null });
    const emptyOrg = org({ org_id: "org-e", storage_bytes: 0 });
    const column = cell("storage", [measuredElsewhere, unmeasuredOrg, emptyOrg]);

    expect(column.org?.(unmeasuredOrg)).toBeNull();
    expect(column.org?.(emptyOrg)).toBe("0 B");
  });

  it("has words for a missing figure only where a figure can be missing", () => {
    const columns = usageColumns([rivermark]).shown;
    expect(columns.filter((c) => c.missing !== undefined).map((c) => c.id)).toEqual(["storage"]);
  });
});
