// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppLifecycle as Lifecycle } from "@/hooks/api/customApps/deriveLifecycle";
import { CustomAppsService } from "@/services/api/customApps";
import type { MovePlan, MovePlanChecks } from "@/types/appPromote";
import type { AppBuild, CustomApp } from "@/types/apps";
import { lifecycleVerdict } from "../../appStatus";

// The real verbs and the real plan read: what the confirm draws and sends is the point.
vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: { listSecrets: vi.fn(), moveProduction: vi.fn() }
}));
vi.mock("@/hooks/api/customApps/useAppLifecycle", () => ({ useAppLifecycle: () => LIFECYCLE }));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

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
  builds: [DRAFT, LIVE],
  isLoading: false,
  error: null
};

const REQUIRED = ["functions", "loads", "design_system", "smoke"];
const ROWS = [
  { kind: "validation", label: "Builds and validates", status: "passed", summary: "ok" },
  { kind: "functions", label: "Its checks pass", status: "passed", summary: "3 of 3 passed" },
  { kind: "loads", label: "It loads, desk and phone", status: "passed", summary: "1.2s" },
  {
    kind: "design_system",
    label: "Follows the design system",
    status: "failed",
    summary: "src/index.css:68 restates --card"
  },
  { kind: "smoke", label: "Every screen opens", status: "passed", summary: "14 screens" },
  { kind: "changes", label: "What changed", status: "passed", summary: "4 of 28 screens differ" }
];
const checks = (over: Partial<MovePlanChecks> = {}): MovePlanChecks => ({
  required: REQUIRED,
  attempt: 2,
  state: "failed",
  rows: ROWS.map((r) => ({ ...r, required: REQUIRED.includes(r.kind) })),
  unmet: ["design_system"],
  running: [],
  changes: { changed: 4, compared: 28, seen: false, seen_required: false },
  summary: {
    author: "model",
    model: "model-test",
    text: "Restyles the Stores table. Risk: dark cards stay white.",
    refs: [],
    created_at: "2026-11-22T10:00:00Z"
  },
  overridable: true,
  overridden: null,
  ...over
});
const FAILED = { code: "checks_not_passed", message: "the server's sentence about the checks" };

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
const BLOCKED = plan({ blockers: [FAILED], checks: checks() });

const refusal = (status: number, code: string) =>
  new AxiosError("refused", "ERR", undefined, undefined, {
    status,
    data: { code, message: "not for you" }
  } as AxiosResponse);

/** The plan answers `dry`; the act answers (or throws) `act`. */
const serve = (dry: MovePlan, act: MovePlan | Error = plan({ dry_run: false, moved: true })) =>
  vi.mocked(CustomAppsService.moveProduction).mockImplementation(async (_id, _t, body) => {
    if (body.dry_run) return dry;
    if (act instanceof Error) throw act;
    return act;
  });
const acts = () =>
  vi.mocked(CustomAppsService.moveProduction).mock.calls.filter(([, , body]) => !body.dry_run);

/** Opens the promote confirm and waits for its plan. */
const openConfirm = async () => {
  render(
    <QueryClientProvider client={new QueryClient()}>
      <AppLifecycle app={{ id: "app-1", org_slug: "acme", slug: "store" } as CustomApp} />
    </QueryClientProvider>
  );
  await userEvent.click(screen.getByTestId("app-lifecycle-promote"));
  const dialog = screen.getByTestId("app-lifecycle-confirm");
  await within(dialog).findByTestId("app-lifecycle-plan");
  return within(dialog);
};
const OVERRIDE = "app-lifecycle-confirm-override";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("promote confirm — with the promote-checks gate off", () => {
  it("draws no Checks line, no note and no way past, whether checks is null or absent", async () => {
    for (const dry of [plan(), plan({ checks: null })]) {
      serve(dry);
      const dialog = await openConfirm();
      expect(dialog.queryByTestId("app-lifecycle-plan-checks")).toBeNull();
      expect(dialog.queryByTestId("app-lifecycle-plan-summary")).toBeNull();
      expect(dialog.queryByTestId(OVERRIDE)).toBeNull();
      expect(dialog.getByTestId("app-lifecycle-plan").textContent).toBe(
        "No function, schedule or migration changes."
      );
      expect(dialog.getByTestId("app-lifecycle-confirm-ok")).toBeEnabled();
      cleanup();
    }
  });

  it("says a check blocker in the server's words when the plan carries no checks", async () => {
    serve(plan({ blockers: [FAILED] }));
    const dialog = await openConfirm();
    expect(dialog.getByTestId("app-lifecycle-plan-blocker-checks_not_passed")).toHaveTextContent(
      "the server's sentence about the checks"
    );
    expect(dialog.queryByTestId(OVERRIDE)).toBeNull();
  });
});

