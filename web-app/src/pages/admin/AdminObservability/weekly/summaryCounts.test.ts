import { describe, expect, it } from "vitest";
import { type SummaryCountId, summaryCounts } from "./summaryCounts";
import { summary } from "./testFixtures";

/** The line as it reads, one item per entry: "1,318 app opens". */
const line = (over: Parameters<typeof summary>[0] = {}): string[] =>
  summaryCounts(summary(over)).map((c) => `${c.value} ${c.label}`);

const ids = (over: Parameters<typeof summary>[0] = {}): SummaryCountId[] =>
  summaryCounts(summary(over)).map((c) => c.id);

const GB = 1024 ** 3;

describe("summaryCounts", () => {
  it("is the three traffic counts when nothing else happened", () => {
    // No calls, no releases, nothing measured: the line does not grow three zeroes.
    expect(line()).toEqual([
      "1,318 app opens",
      "9 of 12 apps in use",
      "4 of 5 organizations active"
    ]);
  });

  it("adds function calls, releases and storage, in that order, after them", () => {
    expect(
      line({ function_calls: 1240, function_failures: 31, releases: 4, storage_bytes: 3.4 * GB })
    ).toEqual([
      "1,318 app opens",
      "9 of 12 apps in use",
      "4 of 5 organizations active",
      "1,240 function calls, 31 failed",
      "4 releases",
      "3.4 GB stored"
    ]);
  });

  it("keeps the three traffic counts even at zero — that is the finding", () => {
    expect(line({ views: 0, active_apps: 0, active_orgs: 0 })).toEqual([
      "0 app opens",
      "0 of 12 apps in use",
      "0 of 5 organizations active"
    ]);
  });

  it("uses the singular for one of something", () => {
    expect(
      line({
        views: 1,
        apps: 1,
        active_apps: 1,
        orgs: 1,
        active_orgs: 1,
        function_calls: 1,
        releases: 1
      })
    ).toEqual([
      "1 app open",
      "1 of 1 app in use",
      "1 of 1 organization active",
      "1 function call",
      "1 release"
    ]);
  });
});

describe("summaryCounts — function calls", () => {
  it("is left out when no function was called", () => {
    expect(ids({ function_calls: 0, function_failures: 0 })).not.toContain("functions");
  });

  it("does not mention failures when none failed", () => {
    expect(line({ function_calls: 1240 })).toContain("1,240 function calls");
    expect(line({ function_calls: 1240 }).join(" ")).not.toContain("failed");
  });

  it("appends the failures when any call failed", () => {
    expect(line({ function_calls: 1240, function_failures: 31 })).toContain(
      "1,240 function calls, 31 failed"
    );
    expect(line({ function_calls: 5000, function_failures: 1200 })).toContain(
      "5,000 function calls, 1,200 failed"
    );
  });

  it("never drops a failure, even beside a call count of zero", () => {
    // The two should not disagree like this. If they do, the failure is the part to show.
    expect(line({ function_calls: 0, function_failures: 2 })).toContain(
      "0 function calls, 2 failed"
    );
  });
});

describe("summaryCounts — releases", () => {
  it("is left out when there were none, rather than reading '0 releases'", () => {
    expect(ids({ releases: 0 })).not.toContain("releases");
    expect(line({ releases: 0 }).join(" ")).not.toContain("release");
  });

  it("is shown when there was one", () => {
    expect(line({ releases: 4 })).toContain("4 releases");
  });
});

describe("summaryCounts — storage", () => {
  it("is left out when nothing was measured", () => {
    expect(ids({ storage_bytes: null })).not.toContain("storage");
  });

  it("is left out at zero too", () => {
    expect(ids({ storage_bytes: 0 })).not.toContain("storage");
    expect(line({ storage_bytes: 0 }).join(" ")).not.toContain("stored");
  });

  it("is shown as a size when there is one", () => {
    expect(line({ storage_bytes: 3.4 * GB })).toContain("3.4 GB stored");
    expect(line({ storage_bytes: 12 * 1024 })).toContain("12 KB stored");
  });

  it("is the size at the end of the week, whatever it was at the start", () => {
    expect(line({ storage_bytes: 2 * GB, storage_bytes_before: 9 * GB })).toContain(
      "2.0 GB stored"
    );
    // Measured at the start only: there is no end-of-week size to state.
    expect(ids({ storage_bytes: null, storage_bytes_before: 9 * GB })).not.toContain("storage");
  });
});

describe("summaryCounts — each optional item stands alone", () => {
  it("shows any one of them without the others", () => {
    expect(ids({ function_calls: 3 })).toEqual(["views", "apps", "orgs", "functions"]);
    expect(ids({ releases: 3 })).toEqual(["views", "apps", "orgs", "releases"]);
    expect(ids({ storage_bytes: 3 })).toEqual(["views", "apps", "orgs", "storage"]);
  });
});
