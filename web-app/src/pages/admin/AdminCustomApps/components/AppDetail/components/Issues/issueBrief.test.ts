import { describe, expect, it } from "vitest";
import type { AppIssue } from "@/services/api/appIssues";
import { describeLastFailure, issueBrief, liveIssueCount } from "./issueBrief";

const INVOCATION = "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d";
const APP = { id: "app-id", org_slug: "acme", slug: "bookkeeping" };

function issue(overrides: Partial<AppIssue> = {}, last: Partial<AppIssue["last"]> = {}): AppIssue {
  return {
    function_name: "upload-report",
    fingerprint: "5c1e0b8a9d2f4e71",
    occurrences: 12,
    first_seen: "2026-10-01T08:00:00+00:00",
    last_seen: "2026-10-03T14:02:11+00:00",
    builds: 2,
    on_live_build: true,
    ...overrides,
    last: {
      invocation_id: INVOCATION,
      status: "error",
      result_status: null,
      error: "function threw: Error: warehouse insert failed",
      build_id: "b2",
      created_at: "2026-10-03T14:02:11+00:00",
      ...last
    }
  };
}

describe("liveIssueCount", () => {
  it("counts only the issues the live build has had", () => {
    // One last seen on a replaced build is history; a badge for history would
    // be lit on every app that ever had a bug.
    expect(
      liveIssueCount([
        issue(),
        issue({ on_live_build: false }),
        issue({ function_name: "sync", on_live_build: true })
      ])
    ).toBe(2);
    expect(liveIssueCount([])).toBe(0);
  });
});

describe("describeLastFailure", () => {
  it("is the error text when one was recorded", () => {
    expect(describeLastFailure(issue())).toBe("function threw: Error: warehouse insert failed");
  });

  it("says what is known for the failures that record no text", () => {
    // An empty box under a red row reads as the console having lost something.
    expect(describeLastFailure(issue({}, { status: "timeout", error: null }))).toMatch(
      /^Timed out\./
    );
    expect(
      describeLastFailure(issue({}, { status: "success", result_status: 502, error: null }))
    ).toMatch(/answered HTTP 502/);
    // `success` with nothing else: a 5xx whose status was not kept, or a ctx.*
    // call the handler caught. The row cannot tell which, so it names both.
    const counted = describeLastFailure(issue({}, { status: "success", error: null }));
    expect(counted).toMatch(/counted the call as failed/);
    expect(counted).toMatch(/5xx/);
    expect(counted).toMatch(/ctx\.\*/);
    expect(describeLastFailure(issue({}, { status: "cancelled", error: null }))).toBe(
      "Ended cancelled, with no error text recorded."
    );
  });

  it("does not read a kept 4xx as the failure", () => {
    // Only a 5xx is what the platform counts; a 404 beside a counted failure
    // is a caught ctx.* call, not the response.
    expect(
      describeLastFailure(issue({}, { status: "success", result_status: 404, error: null }))
    ).toMatch(/counted the call as failed/);
  });
});

describe("issueBrief", () => {
  it("carries what the row shows and how to fetch the evidence", () => {
    const brief = issueBrief(APP, issue(), 7);

    expect(brief).toContain("Custom app issue: acme/bookkeeping");
    expect(brief).toContain("Function:      upload-report");
    expect(brief).toContain("Fingerprint:   5c1e0b8a9d2f4e71");
    expect(brief).toContain("Occurrences:   12 in the last 7 days, on 2 builds");
    expect(brief).toContain("Live build:    has had it");
    expect(brief).toContain(`Last failure:  error, invocation ${INVOCATION}, build b2`);
    expect(brief).toContain("function threw: Error: warehouse insert failed");
    // The two reads, addressed the way each route is: logs by slug, the
    // invocation list by app id.
    expect(brief).toContain(
      `oxyc api "/api/customer-apps/acme/bookkeeping/logs?hours=168&invocation_id=${INVOCATION}"`
    );
    expect(brief).toContain(
      'oxyc api "/api/admin/apps/app-id/invocations?function=upload-report&limit=50"'
    );
  });

  it("says where a failure the live build has not had was last seen", () => {
    const brief = issueBrief(
      APP,
      issue({ on_live_build: false, builds: 1 }, { build_id: "b1" }),
      7
    );
    expect(brief).toContain("Live build:    has not had it (last seen on build b1)");
    expect(brief).toContain("on 1 build");
  });

  it("keeps a kept status and an odd function name intact", () => {
    const brief = issueBrief(
      APP,
      issue(
        { function_name: "reports/daily sync" },
        { status: "success", result_status: 500, error: null }
      ),
      7
    );
    expect(brief).toContain("Last failure:  success, HTTP 500");
    expect(brief).toContain("invocations?function=reports%2Fdaily%20sync&limit=50");
  });
});
