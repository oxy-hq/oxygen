// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { BuildCheck, BuildChecks } from "@/types/appBuildChecks";
import type { AppBuild, PromotionChecks } from "@/types/apps";
import { BuildChecksPanel } from ".";

/** What each attempt answers, by `build:environment`; a read not listed has no data. */
let answers: Record<string, BuildChecks | undefined> = {};
const asked: string[] = [];

vi.mock("@/hooks/api/customApps/useBuildChecks", () => ({
  useBuildChecks: (_app: string, build: string | undefined, environment = "staging") => {
    if (build) asked.push(`${build}:${environment}`);
    const data = build ? answers[`${build}:${environment}`] : undefined;
    return { isPending: !data, isError: false, data };
  },
  useRerunBuildChecks: () => ({ mutate: vi.fn(), isPending: false, isError: false }),
  useMarkChangesSeen: () => ({ mutate: vi.fn(), isPending: false, isError: false })
}));

const STAGING: AppBuild = {
  id: "row-s",
  build_id: "9087d7b1c2d3e4f5",
  created_at: "2026-11-22T09:00:00Z",
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
const OLD: AppBuild = { ...LIVE, id: "row-o", build_id: "0ddba11000000000", is_published: false };

vi.mock("@/hooks/api/customApps/useAppBuilds", () => ({
  useAppBuilds: () => ({ isPending: false, isError: false, data: { builds: [STAGING, LIVE, OLD] } })
}));
vi.mock("@/hooks/api/customApps/useAppFunctions", () => ({
  useInvocationCalls: () => ({ isPending: true, isError: false, data: undefined })
}));
vi.mock("@/hooks/api/customApps/useBuildCheckFile", () => ({
  useBuildCheckFile: () => ({ url: null, state: "missing" })
}));

const row = (over: Partial<BuildCheck>): BuildCheck => ({
  kind: "validation",
  label: "Builds and validates",
  status: "passed",
  advisory: false,
  summary: null,
  detail: { findings: [] },
  notes: [],
  attempt: 1,
  run_id: "r",
  created_at: "2026-11-22T10:00:00Z",
  started_at: null,
  finished_at: null,
  duration_ms: null,
  ...over
});
const attempt = (build: AppBuild, environment: string, checks: BuildCheck[]): BuildChecks => ({
  app_id: "app-1",
  build: { id: build.id, build_id: build.build_id, created_at: build.created_at },
  attempt: checks.length ? 1 : null,
  environment,
  state: checks.length ? "failed" : "none",
  counts: { total: 0, queued: 0, running: 0, passed: 0, failed: 0, error: 0, skipped: 0 },
  checks,
  summary: null,
  attempts: checks.length
    ? [{ attempt: 1, state: "failed", created_at: "2026-11-22T10:00:00Z" }]
    : [],
  runner: null,
  stalled: null
});

const STAGED = attempt(STAGING, "staging", [row({})]);
const SMOKED = attempt(LIVE, "production", [
  row({
    kind: "production_smoke",
    label: "Production opens after promote",
    status: "failed",
    summary: "1 of 2 screens have new errors",
    detail: {
      findings: [],
      baseline_at: "2026-11-22T09:58:00Z",
      screens: [{ path: "stores", viewports: [] }],
      new_errors: [{ path: "stores", viewport: "desk", error: "TypeError: x is undefined" }]
    }
  })
]);

const GATE: PromotionChecks = {
  enabled: true,
  required: null,
  effective_required: ["functions", "loads", "design_system", "smoke"],
  seen_required: false,
  production_smoke: true,
  auto_rollback: false
};

const onEnvironment = vi.fn();
const renderPanel = (
  props: Partial<React.ComponentProps<typeof BuildChecksPanel>> = {},
  requested: string | null = null
) =>
  render(
    <BuildChecksPanel
      appId='app-1'
      requested={requested}
      onShowStaging={vi.fn()}
      onEnvironment={onEnvironment}
      {...props}
    />
  );

const heading = () => screen.getByTestId("build-checks-build").textContent;
const scope = () => screen.queryByTestId("apps-console-panel-build-checks-scope");

afterEach(() => {
  cleanup();
  answers = {};
  asked.length = 0;
  onEnvironment.mockReset();
});

describe("Checks panel — with the promote-checks gate off", () => {
  it("is staging's panel, with no switch, and asks for no production attempt", () => {
    answers = { "row-s:staging": STAGED, "row-p:production": SMOKED };
    for (const promotionChecks of [undefined, null, { ...GATE, enabled: false }]) {
      renderPanel({ promotionChecks, environment: "production" });
      expect(screen.queryByTestId("build-checks-environment")).toBeNull();
      expect(scope()).toHaveTextContent("Staging");
      expect(heading()).toContain("9087d7b");
      expect(heading()).toContain("· on staging");
      expect(screen.getByTestId("build-checks-rerun")).toBeEnabled();
      expect(asked.every((ask) => ask.endsWith(":staging"))).toBe(true);
      cleanup();
    }
  });
});

describe("Checks panel — the attempt run on production", () => {
  it("offers no switch until the live build has a production attempt", () => {
    answers = { "row-s:staging": STAGED, "row-p:production": attempt(LIVE, "production", []) };
    renderPanel({ promotionChecks: GATE });
    expect(asked).toContain("row-p:production");
    expect(screen.queryByTestId("build-checks-environment")).toBeNull();
    expect(scope()).toHaveTextContent("Staging");
  });

  it("does not take a staging answer for a production one", () => {
    // A server that ignores `environment` answers the build's staging attempt.
    answers = { "row-s:staging": STAGED, "row-p:production": attempt(LIVE, "staging", [row({})]) };
    renderPanel({ promotionChecks: GATE, environment: "production" });
    expect(screen.queryByTestId("build-checks-environment")).toBeNull();
    expect(heading()).toContain("9087d7b");
  });

  it("offers Staging and Production once one exists, and stays on staging until asked", async () => {
    answers = { "row-s:staging": STAGED, "row-p:production": SMOKED };
    renderPanel({ promotionChecks: GATE });
    const pressed = () =>
      within(screen.getByTestId("build-checks-environment"))
        .getAllByRole("button")
        .map((b) => [b.textContent, b.getAttribute("aria-pressed")]);
    expect(pressed()).toEqual([
      ["Staging", "true"],
      ["Production", "false"]
    ]);
    // The switch names the environment, so the header's own chip steps aside.
    expect(scope()).toBeNull();
    expect(heading()).toContain("9087d7b");
    expect(screen.getByTestId("build-checks-rerun")).toBeInTheDocument();
    await userEvent.click(screen.getByTestId("build-checks-environment-production"));
    expect(onEnvironment).toHaveBeenCalledWith("production");
  });

  it("shows the live build's production attempt, new errors first, with no Run again", () => {
    answers = { "row-s:staging": STAGED, "row-p:production": SMOKED };
    renderPanel({ promotionChecks: GATE, environment: "production" });
    expect(heading()).toContain("60e2f19");
    expect(heading()).toContain("· on production");
    expect(screen.getByTestId("build-checks-environment-production")).toHaveAttribute(
      "aria-pressed",
      "true"
    );
    expect(screen.queryByTestId("build-checks-rerun")).toBeNull();
    const evidence = screen.getByTestId("build-check-evidence-production_smoke");
    expect(within(evidence).getByTestId("build-check-new-error")).toHaveTextContent(
      "/storesdeskTypeError: x is undefined"
    );
    expect(evidence.querySelector("[data-testid]")?.getAttribute("data-testid")).toBe(
      "build-check-production-smoke"
    );
  });

  it("shows the production attempt of the build ?checks= names, when it has one", () => {
    const rolledBack = attempt(OLD, "production", SMOKED.checks);
    answers = {
      "row-o:staging": attempt(OLD, "staging", [row({})]),
      "row-o:production": rolledBack
    };
    renderPanel({ promotionChecks: GATE, environment: "production" }, "0ddba11000000000");
    expect(asked).toContain("row-o:production");
    expect(heading()).toContain("0ddba11");
    expect(heading()).toContain("· not on production");
    expect(screen.queryByTestId("build-checks-show-staging")).toBeNull();
  });

  it("opens the changed screens for a look while the app holds Promote on them", () => {
    const changed = attempt(STAGING, "staging", [
      row({
        kind: "changes",
        label: "What changed",
        advisory: true,
        detail: {
          findings: [],
          compared: 2,
          changed: 1,
          screens: [{ path: "stores", viewport: "desk", changed: true, ratio: 0.03 }],
          seen: null
        }
      })
    ]);
    answers = { "row-s:staging": changed };
    const isPrimary = (b: HTMLElement) => b.className.split(/\s+/).includes("bg-primary");
    renderPanel({ promotionChecks: { ...GATE, seen_required: true } });
    expect(isPrimary(screen.getByTestId("build-check-mark-seen"))).toBe(true);
    cleanup();

    // Without the rule the row is closed like any check that passed, and the button waits in it.
    renderPanel({ promotionChecks: GATE });
    expect(screen.queryByTestId("build-check-mark-seen")).toBeNull();
    expect(screen.getByTestId("build-check-changes-toggle")).toHaveAttribute(
      "aria-expanded",
      "false"
    );
  });

  it("falls back to staging when production was asked for and has no attempt", () => {
    answers = { "row-s:staging": STAGED, "row-p:production": attempt(LIVE, "production", []) };
    renderPanel({ promotionChecks: GATE, environment: "production" });
    expect(heading()).toContain("9087d7b");
    expect(screen.getByTestId("build-checks-rerun")).toBeInTheDocument();
  });
});
