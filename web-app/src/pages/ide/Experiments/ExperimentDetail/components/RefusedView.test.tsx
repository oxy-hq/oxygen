// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ExperimentListItem } from "@/types/experiment";
import { definitionFixture } from "../../fixtures";
import { COLUMN } from "../column";
import RefusedView from "./RefusedView";

const h = vi.hoisted(() => ({ retro: vi.fn(), admin: true }));

vi.mock("@/hooks/api/useExperiments", () => ({
  useExperimentDesign: () => ({ data: { id: "design-2", content_hash: "hash-2" } }),
  useRetrospectiveExperiment: () => ({ mutateAsync: h.retro, isPending: false })
}));
vi.mock("@/hooks/useRole", () => ({
  useRole: () => ({ is: { workspaceAdmin: h.admin, workspaceEditor: true } })
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const summary = (over: Partial<ExperimentListItem> = {}): ExperimentListItem => ({
  name: "e",
  owner: "ops",
  hypothesis: "h",
  design_id: "design-2",
  design_hash: "hash-2",
  design_frozen_at: null,
  status: "registration_refused",
  registration_refusal: "the first switch date has passed",
  next_switch_on: null,
  retrospective: false,
  retrospective_ready: true,
  proposals_count: 0,
  latest_proposal: null,
  definition: definitionFixture,
  ...over
});

beforeEach(() => {
  h.admin = true;
  h.retro.mockReset().mockResolvedValue({ run_id: "r", design_id: "design-2" });
});
afterEach(cleanup);

describe("RefusedView — retrospective offer", () => {
  it("explains the past-data analysis and runs it on the design's content hash", async () => {
    const user = userEvent.setup();
    render(<RefusedView summary={summary()} />);

    const offer = screen.getByTestId("experiment-retrospective-offer");
    expect(offer).toHaveTextContent("in the past");
    expect(offer).toHaveTextContent("not pre-registered");
    expect(offer).toHaveTextContent("no levers are switched");

    await user.click(screen.getByTestId("experiment-retrospective"));
    expect(h.retro).toHaveBeenCalledWith("hash-2");
  });

  it("is absent when the design cannot be analysed retrospectively", () => {
    render(<RefusedView summary={summary({ retrospective_ready: false })} />);
    expect(screen.queryByTestId("experiment-retrospective-offer")).toBeNull();
    expect(screen.getByTestId("experiment-refused")).toBeInTheDocument();
  });

  it("shows the server's refusal verbatim on error", async () => {
    h.retro.mockRejectedValue(new Error("unit `bondi` has no data before 2026-01-01"));
    const user = userEvent.setup();
    render(<RefusedView summary={summary()} />);

    await user.click(screen.getByTestId("experiment-retrospective"));
    expect(await screen.findByTestId("experiment-retrospective-error")).toHaveTextContent(
      "unit `bondi` has no data before 2026-01-01"
    );
  });

  it("is disabled below workspace admin", () => {
    h.admin = false;
    render(<RefusedView summary={summary()} />);
    expect(screen.getByTestId("experiment-retrospective")).toBeDisabled();
  });
});

describe("RefusedView — at a glance", () => {
  it("leads with a refused badge, refusal still verbatim", () => {
    render(<RefusedView summary={summary()} />);

    expect(screen.getByTestId("experiment-refused-status")).toHaveTextContent(
      "Registration refused"
    );
    expect(screen.getByTestId("experiment-refused-text").textContent).toBe(
      "the first switch date has passed"
    );
    expect(screen.getByTestId("experiment-refused-view")).toHaveTextContent(
      "Edit the design; the platform re-checks it on its next pass."
    );
    expect(screen.getByTestId("experiment-refused-view")).not.toHaveTextContent(/version/i);
  });

  it("sits in the page's shared column, aligned with the Definition", () => {
    render(<RefusedView summary={summary()} />);
    const root = screen.getByTestId("experiment-refused-view");
    for (const c of COLUMN.split(" ")) expect(root).toHaveClass(c);
  });
});
