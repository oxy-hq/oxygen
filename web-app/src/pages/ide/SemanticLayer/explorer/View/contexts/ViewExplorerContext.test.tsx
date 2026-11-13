// @vitest-environment jsdom
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useViewExplorerContext, ViewExplorerProvider } from "./ViewExplorerContext";

const useViewDetails = vi.fn();
vi.mock("@/hooks/api/useSemanticQuery", () => ({
  useViewDetails: (pathb64: string | undefined) => useViewDetails(pathb64),
  useExecuteSemanticQuery: () => ({ mutate: vi.fn(), isPending: false }),
  useCompileSemanticQuery: () => ({ mutate: vi.fn(), isPending: false })
}));

function Probe() {
  useViewExplorerContext();
  return <div data-testid='probe-ok' />;
}

describe("ViewExplorerProvider", () => {
  afterEach(() => useViewDetails.mockReset());

  it("uses the explicit pathb64 prop without an editor context", () => {
    useViewDetails.mockReturnValue({
      data: undefined,
      isLoading: false,
      error: undefined,
      refetch: vi.fn()
    });
    render(
      <ViewExplorerProvider pathb64='dmlldw=='>
        <Probe />
      </ViewExplorerProvider>
    );
    expect(screen.getByTestId("probe-ok")).toBeInTheDocument();
    expect(useViewDetails).toHaveBeenCalledWith("dmlldw==");
  });

  // Regression (PR #2620): switching to a different view must reset the
  // Explorer's field selection. The provider is keyed by pathb64, so a path
  // change remounts it and clears local state. Without the key it would only
  // re-render, leaking the previous view's field into the next view's query
  // (the cross-view "No valid join tree found" bug).
  it("clears field selection when the view (pathb64) changes", () => {
    useViewDetails.mockReturnValue({
      data: { name: "orders", dimensions: [{ name: "amount", type: "number" }], measures: [] },
      isLoading: false,
      error: undefined,
      refetch: vi.fn()
    });

    let ctx!: ReturnType<typeof useViewExplorerContext>;
    function Capture() {
      ctx = useViewExplorerContext();
      return <div data-testid='dims'>{ctx.selectedDimensions.join(",")}</div>;
    }

    const { rerender } = render(
      <ViewExplorerProvider pathb64='view-a'>
        <Capture />
      </ViewExplorerProvider>
    );

    // Select a field while viewing the first file.
    act(() => ctx.toggleDimension("orders.amount"));
    expect(screen.getByTestId("dims").textContent).toBe("orders.amount");

    // Switch to a different view file — selection must reset.
    rerender(
      <ViewExplorerProvider pathb64='view-b'>
        <Capture />
      </ViewExplorerProvider>
    );
    expect(screen.getByTestId("dims").textContent).toBe("");
  });
});
