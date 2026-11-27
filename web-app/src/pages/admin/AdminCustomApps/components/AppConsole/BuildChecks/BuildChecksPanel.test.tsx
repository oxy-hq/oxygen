// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheck, BuildChecks } from "@/types/appBuildChecks";
import type { AppBuild } from "@/types/apps";
import { BuildChecksPanel } from ".";

const rerun = vi.fn();
let checks: BuildChecks | undefined;
let rerunState: { isError: boolean; variables?: string; error?: unknown } = { isError: false };
const asked: (string | undefined)[] = [];

vi.mock("@/hooks/api/customApps/useBuildChecks", () => ({
  useBuildChecks: (_app: string, build: string | undefined) => {
    asked.push(build);
    return { isPending: false, isError: false, data: checks };
  },
  useRerunBuildChecks: () => ({ mutate: rerun, isPending: false, ...rerunState })
}));

const STAGING: AppBuild = {
  id: "row-s",
  build_id: "9087d7b1c2d3e4f5",
  created_at: "2026-11-20T10:00:00Z",
  is_draft: true,
  is_published: false
};
const LIVE: AppBuild = {
  ...STAGING,
  id: "row-p",
  build_id: "60e2f19aaaaaaaaa",
  is_draft: false,
  is_published: true
};

vi.mock("@/hooks/api/customApps/useAppBuilds", () => ({
  useAppBuilds: () => ({ isPending: false, isError: false, data: { builds: [STAGING, LIVE] } })
}));
vi.mock("@/hooks/api/customApps/useAppFunctions", () => ({
  useInvocationCalls: () => ({ isPending: true, isError: false, data: undefined })
}));

const row = (over: Partial<BuildCheck>): BuildCheck => ({
  kind: "validation",
  label: "Builds and validates",
  status: "passed",
  advisory: false,
  summary: null,
  detail: { findings: [] },
  notes: [],
  attempt: 2,
  run_id: "r",
  created_at: "2026-11-20T10:00:00Z",
  started_at: null,
  finished_at: null,
  duration_ms: 1200,
  ...over
});

/** The attempt was queued at 10:00:00; the runner answered 19 seconds later. */
const RUNNER = {
  label: "GitHub Actions run #1482",
  url: "https://github.com/oxy-hq/oxygen-internal/actions/runs/123",
  started_at: "2026-11-20T10:00:19Z"
};

const FAILED: BuildChecks = {
  app_id: "app-1",
  build: { id: "row-s", build_id: "9087d7b1c2d3e4f5", created_at: "2026-11-20T10:00:00Z" },
  attempt: 2,
  environment: "staging",
  state: "failed",
  counts: { total: 4, queued: 0, running: 0, passed: 3, failed: 1, error: 0, skipped: 0 },
  checks: [
    row({
      summary: "bundle valid",
      detail: { findings: [], validation_status: "passed", validation_detail: null }
    }),
    row({
      kind: "functions",
      label: "Its checks pass",
      summary: "1 of 2 failed",
      status: "failed",
      detail: {
        findings: [],
        functions: [
          {
            name: "reorder_math",
            run_id: "f1",
            status: "done",
            passed: true,
            error: null,
            duration_ms: 1200,
            invocation_id: "iv-1"
          },
          {
            name: "digest_render",
            run_id: "f2",
            status: "timed_out",
            passed: false,
            error: "no answer in 30s",
            duration_ms: null,
            invocation_id: null
          }
        ]
      }
    }),
    row({
      kind: "loads",
      label: "It loads, desk and phone",
      summary: "phone never ready",
      detail: {
        findings: [],
        url: "https://acme--store.customer-apps.example/",
        identity: "service:app-checks",
        viewports: [
          {
            name: "desk",
            width: 1440,
            height: 900,
            ready: true,
            ready_ms: 1200,
            environment: "staging",
            build_id: "9087d7b",
            page_errors: [],
            failed_requests: []
          },
          {
            name: "phone",
            width: 390,
            height: 844,
            ready: false,
            ready_ms: null,
            environment: null,
            build_id: null,
            page_errors: ["TypeError: x is undefined"],
            failed_requests: [{ url: "https://acme/api/stores", status: 500, error: null }]
          }
        ]
      }
    }),
    row({
      kind: "design_system",
      label: "Follows the design system",
      status: "failed",
      summary: "restates --card",
      detail: {
        findings: [
          {
            severity: "error",
            rule: "restates-sdk-token",
            at: "assets/index-abc.css:1",
            message: "restates --card, which theme.css defines; remove it"
          }
        ],
        sdk_version: "2.29.0",
        sdk_floor: "2.30.0",
        files: ["index.html", "assets/index-abc.css"]
      }
    }),
    // A kind a later phase adds: shown by its label and findings, never dropped.
    row({
      kind: "screens",
      label: "Every screen opens",
      status: "passed",
      advisory: true,
      summary: "2 screens differ",
      detail: {
        findings: [{ severity: "warning", message: "2 screens differ", at: null, rule: null }]
      },
      notes: [
        {
          author: "model",
          model: "claude-test",
          text: "The header is 2px shorter.",
          refs: [{ kind: "file", label: "src/index.css:68", href: null }],
          created_at: "2026-11-20T10:01:00Z"
        }
      ]
    })
  ],
  summary: {
    author: "model",
    model: "claude-test",
    text: "One likely regression, on the store dialog.",
    refs: [],
    created_at: "2026-11-20T10:02:00Z"
  },
  attempts: [
    { attempt: 2, state: "failed", created_at: "2026-11-20T10:00:00Z" },
    { attempt: 1, state: "failed", created_at: "2026-11-20T09:00:00Z" }
  ],
  runner: RUNNER,
  stalled: null
};

