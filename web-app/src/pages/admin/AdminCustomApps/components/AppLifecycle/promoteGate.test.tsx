// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppLifecycle as Lifecycle } from "@/hooks/api/customApps/deriveLifecycle";
import { AppBuildChecksService } from "@/services/api/appBuildChecks";
import { AppReleasesService } from "@/services/api/appReleases";
import { CustomAppsService } from "@/services/api/customApps";
import type { BuildCheck, BuildChecks } from "@/types/appBuildChecks";
import type { MovePlan } from "@/types/appPromote";
import type { ReleaseTimeline } from "@/types/appReleases";
import type { AppBuild, CustomApp, PromotionChecks } from "@/types/apps";
import { lifecycleVerdict } from "../../appStatus";
import { BuildChecksSummary } from "../AppConsole/BuildChecks";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: { listSecrets: vi.fn(), moveProduction: vi.fn() }
}));
vi.mock("@/services/api/appBuildChecks", () => ({ AppBuildChecksService: { get: vi.fn() } }));
vi.mock("@/services/api/appReleases", () => ({ AppReleasesService: { list: vi.fn() } }));
vi.mock("@/hooks/api/customApps/useAppLifecycle", () => ({ useAppLifecycle: () => LIFECYCLE }));

import { AppLifecycle } from "./index";

const build = (over: Partial<AppBuild>): AppBuild => ({
  id: "b",
  build_id: "0123456789abcdef",
  created_at: "2026-11-22T09:00:00Z",
  is_draft: false,
  is_published: false,
  ...over
});
const LIVE = build({ id: "live", build_id: "aaaaaaaaaaaaaaaa", is_published: true });
const DRAFT = build({ id: "draft", build_id: "bbbbbbbbbbbbbbbb", is_draft: true });
const OLD = build({ id: "old", build_id: "cccccccccccccccc" });
const OLDER = build({ id: "older", build_id: "dddddddddddddddd" });

const LIFECYCLE: Lifecycle = {
  liveBuild: LIVE,
  draftBuild: DRAFT,
  draftAhead: true,
  published: true,
  publishedAt: "2026-11-22T09:30:00Z",
  verdict: lifecycleVerdict({ published: true, health: "operational" }),
  heldCount: null,
  heldTruncated: false,
  lastPromoted: { at: null, byEmail: null },
  builds: [DRAFT, LIVE, OLD, OLDER],
  isLoading: false,
  error: null
};

const GATE: PromotionChecks = {
  enabled: true,
  required: null,
  effective_required: ["functions", "loads", "design_system", "smoke"],
  seen_required: false,
  production_smoke: true,
  auto_rollback: false
};

const KINDS = ["validation", "functions", "loads", "design_system", "smoke"];
const row = (kind: string, status: string, over: Partial<BuildCheck> = {}): BuildCheck => ({
  kind,
  label: kind,
  status,
  advisory: false,
  summary: null,
  detail: null,
  notes: [],
  attempt: 1,
  run_id: "r",
  created_at: "2026-11-22T10:00:00Z",
  started_at: null,
  finished_at: null,
  duration_ms: null,
  ...over
});
const answer = (environment: string, state: string, checks: BuildCheck[]): BuildChecks => ({
  app_id: "app-1",
  build: { id: "b", build_id: "b", created_at: "2026-11-22T09:00:00Z" },
  attempt: checks.length ? 1 : null,
  environment,
  state,
  counts: { total: 0, queued: 0, running: 0, passed: 0, failed: 0, error: 0, skipped: 0 },
  checks,
  summary: null,
  attempts: [],
  runner: null,
  stalled: null
});
const staging = (state: string, statuses: string[]) =>
  answer(
    "staging",
    state,
    statuses.map((s, i) => row(KINDS[i], s))
  );
const smoke = (status: string, over: Partial<BuildCheck> = {}) =>
  answer("production", status === "queued" ? "running" : status, [
    row("production_smoke", status, over)
  ]);
const NO_PRODUCTION = answer("production", "none", []);
const PASSED = staging("passed", ["passed", "passed", "passed", "passed", "passed"]);

