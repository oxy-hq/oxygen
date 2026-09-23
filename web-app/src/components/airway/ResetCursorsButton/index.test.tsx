// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ResetCursorsOutcome, ResetCursorsRequest } from "@/services/api/airway";

const mutateAsync = vi.fn<(r: ResetCursorsRequest) => Promise<ResetCursorsOutcome>>();
const resourceCursors = vi.fn();
const pending = vi.hoisted(() => ({ current: false }));

vi.mock("@/hooks/api/airway/useAirway", () => ({
  useAirwayResourceCursors: (pipelineRef: string, enabled: boolean) =>
    resourceCursors(pipelineRef, enabled),
  useResetCursors: () => ({ mutateAsync, isPending: pending.current })
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() }
}));

import ResetCursorsButton from "./index";

/**
 * The container's job is to keep the refusal readable and the override honest.
 *
 * `RefusalPanel.test.tsx` covers what a refusal looks like; this covers when one
 * is on screen at all, and — the case that actually bites — what happens to a
 * revealed override when the operator changes their mind about the scope.
 */

const APPEND_ONLY =
  "`vendor_forecasting` appends with no merge key, so re-pulling would duplicate rows rather " +
  "than converge. Use Reset schema if you intend to drop and rebuild";

beforeEach(() => {
  pending.current = false;
  mutateAsync.mockReset();
  resourceCursors.mockReset();
  resourceCursors.mockReturnValue({
    data: ["vendor_forecasting", "vendor_sales"],
    isLoading: false,
    isError: false,
    error: null
  });
});

afterEach(cleanup);

const open = () => {
  render(<ResetCursorsButton pipelineRef='pipelines/amazon_vc.airway.yml' />);
  fireEvent.click(screen.getByTestId("airway-rewind-cursors-button"));
};

const check = (resource: string) =>
  fireEvent.click(screen.getByRole("checkbox", { name: resource }));

const confirm = () => screen.getByTestId("airway-rewind-cursors-confirm");

const refuse = () =>
  mutateAsync.mockResolvedValue({ kind: "refused", reasons: [APPEND_ONLY] } as ResetCursorsOutcome);

describe("ResetCursorsButton — the safe reset carries the weight", () => {
  it("is the outlined button, so Reset schema beside it is not the obvious one", () => {
    render(<ResetCursorsButton pipelineRef='pipelines/amazon_vc.airway.yml' />);
    // The safety property is the *relative* weight of the two buttons;
    // `ResetSchemaButton.test.tsx` pins the other half. `border-input` is the
    // class `outline` really emits.
    expect(screen.getByTestId("airway-rewind-cursors-button").className).toContain("border-input");
  });
});

describe("ResetCursorsButton — nothing is selected by default", () => {
  it("cannot be submitted until a resource is picked", () => {
    open();
    // The route reads an empty `resources` as *every* resource. On `amazon_vc`
    // that is the one choice an append-only sibling refuses, so a default of
    // "all" would make an operator's first click a refusal — and the first
    // thing they meet would be the override.
    expect(confirm()).toBeDisabled();

    check("vendor_sales");
    expect(confirm()).toBeEnabled();
  });

  it("sends only the resources that were picked", async () => {
    mutateAsync.mockResolvedValue({
      kind: "cleared",
      cleared: ["vendor_sales"],
      not_held: []
    } as ResetCursorsOutcome);
    open();
    check("vendor_sales");
    fireEvent.click(confirm());

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync).toHaveBeenCalledWith({
      pipeline_ref: "pipelines/amazon_vc.airway.yml",
      resources: ["vendor_sales"],
      force: false
    });
  });

  it("sends nothing at all when the operator changes their mind back to none", async () => {
    mutateAsync.mockResolvedValue({ kind: "cleared", cleared: [], not_held: [] });
    open();
    check("vendor_sales");
    check("vendor_sales");
    // `[]` on the wire means *every* resource, so an empty selection must be
    // unsubmittable rather than sent.
    expect(confirm()).toBeDisabled();
    expect(mutateAsync).not.toHaveBeenCalled();
  });
});

/**
 * `[]` on the wire is the route's all-resources scope, resolved on the server
 * against the cursors held when the request lands — which can include one
 * that appeared after this picker loaded. So the UI always sends what the
 * operator was shown and picked. The server judges a list naming every held
 * cursor as the whole pipeline, so this costs no `UnknownOwnership` refusal.
 */