const KINDS = ["validation", "functions", "loads", "design_system"];

/** An attempt of four plain rows with these statuses, as the server answers it. */
const attempt = (
  state: BuildChecks["state"],
  statuses: string[],
  over: Partial<BuildChecks> = {}
): BuildChecks => ({
  ...FAILED,
  state,
  checks: statuses.map((status, i) => row({ kind: KINDS[i], label: KINDS[i], status })),
  summary: null,
  attempts: [{ attempt: 2, state, created_at: "2026-11-20T10:00:00Z" }],
  ...over
});

/** What the runner left on each row when it never answered: the server's own words. */
const NO_ANSWER = attempt("failed", ["error", "error", "error", "error"], {
  runner: null,
  stalled: "no_runner"
});
NO_ANSWER.checks = NO_ANSWER.checks.map((c) => ({ ...c, summary: "the runner did not answer" }));

afterEach(() => {
  cleanup();
  rerun.mockReset();
  checks = undefined;
  rerunState = { isError: false };
  asked.length = 0;
});

const renderPanel = (requested: string | null = null, onShowStaging = vi.fn()) =>
  render(<BuildChecksPanel appId='app-1' requested={requested} onShowStaging={onShowStaging} />);

/** The strip as `[state, label, value]` per step, in the order drawn. */
const strip = () =>
  within(screen.getByTestId("build-checks-progress"))
    .getAllByRole("listitem")
    .map((li) => [li.dataset.state, ...Array.from(li.children, (el) => el.textContent?.trim())]);