describe("promote confirm — the checks that hold it", () => {
  it("lists the required checks with their status, and how many screens changed", async () => {
    serve(BLOCKED);
    const dialog = await openConfirm();
    const line = within(dialog.getByTestId("app-lifecycle-plan-checks"));
    expect(
      REQUIRED.map((kind) => line.getByTestId(`app-lifecycle-plan-check-${kind}`).dataset.status)
    ).toEqual(["passed", "passed", "failed", "passed"]);
    // Only what holds Promote: validation has its own blocker, changes only informs.
    expect(line.queryByTestId("app-lifecycle-plan-check-validation")).toBeNull();
    expect(line.queryByTestId("app-lifecycle-plan-check-changes")).toBeNull();
    expect(line.getByTestId("app-lifecycle-plan-check-design_system")).toHaveTextContent(
      "Follows the design system"
    );
    expect(line.getByTestId("app-lifecycle-plan-changes")).toHaveTextContent("4 screens changed");
  });

  it("draws the three check blockers like the others, in the console's own words", async () => {
    serve(
      plan({
        blockers: [
          FAILED,
          { code: "checks_running", message: "server words" },
          { code: "changes_not_seen", message: "server words" }
        ],
        checks: checks({ running: ["loads"] })
      })
    );
    const dialog = await openConfirm();
    const blocker = (code: string) => dialog.getByTestId(`app-lifecycle-plan-blocker-${code}`);
    expect(blocker("checks_not_passed").textContent).toBe(
      "Required check did not pass: Follows the design system"
    );
    expect(blocker("checks_running").textContent).toBe(
      "Required check still running: It loads, desk and phone"
    );
    expect(blocker("changes_not_seen").textContent).toBe("Nobody has seen what changed: 4 screens");
    expect(dialog.getByTestId("app-lifecycle-confirm-ok")).toBeDisabled();
  });

  it("shows the build's summary as a note by a model, never as a verdict", async () => {
    serve(BLOCKED);
    const dialog = await openConfirm();
    const note = dialog.getByTestId("app-lifecycle-plan-summary");
    expect(note).toHaveTextContent("model-test · informs");
    expect(note).toHaveTextContent("Restyles the Stores table.");
    cleanup();

    serve(plan({ checks: checks({ summary: null, unmet: [] }) }));
    const open = await openConfirm();
    expect(open.queryByTestId("app-lifecycle-plan-summary")).toBeNull();
    // Nothing unmet: the verb is open and there is nothing to promote past.
    expect(open.getByTestId("app-lifecycle-confirm-ok")).toBeEnabled();
    expect(open.queryByTestId(OVERRIDE)).toBeNull();
  });
});