describe("ResetCursorsButton — a whole-pipeline rewind sends the names it showed", () => {
  const cleared = { kind: "cleared", cleared: [], not_held: [] } as ResetCursorsOutcome;

  it("sends every shown name, not `[]`, when the every-resource row is ticked", async () => {
    mutateAsync.mockResolvedValue(cleared);
    open();
    fireEvent.click(screen.getByTestId("airway-cursor-every-resource"));
    fireEvent.click(confirm());

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        pipeline_ref: "pipelines/amazon_vc.airway.yml",
        resources: ["vendor_forecasting", "vendor_sales"],
        force: false
      })
    );
  });

  it("sends the same names when every box is ticked one by one", async () => {
    mutateAsync.mockResolvedValue(cleared);
    open();
    check("vendor_forecasting");
    check("vendor_sales");
    fireEvent.click(confirm());

    // Two routes to one intent take one path.
    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        pipeline_ref: "pipelines/amazon_vc.airway.yml",
        resources: ["vendor_forecasting", "vendor_sales"],
        force: false
      })
    );
  });

  it("checks the every-resource row once the last box is ticked", () => {
    open();
    const every = screen.getByTestId("airway-cursor-every-resource");
    expect(every).toHaveAttribute("data-state", "unchecked");

    check("vendor_forecasting");
    expect(every).toHaveAttribute("data-state", "unchecked");

    check("vendor_sales");
    // The equivalence has to be visible, or the operator cannot tell that the
    // last tick changed which scope they are about to send.
    expect(every).toHaveAttribute("data-state", "checked");
  });

  it("clears back to nothing when the every-resource row is unticked", () => {
    open();
    fireEvent.click(screen.getByTestId("airway-cursor-every-resource"));
    expect(confirm()).toBeEnabled();

    fireEvent.click(screen.getByTestId("airway-cursor-every-resource"));
    expect(confirm()).toBeDisabled();
  });

  /**
   * `held` is live query data. A refetch that shrinks it under an open dialog
   * used to make a partial selection's *count* equal the list's, and the
   * partial selection went out as `[]` — every resource — clearing one the
   * operator never picked.
   */
  it("keeps a partial selection explicit when the held list shrinks under it", async () => {
    mutateAsync.mockResolvedValue(cleared);
    const { rerender } = render(
      <ResetCursorsButton pipelineRef='pipelines/amazon_vc.airway.yml' />
    );
    fireEvent.click(screen.getByTestId("airway-rewind-cursors-button"));
    check("vendor_sales");

    // `vendor_sales`' cursor was cleared elsewhere; the refetch drops it.
    resourceCursors.mockReturnValue({
      data: ["vendor_forecasting"],
      isLoading: false,
      isError: false,
      error: null
    });
    rerender(<ResetCursorsButton pipelineRef='pipelines/amazon_vc.airway.yml' />);

    expect(screen.getByTestId("airway-cursor-every-resource")).toHaveAttribute(
      "data-state",
      "unchecked"
    );
    fireEvent.click(confirm());
    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        pipeline_ref: "pipelines/amazon_vc.airway.yml",
        resources: ["vendor_sales"],
        force: false
      })
    );
  });

  it("keeps a partial selection on the explicit scope", async () => {
    mutateAsync.mockResolvedValue(cleared);
    open();
    check("vendor_sales");
    fireEvent.click(confirm());

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        pipeline_ref: "pipelines/amazon_vc.airway.yml",
        resources: ["vendor_sales"],
        force: false
      })
    );
  });
});

describe("ResetCursorsButton — a refusal stays on screen", () => {
  it("keeps the dialog open and renders the reason rather than toasting it", async () => {
    refuse();
    open();
    check("vendor_forecasting");
    fireEvent.click(confirm());

    const reasons = await screen.findByTestId("airway-cursor-refusal-reasons");
    expect(reasons.querySelector("li")?.textContent).toBe(APPEND_ONLY);
    // Still in the dialog: ninety words is not something a toast can hold long
    // enough to read.
    expect(screen.getByTestId("airway-cursor-picker")).toBeInTheDocument();
  });

  it("blocks the plain confirm once refused, so the only way on is the override", async () => {
    refuse();
    open();
    check("vendor_forecasting");
    fireEvent.click(confirm());

    await screen.findByTestId("airway-cursor-refusal");
    expect(confirm()).toBeDisabled();
  });

  it("forces only after the override is disclosed, and says so on the wire", async () => {
    refuse();
    open();
    check("vendor_forecasting");
    fireEvent.click(confirm());
    await screen.findByTestId("airway-cursor-refusal");

    fireEvent.click(screen.getByTestId("airway-cursor-reveal-override"));
    mutateAsync.mockResolvedValue({
      kind: "cleared",
      cleared: ["vendor_forecasting"],
      not_held: []
    } as ResetCursorsOutcome);
    fireEvent.click(screen.getByRole("button", { name: "Rewind anyway" }));

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenLastCalledWith({
        pipeline_ref: "pipelines/amazon_vc.airway.yml",
        resources: ["vendor_forecasting"],
        force: true
      })
    );
  });
});

/**
 * A refusal is only useful if it lands somewhere it can be read. Dismissed
 * mid-request, the dialog used to close under the request; the `409` then
 * wrote its reasons into state nothing rendered, and the next open wiped them.
 */