/** Whether `a` is drawn before `b`. */
const precedes = (a: HTMLElement, b: HTMLElement) =>
  (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;

const rerunButton = () => screen.getByTestId("build-checks-rerun");
const isPrimary = (button: HTMLElement) => button.className.split(/\s+/).includes("bg-primary");

describe("BuildChecksPanel", () => {
  it("lists every check of staging's build, in the server's order", () => {
    checks = FAILED;
    renderPanel();
    // Read by the build's row id, so the card and this panel share one read.
    expect(asked).toContain("row-s");
    expect(screen.getByTestId("build-checks-build").textContent).toContain("9087d7b");
    expect(screen.getByTestId("build-checks-build").textContent).toContain("attempt 2");

    const kinds = within(screen.getByTestId("build-checks"))
      .getAllByRole("listitem")
      .filter((li) => li.dataset.testid?.startsWith("build-check-") && li.dataset.status)
      .map((li) => li.dataset.testid);
    expect(kinds).toEqual([
      "build-check-validation",
      "build-check-functions",
      "build-check-loads",
      "build-check-design_system",
      "build-check-screens"
    ]);
  });

  it("opens a failed check to its evidence, and leaves a passed one closed", () => {
    checks = FAILED;
    renderPanel();
    const design = screen.getByTestId("build-check-design_system");
    expect(design.textContent).toContain("Follows the design system");
    expect(design.textContent).toContain("restates --card");
    // Failed rows start open, with the theme guard's finding as `file:line rule: message`.
    expect(
      screen.getByTestId("build-check-design_system-toggle").getAttribute("aria-expanded")
    ).toBe("true");
    expect(within(design).getByTestId("build-check-finding-text").textContent).toBe(
      "assets/index-abc.css:1 restates-sdk-token: restates --card, which theme.css defines; remove it"
    );
    expect(within(design).getByTestId("build-check-sdk-version").dataset.belowFloor).toBe("true");
    // A passed row stays closed.
    expect(screen.getByTestId("build-check-loads-toggle").getAttribute("aria-expanded")).toBe(
      "false"
    );
    expect(screen.queryByTestId("build-check-evidence-loads")).toBeNull();
  });

  it("shows each function's verdict, and opens the one with an invocation", () => {
    checks = FAILED;
    renderPanel();
    const passed = screen.getByTestId("build-check-function-reorder_math");
    const failed = screen.getByTestId("build-check-function-digest_render");
    expect(passed.textContent).toContain("passed");
    expect(passed.textContent).toContain("1.2s");
    expect(failed.textContent).toContain("timed out");
    expect(failed.textContent).toContain("no answer in 30s");
    expect(within(failed).queryByRole("button")).toBeNull();
    fireEvent.click(within(passed).getByRole("button"));
    expect(within(passed).getByTestId("admin-app-invocation-calls")).toBeTruthy();
  });

  it("opens the load check to one block per viewport", () => {
    checks = FAILED;
    renderPanel();
    fireEvent.click(screen.getByTestId("build-check-loads-toggle"));
    expect(screen.getByTestId("build-check-viewport-desk").textContent).toContain("Desk 1440");
    expect(screen.getByTestId("build-check-viewport-desk").textContent).toContain("ready in 1.2s");
    const phone = screen.getByTestId("build-check-viewport-phone");
    expect(phone.textContent).toContain("never ready");
    expect(phone.textContent).toContain("TypeError: x is undefined");
    expect(within(phone).getByTestId("build-check-failed-request").textContent).toContain("500");
  });

  it("renders an unknown kind by label and findings, tags only advisory rows, and shows notes", () => {
    checks = FAILED;
    renderPanel();
    expect(screen.getAllByTestId("build-check-informs")).toHaveLength(1);
    expect(
      within(screen.getByTestId("build-check-screens")).getByTestId("build-check-informs")
    ).toBeTruthy();
    expect(screen.getByTestId("build-checks-summary").textContent).toContain(
      "One likely regression"
    );
    fireEvent.click(screen.getByTestId("build-check-screens-toggle"));
    const screens = screen.getByTestId("build-check-evidence-screens");
    expect(within(screens).getByTestId("build-check-finding-text").textContent).toBe(
      "2 screens differ"
    );
    expect(within(screens).getByTestId("build-check-note").textContent).toContain(
      "src/index.css:68"
    );
  });

  it("never says a check holds Promote", () => {
    checks = FAILED;
    renderPanel();
    for (const kind of ["validation", "loads", "screens"]) {
      fireEvent.click(screen.getByTestId(`build-check-${kind}-toggle`));
    }
    const text = (document.body.textContent ?? "").toLowerCase();
    expect(text).not.toMatch(/required|blocked|waiting on/);
  });

  it("says a build published before checks existed was not checked", () => {
    checks = {
      ...FAILED,
      state: "none",
      attempt: null,
      checks: [],
      summary: null,
      attempts: [],
      runner: null
    };
    renderPanel();
    expect(screen.getByTestId("build-checks-none").textContent).toBe("Not checked.");
    // Nothing was queued, so there is no run to show the progress of.
    expect(screen.queryByTestId("build-checks-progress")).toBeNull();
    expect(screen.queryByTestId("build-checks-when")).toBeNull();
  });

  it("draws the run as four steps above the rows: a glyph, one word, one value", () => {
    // The last verdict came 1 minute 52 seconds after the attempt was queued.
    checks = {
      ...FAILED,
      checks: FAILED.checks.map((c) => ({ ...c, finished_at: "2026-11-20T10:01:52Z" }))
    };
    renderPanel();
    // Per step: the glyph (no text), the word, what a screen reader hears, the value.
    expect(strip()).toEqual([
      ["done", "", "Queued", "done", "0:00"],
      ["done", "", "Runner", "done", "0:19"],
      ["done", "", "Checking", "done", "4 of 4"],
      ["failed", "", "Done", "failed", "1:52"]
    ]);
    expect(
      precedes(screen.getByTestId("build-checks-progress"), screen.getByTestId("build-checks"))
    ).toBe(true);
    // The glyphs are the rows' own: a check when done, an x when failed.
    const glyph = (key: string) =>
      screen.getByTestId(`build-checks-step-${key}`).querySelector("svg")?.getAttribute("class");
    expect(glyph("runner")).toContain("lucide-circle-check");
    expect(glyph("runner")).toContain(ADMIN_TONE.ok.text);
    expect(glyph("done")).toContain("lucide-circle-x");
    expect(glyph("done")).toContain(ADMIN_TONE.danger.text);
  });

  it("shows a run nobody has answered yet as starting, with no runner to name", () => {
    checks = attempt("running", ["queued", "queued", "queued", "queued"], { runner: null });
    renderPanel();
    expect(strip()).toEqual([
      ["done", "", "Queued", "done", "0:00"],
      ["active", "", "Runner", "in progress", "starting"],
      ["todo", "", "Checking", "to do", ""],
      ["todo", "", "Done", "to do", ""]
    ]);
    const runner = screen.getByTestId("build-checks-step-runner");
    expect(runner.getAttribute("aria-current")).toBe("step");
    // The spinner stops for someone who asked for less motion.
    const spinner = runner.querySelector("svg")?.getAttribute("class") ?? "";
    expect(spinner).toContain("animate-spin");
    expect(spinner).toContain("motion-reduce:animate-none");
    expect(screen.queryByTestId("build-checks-runner")).toBeNull();
    expect(screen.getByTestId("build-checks-when").textContent).toContain("queued");
    expect(isPrimary(rerunButton())).toBe(false);
  });

  it("counts the verdicts in while the runner checks", () => {
    checks = attempt("running", ["passed", "running", "running", "passed"]);
    renderPanel();
    expect(strip().map(([state, , label, , value]) => [state, label, value])).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", "0:19"],
      ["active", "Checking", "2 of 4"],
      ["todo", "Done", ""]
    ]);
    expect(screen.getByTestId("build-checks-when").textContent).toContain("started");
  });

  it("names the runner in the heading, linked to its run in a new tab", () => {
    checks = FAILED;
    renderPanel();
    const link = screen.getByTestId("build-checks-runner-link") as HTMLAnchorElement;
    expect(link.textContent).toBe("GitHub Actions run #1482");
    expect(link.getAttribute("href")).toBe(RUNNER.url);
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noreferrer");
    expect(screen.getByTestId("build-checks-build").contains(link)).toBe(true);
  });

  it("names a runner that has no run to link to as plain text", () => {
    checks = { ...FAILED, runner: { ...RUNNER, label: "oxyc, by hand", url: null } };
    renderPanel();
    expect(screen.getByTestId("build-checks-runner").textContent).toBe("· oxyc, by hand");
    expect(screen.queryByTestId("build-checks-runner-link")).toBeNull();
    expect(screen.getByTestId("build-checks-build").querySelector("a")).toBeNull();
  });

  it("never makes a runner's URL clickable unless it is a web address", () => {
    checks = { ...FAILED, runner: { ...RUNNER, url: "javascript:alert(1)" } };
    renderPanel();
    expect(screen.getByTestId("build-checks-runner").textContent).toBe(
      "· GitHub Actions run #1482"
    );
    expect(screen.queryByTestId("build-checks-runner-link")).toBeNull();
  });

  it("says the runner never answered, and makes Run again the primary action", () => {
    checks = NO_ANSWER;
    renderPanel();
    const line = screen.getByTestId("build-checks-stalled");
    expect(line.textContent).toBe("The runner did not answer. Nothing was checked.");
    expect(line.className).toContain(ADMIN_TONE.danger.text);
    expect(precedes(line, screen.getByTestId("build-checks"))).toBe(true);
    expect(strip().map(([state, , label, , value]) => [state, label, value])).toEqual([
      ["done", "Queued", "0:00"],
      ["failed", "Runner", "no answer"],
      ["todo", "Checking", ""],
      ["todo", "Done", ""]
    ]);
    expect(screen.queryByTestId("build-checks-runner")).toBeNull();
    expect(screen.getByTestId("build-checks-when").textContent).toContain("queued");

    expect(isPrimary(rerunButton())).toBe(true);
    fireEvent.click(rerunButton());
    expect(rerun).toHaveBeenCalledWith("row-s");
  });

  it("leaves the rows of a run nobody answered as the server sent them, closed", () => {
    checks = NO_ANSWER;
    renderPanel();
    // The rows are the server's: `error`, each with the summary it was sent.
    for (const kind of KINDS) {
      // By its toggle: an open row's evidence block shares the row's own testid.
      const toggle = screen.getByTestId(`build-check-${kind}-toggle`);
      const li = toggle.closest("li");
      expect(li?.dataset.status).toBe("error");
      expect(li?.textContent).toContain("the runner did not answer");
      expect(li?.textContent).toContain("could not run");
      // Nothing ran, so there is no evidence to open to: the rows start closed.
      expect(toggle.getAttribute("aria-expanded")).toBe("false");
      expect(screen.queryByTestId(`build-check-evidence-${kind}`)).toBeNull();
    }
  });

  it("says a runner that stopped part-way did, and ends the strip at Checking", () => {
    checks = attempt("failed", ["passed", "error", "error", "passed"], {
      stalled: "runner_stopped"
    });
    renderPanel();
    expect(screen.getByTestId("build-checks-stalled").textContent).toBe(
      "The runner stopped before it finished."
    );
    expect(strip().map(([state, , label, , value]) => [state, label, value])).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", "0:19"],
      ["failed", "Checking", "stopped"],
      ["todo", "Done", ""]
    ]);
    expect(isPrimary(rerunButton())).toBe(true);
    // The runner that stopped is still the one to open.
    expect(screen.getByTestId("build-checks-runner-link").getAttribute("href")).toBe(RUNNER.url);
    expect(screen.getByTestId("build-check-functions-toggle").getAttribute("aria-expanded")).toBe(
      "false"
    );
  });

  it("still opens a check that failed before the runner stopped", () => {
    checks = attempt("failed", ["passed", "error", "failed", "passed"], {
      stalled: "runner_stopped"
    });
    renderPanel();
    expect(screen.getByTestId("build-check-loads-toggle").getAttribute("aria-expanded")).toBe(
      "true"
    );
    expect(screen.getByTestId("build-check-functions-toggle").getAttribute("aria-expanded")).toBe(
      "false"
    );
  });

  it("keeps Run again quiet when the checks ran, whatever they found", () => {
    checks = FAILED;
    renderPanel();
    expect(screen.queryByTestId("build-checks-stalled")).toBeNull();
    expect(isPrimary(rerunButton())).toBe(false);
  });

  it("reads an older server's answer, which names no runner and no stall", () => {
    const { runner: _runner, stalled: _stalled, ...older } = FAILED;
    checks = older;
    renderPanel();
    expect(screen.getByTestId("build-checks")).toBeTruthy();
    expect(strip().map(([state, , label, , value]) => [state, label, value])).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", ""],
      ["done", "Checking", "4 of 4"],
      ["failed", "Done", ""]
    ]);
    expect(screen.queryByTestId("build-checks-runner")).toBeNull();
    expect(screen.queryByTestId("build-checks-stalled")).toBeNull();
    expect(isPrimary(rerunButton())).toBe(false);
    expect(screen.getByTestId("build-checks-when").textContent).toContain("checked");
  });

  it("runs the shown build's checks again, and says a refusal inline", () => {
    checks = FAILED;
    renderPanel();
    fireEvent.click(screen.getByTestId("build-checks-rerun"));
    expect(rerun).toHaveBeenCalledWith("row-s");
    cleanup();

    rerunState = {
      isError: true,
      variables: "row-s",
      error: new AxiosError("409", "ERR_BAD_REQUEST", undefined, undefined, {
        status: 409,
        data: { code: "checks_running", message: "attempt 2 is running" }
      } as AxiosResponse)
    };
    renderPanel();
    expect(screen.getByTestId("build-checks-rerun-refused").textContent).toBe(
      "Its checks are still running."
    );
  });

  it("shows a build `?checks=` names, and offers the way back to staging's", () => {
    checks = FAILED;
    const back = vi.fn();
    renderPanel("60e2f19aaaaaaaaa", back);
    expect(asked).toContain("row-p");
    expect(screen.getByTestId("build-checks-build").textContent).toContain("not on staging");
    fireEvent.click(screen.getByTestId("build-checks-show-staging"));
    expect(back).toHaveBeenCalledTimes(1);
  });
});
