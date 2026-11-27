/**
 * The `loads` verdict: one case per rule, and the detail the console renders.
 */

import { describe, expect, it } from "vitest";
import {
  newObserved,
  type Observed,
  pushFailed,
  pushOffOrigin,
  pushPageError,
  pushRefused,
  scoreLoads,
  verdict
} from "./loads-score.js";
import { loadsTarget, VIEWPORTS } from "./loads-target.js";

const [DESK, PHONE] = VIEWPORTS;
const URL_ = "https://app.example.com/customer-apps/acme/store/";

const TARGET = loadsTarget({
  pageUrl: URL_,
  deployment: "https://app.example.com",
  credential: { "x-api-key": "oxysvc_x" },
  appId: "00000000-0000-0000-0000-0000000000a1",
  environment: "staging",
  expectedBuild: "b1"
});

/** A page that mounted on staging's build. */
function ready(overrides: Partial<Observed> = {}): Observed {
  return {
    ...newObserved(),
    readyMs: 1200,
    app: { environment: "staging", buildId: "b1" },
    ...overrides
  };
}

describe("verdict", () => {
  it("passes a ready page serving the build", () => {
    expect(verdict(TARGET, ready())).toBeUndefined();
  });

  it("fails on a navigation error", () => {
    expect(verdict(TARGET, ready({ navigationError: "the page answered 502" }))).toBe(
      "the page answered 502"
    );
  });

  it("fails a page another environment served", () => {
    expect(verdict(TARGET, ready({ app: { environment: "production", buildId: "b0" } }))).toBe(
      "the page was served by production, not staging: the environment header did not reach the document"
    );
  });

  it("fails a page served another build, or none", () => {
    expect(verdict(TARGET, ready({ app: { environment: "staging", buildId: "b0" } }))).toBe(
      "the page was served build b0, but staging serves b1"
    );
    expect(verdict(TARGET, ready({ app: { environment: "staging", buildId: null } }))).toBe(
      "the page was served build (none), but staging serves b1"
    );
  });

  it("fails a page that never sent the ready beacon", () => {
    expect(verdict(TARGET, ready({ readyMs: null }))).toBe(
      "the app never sent oxy-app-ready within 30s — a blank page, a crash on boot, or a sign-in page"
    );
  });

  it("fails on an uncaught page error, naming the first", () => {
    const threw = ready();
    pushPageError(threw, "TypeError: x is undefined");
    pushPageError(threw, "RangeError: later");
    expect(verdict(TARGET, threw)).toBe("uncaught page error: TypeError: x is undefined");
  });

  it("fails a ready page that carries no window.__OXY_APP__", () => {
    expect(verdict(TARGET, ready({ app: undefined }))).toBe(
      "the page carries no window.__OXY_APP__"
    );
  });

  it("names the first failure found, in the order of the rules", () => {
    const everything = ready({
      navigationError: "the page answered 502",
      readyMs: null,
      app: { environment: "production", buildId: "b0" },
      pageErrors: ["boom"]
    });
    expect(verdict(TARGET, everything)).toBe("the page answered 502");
    expect(verdict(TARGET, { ...everything, navigationError: undefined })).toMatch(
      /^the page was served by production/
    );
    expect(
      verdict(TARGET, {
        ...everything,
        navigationError: undefined,
        app: { environment: "staging", buildId: "b0" }
      })
    ).toMatch(/^the page was served build b0/);
    expect(
      verdict(TARGET, {
        ...everything,
        navigationError: undefined,
        app: { environment: "staging", buildId: "b1" }
      })
    ).toMatch(/^the app never sent oxy-app-ready/);
  });

  it("says which calls went without the credential when a page fails", () => {
    const denied = [
      "/api/sql/query answered 401",
      "/api/sql/query answered 401",
      "/api/orgs answered 403",
      "/api/a answered 401",
      "/api/b answered 401"
    ];
    const why = verdict(TARGET, ready({ navigationError: "the page answered 502", denied }));
    expect(why).toBe(
      "the page answered 502; the page's /api/sql/query answered 401, /api/orgs answered 403, /api/a answered 401 — the check sends no credential outside the app's base path (/customer-apps/acme/store/), so the app's own /api calls run as an anonymous visitor's"
    );
    // A page that passes anyway passes, with nothing added.
    expect(verdict(TARGET, ready({ denied }))).toBeUndefined();
  });
});