/** What the server answers for staging's build, and for the live one on production. */
const serve = (stagingAnswer: BuildChecks, productionAnswer: BuildChecks = NO_PRODUCTION) =>
  vi
    .mocked(AppBuildChecksService.get)
    .mockImplementation(async (_app, _build, environment) =>
      environment === "production" ? productionAnswer : stagingAnswer
    );

const plan = (over: Partial<MovePlan> = {}): MovePlan => ({
  action: "promote",
  app_id: "app-1",
  from: "staging",
  to: "production",
  build: { id: "draft", build_id: "bbbbbbbbbbbbbbbb", published_by: null, created_at: "" },
  current: { id: "live", build_id: "aaaaaaaaaaaaaaaa" },
  expect_current: "live",
  served_since: null,
  functions: { added: [], removed: [], changed: [] },
  schedules: { added: [], removed: [], changed: [] },
  migrations: { when: "before_move", oltp: [], airhouse: [] },
  required_secrets_unset: [],
  blockers: [],
  warnings: [],
  dry_run: true,
  moved: false,
  ...over
});

let client = new QueryClient();

/** The card as the console mounts it: the Checks row, and where the smoke's evidence is. */
const renderCard = (promotion_checks?: PromotionChecks | null) => {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MemoryRouter>
      <QueryClientProvider client={client}>
        <AppLifecycle
          app={{ id: "app-1", org_slug: "acme", slug: "store", promotion_checks } as CustomApp}
          stagingChecks={(build) => (
            <BuildChecksSummary
              appId='app-1'
              build={build}
              to='/releases'
              promotionChecks={promotion_checks}
            />
          )}
          smokeTo={(live) => `/releases?checks=${live.build_id}&checks_env=production`}
        />
      </QueryClientProvider>
    </MemoryRouter>
  );
};

/**
 * Every read the card made has answered and been drawn. A test that expects something
 * NOT to happen (Promote stays open, no row appears) waits for this first: asserting
 * the moment the request leaves would pass before the answer could change anything.
 */
const settled = async (reads: number) => {
  await vi.waitFor(() => {
    expect(AppBuildChecksService.get).toHaveBeenCalledTimes(reads);
    expect(client.isFetching()).toBe(0);
  });
  await act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));
};

const promote = () => screen.getByTestId("app-lifecycle-promote");
const chip = () => screen.getByTestId("app-lifecycle-checks").textContent;
const isPrimary = (button: HTMLElement) => button.className.split(/\s+/).includes("bg-primary");
const WAITING = staging("running", ["passed", "passed", "running", "passed", "queued"]);

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Live build card — with the promote-checks gate off", () => {
  it("is the card it was: Promote open on the same checks, no smoke row, nothing new read", async () => {
    for (const promotion_checks of [undefined, null, { ...GATE, enabled: false }]) {
      // The answers that hold Promote and draw a smoke row once the gate is on.
      serve(WAITING, smoke("failed"));
      renderCard(promotion_checks);
      await settled(1);
      expect(chip()).toBe("3 of 5 done");
      expect(promote()).toBeEnabled();
      expect(promote()).toHaveTextContent(/^Promote to production$/);
      expect(isPrimary(promote())).toBe(true);
      expect(screen.queryByTestId("app-lifecycle-smoke")).toBeNull();
      expect(screen.queryByTestId("app-lifecycle-rollback-before")).toBeNull();
      expect(
        Array.from(
          screen.getByTestId("app-lifecycle").querySelectorAll("dt"),
          (dt) => dt.textContent
        )
      ).toEqual(["Production", "Staging", "Checks"]);
      // The one read the Checks row always made, and no other.
      expect(vi.mocked(AppBuildChecksService.get).mock.calls).toEqual([["app-1", "draft"]]);
      expect(AppReleasesService.list).not.toHaveBeenCalled();
      cleanup();
      vi.clearAllMocks();
    }
  });
});