describe("ResetCursorsButton — a pending rewind cannot be dismissed", () => {
  it("disables Cancel while the request is in flight", () => {
    pending.current = true;
    open();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
  });

  it("stays open on Escape while the request is in flight", () => {
    pending.current = true;
    open();
    fireEvent.keyDown(screen.getByTestId("airway-cursor-picker"), { key: "Escape" });
    expect(screen.getByTestId("airway-cursor-picker")).toBeInTheDocument();
  });

  it("still closes on Escape once nothing is pending", async () => {
    open();
    fireEvent.keyDown(screen.getByTestId("airway-cursor-picker"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("airway-cursor-picker")).toBeNull());
  });
});

/**
 * A run in flight holds the pipeline lease, and a reset landing then would
 * break that run's cursor save. The server refuses, and `force` cannot get past
 * it — so the dialog must say why without offering an override that would
 * only be refused again.
 */
describe("ResetCursorsButton — a run in flight is not a refusal to override", () => {
  const RUNNING =
    "cursor reset refused: run `run-123` of this pipeline is in flight. Wait for it to finish, " +
    "or cancel it, then rewind";

  it("says a run is in flight and offers no override", async () => {
    mutateAsync.mockResolvedValue({
      kind: "pipeline_running",
      run_id: "run-123",
      message: RUNNING
    } as ResetCursorsOutcome);
    open();
    check("vendor_sales");
    fireEvent.click(confirm());

    const notice = await screen.findByTestId("airway-cursor-pipeline-running");
    expect(notice.textContent).toBe(RUNNING);
    expect(screen.queryByTestId("airway-cursor-refusal")).toBeNull();
    expect(screen.queryByTestId("airway-cursor-reveal-override")).toBeNull();
    // Retrying once the run ends is the way on, so the confirm stays live.
    expect(confirm()).toBeEnabled();
  });
});

describe("ResetCursorsButton — a refusal answers one scope, and only that scope", () => {
  it("retracts the refusal when the selection changes", async () => {
    refuse();
    open();
    check("vendor_forecasting");
    fireEvent.click(confirm());
    await screen.findByTestId("airway-cursor-refusal");

    // A different set of resources is a different question; the answer to the
    // old one no longer describes what would happen.
    check("vendor_sales");
    expect(screen.queryByTestId("airway-cursor-refusal")).toBeNull();
    expect(confirm()).toBeEnabled();
  });

  it("retracts a revealed override too, so force cannot survive into a new scope", async () => {
    refuse();
    open();
    check("vendor_forecasting");
    fireEvent.click(confirm());
    await screen.findByTestId("airway-cursor-refusal");
    fireEvent.click(screen.getByTestId("airway-cursor-reveal-override"));
    expect(screen.getByRole("button", { name: "Rewind anyway" })).toBeInTheDocument();

    check("vendor_sales");
    // An armed `force` carried across a selection change would override reasons
    // that were never about the request it ends up sending.
    expect(screen.queryByRole("button", { name: "Rewind anyway" })).toBeNull();
  });

  it("re-arms nothing when the dialog is reopened", async () => {
    refuse();
    open();
    check("vendor_forecasting");
    fireEvent.click(confirm());
    await screen.findByTestId("airway-cursor-refusal");
    fireEvent.click(screen.getByTestId("airway-cursor-reveal-override"));

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByTestId("airway-rewind-cursors-button"));

    expect(screen.queryByTestId("airway-cursor-refusal")).toBeNull();
    expect(confirm()).toBeDisabled();
  });
});

describe("ResetCursorsButton — what it says about itself", () => {
  it("leads with the fact that nothing is dropped", () => {
    open();
    expect(screen.getByText("Rewind cursors — nothing is dropped")).toBeInTheDocument();
    expect(screen.getByText(/Every landed row stays/)).toBeInTheDocument();
  });

  it("names the other reset's consequence rather than leaving it to the verb", () => {
    open();
    expect(screen.getByText(/drops every destination table/)).toBeInTheDocument();
  });

  it("explains an empty cursor list instead of offering an empty picker", () => {
    resourceCursors.mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null
    });
    open();
    expect(screen.getByTestId("airway-cursor-picker-empty").textContent).toContain(
      "holds no cursors"
    );
    expect(confirm()).toBeDisabled();
  });

  it("only asks for the cursor list once the dialog is open", () => {
    render(<ResetCursorsButton pipelineRef='pipelines/amazon_vc.airway.yml' />);
    expect(resourceCursors).toHaveBeenLastCalledWith("pipelines/amazon_vc.airway.yml", false);

    fireEvent.click(screen.getByTestId("airway-rewind-cursors-button"));
    expect(resourceCursors).toHaveBeenLastCalledWith("pipelines/amazon_vc.airway.yml", true);
  });
});
