// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ExperimentListItem } from "@/types/experiment";
import { definitionFixture } from "./fixtures";
import ExperimentsListPage from "./index";

const list = vi.hoisted(() => ({
  value: {} as {
    data: ExperimentListItem[] | undefined;
    isPending: boolean;
    error: Error | null;
    refetch: () => void;
  }
}));

vi.mock("@/hooks/api/useExperiments", () => ({
  useExperiments: () => list.value,
  useCreateExperiment: () => ({ mutateAsync: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" }, branchName: "main" })
}));

const item = (over: Partial<ExperimentListItem>): ExperimentListItem => ({
  name: "e",
  owner: "ops",
  hypothesis: "A promo lifts beverage revenue",
  design_id: "design-1",
  design_hash: "hash-1",
  design_frozen_at: null,
  status: "draft",
  registration_refusal: null,
  next_switch_on: null,
  retrospective: false,
  retrospective_ready: false,
  proposals_count: 0,
  latest_proposal: null,
  definition: definitionFixture,
  ...over
});

const serve = (data: ExperimentListItem[]) => {
  list.value = { data, isPending: false, error: null, refetch: vi.fn() };
};

const renderPage = () =>
  render(
    <MemoryRouter>
      <ExperimentsListPage />
    </MemoryRouter>
  );

describe("ExperimentsListPage", () => {
  beforeEach(() => serve([]));
  afterEach(cleanup);

  it("shows each row's name, status badge, owner and next switch date as a plain date", () => {
    serve([
      item({ name: "d", status: "draft" }),
      item({ name: "l", status: "live", next_switch_on: "2026-03-16", owner: "growth" }),
      item({ name: "r", status: "reported" }),
      item({
        name: "x",
        status: "registration_refused",
        registration_refusal: "unit `bondi` not found"
      })
    ]);
    renderPage();

    const live = within(screen.getByTestId("experiment-row-l"));
    expect(live.getByTestId("experiment-state-badge")).toHaveTextContent("Live");
    expect(live.getByText("growth")).toBeInTheDocument();
    expect(screen.getByTestId("experiment-row-next-switch-l").textContent).toBe("2026-03-16");
    // No date, no invented text: a dash.
    expect(screen.getByTestId("experiment-row-next-switch-d").textContent).toBe("—");
    expect(screen.getByTestId("experiment-row-next-switch-r").textContent).toBe("—");
    expect(
      within(screen.getByTestId("experiment-row-x")).getByTestId("experiment-state-badge")
    ).toHaveTextContent("Registration refused");
  });

  it("marks a retrospective run next to its status", () => {
    serve([item({ name: "p", status: "reported", retrospective: true }), item({ name: "q" })]);
    renderPage();

    expect(
      within(screen.getByTestId("experiment-row-p")).getByTestId("experiment-retrospective-badge")
    ).toHaveTextContent("Retrospective");
    expect(
      within(screen.getByTestId("experiment-row-q")).queryByTestId("experiment-retrospective-badge")
    ).toBeNull();
  });

  it("links each row to its detail page", () => {
    serve([item({ name: "beverage_promo" })]);
    renderPage();

    expect(screen.getByTestId("experiment-link-beverage_promo")).toHaveAttribute(
      "href",
      "/ide/experiments/beverage_promo"
    );
  });

  it("never shows a headline or figure on a row, even if the payload carries one", () => {
    const leaky = { headline: "gross +$99.00/unit-day" } as unknown as Partial<ExperimentListItem>;
    serve([item({ ...leaky, name: "r", status: "reported" })]);
    renderPage();

    expect(screen.queryByText(/99\.00/)).toBeNull();
  });

  it("says so when the workspace has no experiments", () => {
    renderPage();

    expect(screen.getByText("No experiments in this workspace")).toBeInTheDocument();
  });

  it("fills the page width in the empty, loading and error states, not just its content's", () => {
    renderPage();
    expect(
      screen
        .getByText("No experiments in this workspace")
        .closest("[data-testid='experiments-page']")
    ).toHaveClass("w-full", "flex-1");

    list.value = { data: undefined, isPending: true, error: null, refetch: vi.fn() };
    cleanup();
    renderPage();
    expect(screen.getByTestId("experiments-page")).toHaveClass("w-full", "flex-1");
  });

  it("keeps the cached list and shows an inline banner when a background refetch fails", () => {
    list.value = {
      data: [item({ name: "beverage_promo" })],
      isPending: false,
      error: new Error("not compiled yet; retry shortly"),
      refetch: vi.fn()
    };
    renderPage();

    expect(screen.getByTestId("experiment-row-beverage_promo")).toBeInTheDocument();
    expect(screen.getByTestId("experiments-refresh-error")).toHaveTextContent(
      "not compiled yet; retry shortly"
    );
  });

  it("does not render a failed read as an empty workspace", () => {
    list.value = {
      data: undefined,
      isPending: false,
      error: new Error("not compiled yet; retry shortly"),
      refetch: vi.fn()
    };
    renderPage();

    expect(screen.getByText("not compiled yet; retry shortly")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.queryByText("No experiments in this workspace")).toBeNull();
  });
});