describe("Live build card — on a surface with no Checks row, gate off", () => {
  it("reads no checks at all: nothing is asked that was not asked before", async () => {
    serve(WAITING, smoke("failed"));
    render(
      <QueryClientProvider client={new QueryClient()}>
        <AppLifecycle app={{ id: "app-1", org_slug: "acme", slug: "store" } as CustomApp} />
      </QueryClientProvider>
    );
    await act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));
    expect(promote()).toBeEnabled();
    expect(AppBuildChecksService.get).not.toHaveBeenCalled();
    expect(AppReleasesService.list).not.toHaveBeenCalled();
  });
});

describe("Live build card — Promote waits on the checks the app requires", () => {
  it("is disabled and says so while a required check is open", async () => {
    serve(WAITING);
    renderCard(GATE);
    await settled(2);
    expect(promote()).toBeDisabled();
    expect(promote()).toHaveTextContent(/^Promote · waiting on checks$/);
    expect(chip()).toBe("3 of 5 done");
    // Staging's attempt is asked for as it always was; production's by name.
    expect(AppBuildChecksService.get).toHaveBeenCalledWith("app-1", "draft");
    expect(AppBuildChecksService.get).toHaveBeenCalledWith("app-1", "live", "production");
  });

  it("opens once every required check passed", async () => {
    serve(PASSED);
    renderCard(GATE);
    await settled(2);
    expect(chip()).toBe("All 5 passed");
    expect(promote()).toBeEnabled();
    expect(promote()).toHaveTextContent(/^Promote to production$/);
  });

  it("does not wait on a check this app does not require", async () => {
    serve(staging("running", ["passed", "passed", "passed", "running", "passed"]));
    renderCard({ ...GATE, required: ["smoke"], effective_required: ["smoke"] });
    await settled(2);
    expect(chip()).toBe("4 of 5 done");
    expect(promote()).toBeEnabled();
  });

  it("waits on the comparison with production when a person has to see what changed", async () => {
    const comparing = staging("running", ["passed", "passed", "passed", "passed", "passed"]);
    comparing.checks.push(row("changes", "running", { advisory: true }));
    serve(comparing);
    renderCard({ ...GATE, seen_required: true });
    await settled(2);
    expect(promote()).toBeDisabled();
    expect(promote()).toHaveTextContent(/^Promote · waiting on checks$/);
    cleanup();
    vi.clearAllMocks();

    serve(comparing);
    renderCard(GATE);
    await settled(2);
    expect(promote()).toBeEnabled();
  });

  it("says a required check failed in one chip, and Promote opens the confirm that holds the block", async () => {
    serve(staging("failed", ["passed", "passed", "passed", "failed", "passed"]));
    vi.mocked(CustomAppsService.moveProduction).mockResolvedValue(
      plan({ blockers: [{ code: "checks_not_passed", message: "design system failed" }] })
    );
    renderCard(GATE);
    await settled(2);
    expect(chip()).toBe("1 required check failed");
    expect(promote()).toBeEnabled();
    expect(promote()).toHaveTextContent(/^Promote to production$/);
    await userEvent.click(promote());
    const dialog = screen.getByTestId("app-lifecycle-confirm");
    await within(dialog).findByTestId("app-lifecycle-plan-blocker-checks_not_passed");
    expect(within(dialog).getByTestId("app-lifecycle-confirm-ok")).toBeDisabled();
  });
});

