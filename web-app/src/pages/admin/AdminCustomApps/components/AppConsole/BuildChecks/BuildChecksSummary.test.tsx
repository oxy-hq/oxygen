// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppLifecycle } from "@/hooks/api/customApps/deriveLifecycle";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheck, BuildChecks } from "@/types/appBuildChecks";
import type { AppBuild, PromotionChecks } from "@/types/apps";
import { lifecycleVerdict } from "../../../appStatus";
import { LifecycleCard } from "../../AppLifecycle/LifecycleCard";
import { BuildChecksSummary } from "./BuildChecksSummary";

let read: { isPending: boolean; isError: boolean; data?: BuildChecks } = {
  isPending: true,
  isError: false
};
const asked: (string | undefined)[] = [];

vi.mock("@/hooks/api/customApps/useBuildChecks", () => ({
  useBuildChecks: (_app: string, build: string | undefined) => {
    asked.push(build);
    return read;
  }
}));

const KINDS = ["validation", "functions", "loads", "design_system"];
const rows = (...statuses: string[]): BuildCheck[] =>
  statuses.map((status, i) => ({
    kind: KINDS[i],
    label: KINDS[i],
    status,
    advisory: false,
    summary: null,
    detail: null,
    notes: [],
    attempt: 1,
    run_id: `r${i}`,
    created_at: "2026-11-20T10:00:00Z",
    started_at: null,
    finished_at: null,
    duration_ms: null
  }));

const checks = (state: BuildChecks["state"], list: BuildCheck[]): BuildChecks => ({
  app_id: "app-1",
  build: { id: "draft", build_id: "bbbbbbbbbbbbbbbb", created_at: "2026-11-20T10:00:00Z" },
  attempt: list.length ? 1 : null,
  environment: "staging",
  state,
  counts: { total: list.length, queued: 0, running: 0, passed: 0, failed: 0, error: 0, skipped: 0 },
  checks: list,
  summary: null,
  attempts: [],
  runner: null,
  stalled: null
});

const build = (over: Partial<AppBuild>): AppBuild => ({
  id: "b",
  build_id: "0123456789abcdef",
  created_at: "2026-11-20T10:00:00Z",
  is_draft: false,
  is_published: false,
  ...over
});
const LIVE = build({ id: "live", build_id: "aaaaaaaaaaaaaaaa", is_published: true });
const DRAFT = build({ id: "draft", build_id: "bbbbbbbbbbbbbbbb", is_draft: true });

const lifecycle: AppLifecycle = {
  liveBuild: LIVE,
  draftBuild: DRAFT,
  draftAhead: true,
  published: true,
  publishedAt: "2026-11-20T09:00:00Z",
  verdict: lifecycleVerdict({ published: true, health: "operational" }),
  heldCount: 0,
  heldTruncated: false,
  lastPromoted: { at: null, byEmail: null },
  builds: [DRAFT, LIVE],
  isLoading: false,
  error: null
};

const TO = "/admin/apps/acme/store/releases?checks=bbbbbbbbbbbbbbbb";

/** The card as the console mounts it: the Checks row for the build staging serves. */
const renderCard = (promotionChecks?: PromotionChecks | null) =>
  render(
    <MemoryRouter>
      <LifecycleCard
        lifecycle={lifecycle}
        orgSlug='acme'
        actions={{
          promoteDraft: vi.fn(async () => undefined),
          rollbackTo: vi.fn(async () => undefined),
          pending: false
        }}
        checks={
          <BuildChecksSummary
            appId='app-1'
            build={DRAFT}
            to={TO}
            promotionChecks={promotionChecks}
          />
        }
      />
    </MemoryRouter>
  );

afterEach(() => {
  cleanup();
  read = { isPending: true, isError: false };
  asked.length = 0;
});

const row = () => screen.getByTestId("app-lifecycle-checks");
const link = () => screen.getByTestId("build-checks-summary-link");