describe("scoreLoads", () => {
  it("passes both viewports and says how long each took", () => {
    const out = scoreLoads(
      TARGET,
      [
        [DESK, ready()],
        [PHONE, ready({ readyMs: 1449 })]
      ],
      "app_checker"
    );
    expect(out.status).toBe("passed");
    expect(out.summary).toBe("ready in 1.2s desk, 1.4s phone");
    expect(out.detail).toEqual({
      findings: [],
      url: URL_,
      identity: "app_checker",
      viewports: [
        {
          name: "desk",
          width: 1440,
          height: 900,
          ready: true,
          ready_ms: 1200,
          environment: "staging",
          build_id: "b1",
          page_errors: [],
          failed_requests: [],
          refused_writes: [],
          refused_off_origin: []
        },
        {
          name: "phone",
          width: 390,
          height: 844,
          ready: true,
          ready_ms: 1449,
          environment: "staging",
          build_id: "b1",
          page_errors: [],
          failed_requests: [],
          refused_writes: [],
          refused_off_origin: []
        }
      ]
    });
  });

  it("fails on the first viewport that failed, and keeps the other's evidence", () => {
    const out = scoreLoads(
      TARGET,
      [
        [DESK, ready()],
        [PHONE, ready({ readyMs: null, app: undefined })]
      ],
      "app_checker"
    );
    expect(out.status).toBe("failed");
    expect(out.summary).toMatch(/^phone: the app never sent oxy-app-ready within 30s/);
    expect(out.detail.findings).toEqual([
      { severity: "error", message: out.summary, at: null, rule: null }
    ]);
    const viewports = out.detail.viewports as { ready: boolean; ready_ms: number | null }[];
    expect(viewports.map((v) => [v.ready, v.ready_ms])).toEqual([
      [true, 1200],
      [false, null]
    ]);
  });

  it("records failed requests as warnings without failing a ready page", () => {
    const page = ready();
    pushFailed(page, { url: `${URL_}fn/list`, status: 500, error: null });
    pushFailed(page, {
      url: "https://cdn.example.com/a.js",
      status: null,
      error: "net::ERR_FAILED"
    });
    pushFailed(page, { url: "https://cdn.example.com/b.js", status: null, error: null });
    const out = scoreLoads(
      TARGET,
      [
        [DESK, page],
        [PHONE, ready()]
      ],
      "app_checker"
    );
    expect(out.status).toBe("passed");
    expect(out.detail.findings).toEqual([
      {
        severity: "warning",
        message: "desk: a request answered 500",
        at: `${URL_}fn/list`,
        rule: null
      },
      {
        severity: "warning",
        message: "desk: a request net::ERR_FAILED",
        at: "https://cdn.example.com/a.js",
        rule: null
      },
      {
        severity: "warning",
        message: "desk: a request failed",
        at: "https://cdn.example.com/b.js",
        rule: null
      }
    ]);
  });

  it("records a write the check refused as a warning, once per distinct request", () => {
    const page = ready();
    pushRefused(page, "POST /customer-apps/acme/store/fn/save");
    pushRefused(page, "POST /customer-apps/acme/store/fn/save");
    expect(page.refused).toHaveLength(1);
    const out = scoreLoads(TARGET, [[DESK, page]], "app_checker");
    expect(out.status).toBe("passed");
    expect(out.detail.findings).toEqual([
      {
        severity: "warning",
        message:
          "desk: not sent: POST /customer-apps/acme/store/fn/save (the checker reads, and sends no write)",
        at: null,
        rule: "write-refused"
      }
    ]);
    const [desk] = out.detail.viewports as { refused_writes: string[] }[];
    expect(desk?.refused_writes).toEqual(["POST /customer-apps/acme/store/fn/save"]);
  });

  it("records a request to another origin the check did not send as a warning of its own, never a failure", () => {
    const font = "GET https://fonts.example.com/a.woff2 (another origin)";
    const page = ready();
    pushOffOrigin(page, font);
    pushOffOrigin(page, font);
    expect(page.offOrigin).toHaveLength(1);
    const out = scoreLoads(TARGET, [[DESK, page]], "app_checker");
    // The page came up without its font: that is a passed load, and a warning.
    expect(out.status).toBe("passed");
    expect(out.detail.findings).toEqual([
      {
        severity: "warning",
        message: `desk: not sent: ${font} — a checked page reaches only the app's own origin`,
        at: null,
        rule: "off-origin-refused"
      }
    ]);
    const [desk] = out.detail.viewports as Record<string, string[]>[];
    expect(desk?.refused_off_origin).toEqual([font]);
    expect(desk?.refused_writes).toEqual([]);
  });

  it("names what was not sent when the page that asked for it did not come up", () => {
    const script = "GET https://cdn.example.com/lib.js (another origin)";
    const why = `; not sent: ${script} — a checked page reaches only the app's own origin (https://app.example.com)`;
    // An app that mounts only with a script of another origin: the check refused it, and says so.
    expect(verdict(TARGET, ready({ readyMs: null, offOrigin: [script] }))).toBe(
      `the app never sent oxy-app-ready within 30s — a blank page, a crash on boot, or a sign-in page${why}`
    );
    expect(
      verdict(
        TARGET,
        ready({ pageErrors: ["ReferenceError: lib is not defined"], offOrigin: [script] })
      )
    ).toBe(`uncaught page error: ReferenceError: lib is not defined${why}`);
    // What the page asked for cannot explain a document that never answered, or another build's.
    expect(
      verdict(TARGET, ready({ navigationError: "the page answered 502", offOrigin: [script] }))
    ).toBe("the page answered 502");
    expect(
      verdict(
        TARGET,
        ready({ app: { environment: "staging", buildId: "b0" }, offOrigin: [script] })
      )
    ).toBe("the page was served build b0, but staging serves b1");
  });

  it("keeps what a page printed short, and only so much of it", () => {
    const page = ready();
    for (let i = 0; i < 30; i += 1) {
      pushPageError(page, `${i}: ${"e".repeat(1000)}`);
      pushFailed(page, {
        url: `https://x.example.com/${"u".repeat(1000)}`,
        status: 404,
        error: null
      });
      pushRefused(page, `POST /customer-apps/acme/store/fn/f${i}`);
    }
    expect(page.pageErrors).toHaveLength(20);
    expect(page.failedRequests).toHaveLength(20);
    expect(page.refused).toHaveLength(20);
    expect(Array.from(page.pageErrors[0] ?? "")).toHaveLength(400);
    expect(page.pageErrors[0]?.endsWith("…")).toBe(true);
    expect(Array.from(page.failedRequests[0]?.url ?? "")).toHaveLength(400);
    // Each viewport: one error, 20 failed requests, 20 refused writes. More than a
    // check reports, so the rest are counted.
    const out = scoreLoads(
      TARGET,
      [
        [DESK, page],
        [PHONE, { ...page }]
      ],
      "app_checker"
    );
    expect(out.detail.findings).toHaveLength(50);
    expect(out.detail.findings_omitted).toBe(2 * 41 - 50);
    expect(Array.from(out.summary)).toHaveLength(200);
  });
});