describe("Live build card — production's smoke after a move", () => {
  const line = () => screen.getByTestId("app-lifecycle-smoke");

  it("waits for the new build", async () => {
    serve(PASSED, smoke("queued"));
    renderCard(GATE);
    await settled(2);
    expect(line()).toHaveAttribute("data-state", "waiting");
    expect(line()).toHaveTextContent("Waiting for the new build");
    // The row sits under Production, above Staging.
    expect(
      Array.from(screen.getByTestId("app-lifecycle").querySelectorAll("dt"), (dt) => dt.textContent)
    ).toEqual(["Production", "Smoke", "Staging", "Checks"]);
    expect(screen.queryByTestId("app-lifecycle-rollback-before")).toBeNull();
    expect(isPrimary(promote())).toBe(true);
    expect(isPrimary(screen.getByTestId("app-lifecycle-rollback"))).toBe(false);
  });

  it("says how it passed, linking to its evidence", async () => {
    const screens = Array.from({ length: 14 }, (_, i) => ({ path: `s${i}`, viewports: [] }));
    serve(PASSED, smoke("passed", { detail: { screens, new_errors: [] } }));
    renderCard(GATE);
    await settled(2);
    expect(line()).toHaveAttribute("data-state", "passed");
    expect(line()).toHaveTextContent("14 screens, 0 new errors");
    expect(within(line()).getByTestId("app-lifecycle-smoke-link")).toHaveAttribute(
      "href",
      "/releases?checks=aaaaaaaaaaaaaaaa&checks_env=production"
    );
    // Nothing failed: the timeline that names the build to go back to is not read.
    expect(AppReleasesService.list).not.toHaveBeenCalled();
    expect(isPrimary(promote())).toBe(true);
  });

  const FAILED = smoke("failed", {
    detail: { new_errors: [{ path: "stores", viewport: "desk", error: "TypeError: x" }] }
  });
  /** Production served `older` before `live`; `old` only ever reached staging. */
  const timeline = (previous: number | null) =>
    ({
      releases: [
        { index: 2, build: { id: "live" }, current: true, previous },
        { index: 1, build: { id: "old" }, current: false, previous: null },
        { index: 0, build: { id: "older" }, current: false, previous: null }
      ]
    }) as unknown as ReleaseTimeline;

  it("makes rolling back to the build before the first thing to do when it failed", async () => {
    serve(PASSED, FAILED);
    vi.mocked(AppReleasesService.list).mockResolvedValue(timeline(0));
    vi.mocked(CustomAppsService.moveProduction).mockResolvedValue(
      plan({ action: "rollback", from: null })
    );
    renderCard(GATE);
    await settled(2);
    expect(line()).toHaveTextContent("1 screen with new errors");
    const back = await screen.findByTestId("app-lifecycle-rollback-before");
    expect(back).toHaveTextContent(/^Roll back to ddddddd$/);
    // The one primary control, and the first: Promote and the menu step back.
    expect(isPrimary(back)).toBe(true);
    expect(isPrimary(promote())).toBe(false);
    expect(isPrimary(screen.getByTestId("app-lifecycle-rollback"))).toBe(false);
    expect(screen.getByTestId("app-lifecycle-actions").firstElementChild).toBe(back);
    // It confirms on that build's rollback plan, like a pick from the menu.
    await userEvent.click(back);
    await within(screen.getByTestId("app-lifecycle-confirm")).findByTestId("app-lifecycle-plan");
    expect(CustomAppsService.moveProduction).toHaveBeenCalledWith(
      "app-1",
      { act: "rollback", buildId: "older" },
      { dry_run: true }
    );
  });

  it("offers the menu of retained builds first when the build before is not known", async () => {
    serve(PASSED, FAILED);
    vi.mocked(AppReleasesService.list).mockResolvedValue(timeline(null));
    renderCard(GATE);
    await settled(2);
    await vi.waitFor(() => expect(AppReleasesService.list).toHaveBeenCalled());
    await settled(2);
    expect(screen.queryByTestId("app-lifecycle-rollback-before")).toBeNull();
    expect(isPrimary(screen.getByTestId("app-lifecycle-rollback"))).toBe(true);
    expect(isPrimary(promote())).toBe(false);
  });

  it("keeps a smoke that could not run apart from one that failed", async () => {
    serve(PASSED, smoke("error", { summary: "production did not serve the build" }));
    renderCard(GATE);
    await settled(2);
    expect(line()).toHaveAttribute("data-state", "error");
    expect(line()).toHaveTextContent("Could not run");
    expect(AppReleasesService.list).not.toHaveBeenCalled();
    expect(isPrimary(promote())).toBe(true);
  });

  it("shows no line for a build production was never smoked on", async () => {
    serve(PASSED, NO_PRODUCTION);
    renderCard(GATE);
    await settled(2);
    expect(screen.queryByTestId("app-lifecycle-smoke")).toBeNull();
    expect(isPrimary(screen.getByTestId("app-lifecycle-rollback"))).toBe(false);
    expect(isPrimary(promote())).toBe(true);
  });
});