describe("promote confirm — Promote anyway", () => {
  it("is offered only when the plan says the caller may", async () => {
    serve(BLOCKED);
    expect((await openConfirm()).getByTestId(OVERRIDE)).toHaveTextContent("Promote anyway…");
    cleanup();

    serve(plan({ blockers: [FAILED], checks: checks({ overridable: false }) }));
    const locked = await openConfirm();
    expect(locked.queryByTestId(OVERRIDE)).toBeNull();
    expect(locked.getByTestId("app-lifecycle-confirm-ok")).toBeDisabled();
  });

  it("is not offered while another blocker stands, which no reason clears", async () => {
    serve(
      plan({
        blockers: [FAILED, { code: "required_secrets_unset", message: "no NEW_KEY" }],
        checks: checks()
      })
    );
    expect((await openConfirm()).queryByTestId(OVERRIDE)).toBeNull();
  });

  it("opens a step that waits for a reason of ten characters, counting them", async () => {
    serve(BLOCKED);
    const dialog = await openConfirm();
    await userEvent.click(dialog.getByTestId(OVERRIDE));
    expect(dialog.getByTestId(`${OVERRIDE}-step`)).toHaveTextContent("Promote past a failed check");
    expect(dialog.getByTestId(`${OVERRIDE}-past`).textContent).toBe("Follows the design system");
    const go = dialog.getByTestId(`${OVERRIDE}-ok`);
    const count = dialog.getByTestId(`${OVERRIDE}-count`);
    expect(go).toBeDisabled();
    expect(count.textContent).toBe("0 / 10");

    const reason = dialog.getByTestId(`${OVERRIDE}-reason`);
    await userEvent.type(reason, "   hotfix 12");
    expect(count.textContent).toBe("9 / 10");
    expect(go).toBeDisabled();
    await userEvent.type(reason, "3");
    expect(count.textContent).toBe("10");
    expect(count).toHaveAttribute("data-ok", "true");
    expect(go).toBeEnabled();
    // Nothing was sent by typing.
    expect(acts()).toHaveLength(0);

    // Back returns to the plan, with the verb still held.
    await userEvent.click(dialog.getByTestId(`${OVERRIDE}-back`));
    expect(dialog.getByTestId("app-lifecycle-confirm-ok")).toBeDisabled();
  });

  it("sends the trimmed reason as override_checks, beside what a promote always sends", async () => {
    serve(BLOCKED);
    const dialog = await openConfirm();
    await userEvent.click(dialog.getByTestId(OVERRIDE));
    await userEvent.type(dialog.getByTestId(`${OVERRIDE}-reason`), "  Hotfix for the kiosk  ");
    await userEvent.click(dialog.getByTestId(`${OVERRIDE}-ok`));
    await vi.waitFor(() => expect(acts()).toHaveLength(1));
    expect(acts()[0]).toEqual([
      "app-1",
      { act: "promote", buildId: "draft" },
      {
        expect_current: "live",
        confirm_schedule_removals: [],
        override_checks: { reason: "Hotfix for the kiosk" }
      }
    ]);
    await vi.waitFor(() => expect(screen.queryByTestId("app-lifecycle-confirm")).toBeNull());
  });

  it("says a 403 override_not_allowed under the reason, and keeps the step", async () => {
    // A refused act re-reads the plan. That read is left unanswered here, as it is for
    // a moment in the app: the step must not vanish, or lose the reason, while it runs.
    let reads = 0;
    vi.mocked(CustomAppsService.moveProduction).mockImplementation((_id, _t, body) => {
      if (!body.dry_run) return Promise.reject(refusal(403, "override_not_allowed"));
      reads += 1;
      return reads === 1 ? Promise.resolve(BLOCKED) : new Promise(() => undefined);
    });
    const dialog = await openConfirm();
    await userEvent.click(dialog.getByTestId(OVERRIDE));
    await userEvent.type(dialog.getByTestId(`${OVERRIDE}-reason`), "Hotfix for the kiosk");
    await userEvent.click(dialog.getByTestId(`${OVERRIDE}-ok`));
    const refused = await dialog.findByTestId(`${OVERRIDE}-refused`);
    expect(refused).toHaveTextContent("Only an org owner or admin can promote past a check.");
    await vi.waitFor(() => expect(reads).toBe(2));
    expect(dialog.getByTestId(`${OVERRIDE}-reason`)).toHaveValue("Hotfix for the kiosk");
    // With no plan to act on, the same reason cannot be sent again.
    expect(dialog.getByTestId(`${OVERRIDE}-ok`)).toBeDisabled();
    // Said inline, not toasted as well.
    expect(toast.error).not.toHaveBeenCalled();
    expect(acts()).toHaveLength(1);
  });

  it("toasts any other refusal of the act as it always did, with nothing under the reason", async () => {
    serve(BLOCKED, refusal(409, "target_moved"));
    const dialog = await openConfirm();
    await userEvent.click(dialog.getByTestId(OVERRIDE));
    await userEvent.type(dialog.getByTestId(`${OVERRIDE}-reason`), "Hotfix for the kiosk");
    await userEvent.click(dialog.getByTestId(`${OVERRIDE}-ok`));
    await vi.waitFor(() => expect(toast.error).toHaveBeenCalledWith("not for you"));
    expect(dialog.queryByTestId(`${OVERRIDE}-refused`)).toBeNull();
  });

  it("never sends override_checks from the plain verb", async () => {
    serve(plan({ checks: checks({ unmet: [] }) }));
    const dialog = await openConfirm();
    await userEvent.click(dialog.getByTestId("app-lifecycle-confirm-ok"));
    await vi.waitFor(() => expect(acts()).toHaveLength(1));
    expect(acts()[0][2]).toEqual({ expect_current: "live", confirm_schedule_removals: [] });
  });
});