describe("the Live build card's Checks row", () => {
  it("sits under Staging and reads the staging build by its row id", () => {
    read = { isPending: false, isError: false, data: checks("none", []) };
    renderCard();
    expect(row().previousElementSibling?.textContent).toBe("Checks");
    expect(asked).toEqual(["draft"]);
  });

  it("says a build never checked is 'Not checked', muted, with no chip", () => {
    read = { isPending: false, isError: false, data: checks("none", []) };
    renderCard();
    expect(link().dataset.state).toBe("none");
    expect(row().textContent).toBe("Not checked");
    expect(link().innerHTML).toContain("text-muted-foreground");
  });

  it("counts what is done while the checks run", () => {
    read = {
      isPending: false,
      isError: false,
      data: checks("running", rows("passed", "passed", "running", "queued"))
    };
    renderCard();
    expect(link().dataset.state).toBe("running");
    expect(row().textContent).toBe("2 of 4 done");
    expect(link().innerHTML).not.toContain(ADMIN_TONE.danger.text);
  });

  it("says 'Starting' until a runner answers, then counts", () => {
    const queued = checks("running", rows("queued", "queued", "queued", "queued"));
    read = { isPending: false, isError: false, data: queued };
    renderCard();
    expect(link().dataset.state).toBe("running");
    expect(row().textContent).toBe("Starting");
    expect(link().innerHTML).not.toContain(ADMIN_TONE.danger.text);
    cleanup();

    const runner = {
      label: "GitHub Actions run #1482",
      url: null,
      started_at: "2026-11-20T10:00:19Z"
    };
    read = { isPending: false, isError: false, data: { ...queued, runner } };
    renderCard();
    expect(row().textContent).toBe("0 of 4 done");
  });

  it("names a run that got no answer as checks that could not run, in red", () => {
    read = {
      isPending: false,
      isError: false,
      data: { ...checks("failed", rows("error", "error", "error", "error")), stalled: "no_runner" }
    };
    renderCard();
    expect(row().textContent).toBe("4 checks could not run");
    expect(link().innerHTML).toContain(ADMIN_TONE.danger.text);
  });

  it("names a failure in red, and links to the build's Checks list", () => {
    read = {
      isPending: false,
      isError: false,
      data: checks("failed", rows("passed", "passed", "passed", "failed"))
    };
    renderCard();
    expect(row().textContent).toBe("1 check failed");
    expect(link().innerHTML).toContain(ADMIN_TONE.danger.text);
    expect(link().getAttribute("href")).toBe(TO);
    // Phase 1 reports only: Promote is offered and enabled exactly as before.
    const promote = screen.getByTestId("app-lifecycle-promote") as HTMLButtonElement;
    expect(promote.disabled).toBe(false);
    expect(promote.textContent).toBe("Promote to production");
  });

  it("says all passed in green, and names skips", () => {
    read = {
      isPending: false,
      isError: false,
      data: checks("passed", rows("passed", "passed", "passed", "passed"))
    };
    renderCard();
    expect(row().textContent).toBe("All 4 passed");
    expect(link().innerHTML).toContain(ADMIN_TONE.ok.text);
    cleanup();

    read = {
      isPending: false,
      isError: false,
      data: checks("passed", rows("passed", "skipped", "passed", "passed"))
    };
    renderCard();
    expect(row().textContent).toBe("3 passed, 1 skipped");
  });

  it("has no Checks row on a surface that passes none", () => {
    render(
      <LifecycleCard
        lifecycle={lifecycle}
        orgSlug='acme'
        actions={{ pending: false, promoteDraft: vi.fn(async () => undefined) }}
      />
    );
    expect(screen.queryByTestId("app-lifecycle-checks")).toBeNull();
    expect(asked).toEqual([]);
  });
});

describe("the Checks row once checks hold Promote", () => {
  const GATE: PromotionChecks = {
    enabled: true,
    required: null,
    effective_required: ["functions", "loads", "design_system"],
    seen_required: false,
    production_smoke: true,
    auto_rollback: false
  };
  const failed = () => checks("failed", rows("passed", "passed", "passed", "failed"));

  it("says a required check failed, in the one chip, in red", () => {
    read = { isPending: false, isError: false, data: failed() };
    renderCard(GATE);
    expect(row().textContent).toBe("1 required check failed");
    expect(link().innerHTML).toContain(ADMIN_TONE.danger.text);
    expect(row().querySelectorAll("[data-testid='build-checks-summary-link'] > *")).toHaveLength(1);
  });

  it("is the roll-up it was with the gate off, on the same checks", () => {
    read = { isPending: false, isError: false, data: failed() };
    renderCard({ ...GATE, enabled: false });
    expect(row().textContent).toBe("1 check failed");
    cleanup();
    renderCard(null);
    expect(row().textContent).toBe("1 check failed");
  });

  it("keeps the old words for a failure the app does not require", () => {
    read = { isPending: false, isError: false, data: failed() };
    renderCard({ ...GATE, effective_required: ["functions", "loads"] });
    expect(row().textContent).toBe("1 check failed");
  });

  it("turns a build never checked into a red chip while something is required", () => {
    read = { isPending: false, isError: false, data: checks("none", []) };
    renderCard(GATE);
    expect(row().textContent).toBe("Not checked");
    expect(link().innerHTML).toContain(ADMIN_TONE.danger.text);
  });
});
