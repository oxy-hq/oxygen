// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CustomAppsService } from "@/services/api/customApps";
import type { CustomApp, PromotionChecks } from "@/types/apps";
import { PublishCiPanel } from "./PublishCiPanel";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: { setPromotionChecks: vi.fn(), setPromotionRule: vi.fn() }
}));
vi.mock("@/hooks/api/publishTokens/usePublishTokens", () => ({
  usePublishTokens: () => ({ data: [] })
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

const DEFAULTS: PromotionChecks = {
  enabled: true,
  required: null,
  effective_required: ["functions", "loads", "design_system", "smoke"],
  seen_required: false,
  production_smoke: true,
  auto_rollback: false
};

const renderPanel = (promotion_checks?: PromotionChecks | null) =>
  render(
    <MemoryRouter>
      <QueryClientProvider client={new QueryClient()}>
        <PublishCiPanel
          app={
            {
              id: "app-1",
              name: "Store",
              org_slug: "acme",
              slug: "store",
              require_second_person_to_promote: false,
              promotion_checks
            } as CustomApp
          }
        />
      </QueryClientProvider>
    </MemoryRouter>
  );

const ROWS = [
  "functions",
  "loads",
  "design_system",
  "smoke",
  "seen_required",
  "production_smoke",
  "auto_rollback"
];
const toggle = (name: string) => screen.getByTestId(`apps-console-ci-checks-${name}-switch`);
const states = () => ROWS.map((name) => toggle(name).getAttribute("aria-checked") === "true");
const gateCodes = () =>
  within(screen.getByTestId("apps-console-ci-gates"))
    .getAllByRole("listitem")
    .map((li) => li.dataset.testid?.replace("apps-console-ci-gate-", ""));
const sent = () => vi.mocked(CustomAppsService.setPromotionChecks).mock.calls;

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Publishing & CI — with the promote-checks gate off", () => {
  it("has the four-eyes rule and the four gates it always had, and no checks settings", () => {
    for (const promotion_checks of [undefined, null, { ...DEFAULTS, enabled: false }]) {
      renderPanel(promotion_checks);
      expect(screen.queryByTestId("apps-console-ci-checks")).toBeNull();
      expect(screen.getByTestId("apps-console-ci-four-eyes")).toBeInTheDocument();
      expect(gateCodes()).toEqual([
        "staging_never_served",
        "validation_not_passed",
        "required_secrets_unset",
        "schedule_removal_unconfirmed"
      ]);
      cleanup();
    }
  });
});

describe("Publishing & CI — the promotion checks", () => {
  it("sits beside the four-eyes rule: a few words and a switch per row", () => {
    renderPanel(DEFAULTS);
    const settings = screen.getByTestId("apps-console-ci-checks");
    expect(screen.getByTestId("apps-console-ci-four-eyes").nextElementSibling).toBe(settings);
    expect(
      ROWS.map((name) => screen.getByTestId(`apps-console-ci-checks-${name}`).textContent)
    ).toEqual([
      "Its checks pass",
      "It loads, desk and phone",
      "Follows the design system",
      "Every screen opens",
      "A person has seen what changed",
      "Smoke production after promote",
      "Roll back automatically if it fails"
    ]);
    // Each label names its switch.
    expect(screen.getByLabelText("Every screen opens")).toBe(toggle("smoke"));
  });

  it("shows an app on the defaults as the defaults", () => {
    renderPanel(DEFAULTS);
    expect(states()).toEqual([true, true, true, true, false, true, false]);
    expect(gateCodes().slice(-2)).toEqual(["checks_not_passed", "checks_running"]);
  });

  it("shows what an app chose, and the seen gate once it is on", () => {
    renderPanel({
      ...DEFAULTS,
      required: ["functions", "smoke"],
      effective_required: ["functions", "smoke"],
      seen_required: true,
      auto_rollback: true
    });
    expect(states()).toEqual([true, false, false, true, true, true, true]);
    expect(gateCodes()).toContain("changes_not_seen");
  });

  it("saves a check turned off as the list the app then has", async () => {
    vi.mocked(CustomAppsService.setPromotionChecks).mockResolvedValue({} as CustomApp);
    renderPanel(DEFAULTS);
    await userEvent.click(toggle("design_system"));
    await vi.waitFor(() => expect(sent()).toHaveLength(1));
    expect(sent()[0]).toEqual([
      "app-1",
      {
        required: ["functions", "loads", "smoke"],
        seen_required: false,
        production_smoke: true,
        auto_rollback: false
      }
    ]);
  });

  it("saves another rule and leaves an app on the defaults on them", async () => {
    vi.mocked(CustomAppsService.setPromotionChecks).mockResolvedValue({} as CustomApp);
    renderPanel(DEFAULTS);
    await userEvent.click(toggle("seen_required"));
    await vi.waitFor(() => expect(sent()).toHaveLength(1));
    expect(sent()[0][1]).toEqual({
      required: null,
      seen_required: true,
      production_smoke: true,
      auto_rollback: false
    });
  });

  it("shows where a save is going while it is in flight, and holds every switch", async () => {
    let land: (app: CustomApp) => void = () => {};
    vi.mocked(CustomAppsService.setPromotionChecks).mockReturnValue(
      new Promise((resolve) => (land = resolve))
    );
    renderPanel(DEFAULTS);
    await userEvent.click(toggle("auto_rollback"));
    await vi.waitFor(() => expect(toggle("auto_rollback")).toBeDisabled());
    expect(states()).toEqual([true, true, true, true, false, true, true]);
    expect(ROWS.every((name) => (toggle(name) as HTMLButtonElement).disabled)).toBe(true);
    land({} as CustomApp);
    await vi.waitFor(() => expect(toggle("smoke")).toBeEnabled());
  });

  it("rests the automatic rollback while production is not smoked", () => {
    renderPanel({ ...DEFAULTS, production_smoke: false, auto_rollback: true });
    expect(toggle("auto_rollback")).toBeDisabled();
    expect(toggle("auto_rollback")).toHaveAttribute("aria-checked", "false");
    expect(toggle("production_smoke")).toBeEnabled();
  });
});
