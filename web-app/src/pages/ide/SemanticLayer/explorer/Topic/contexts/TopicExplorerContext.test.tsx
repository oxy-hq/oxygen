// @vitest-environment jsdom
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TopicExplorerProvider, useTopicExplorerContext } from "./TopicExplorerContext";

const useTopicDetails = vi.fn();
vi.mock("@/hooks/api/useSemanticQuery", () => ({
  useTopicDetails: (pathb64: string | undefined) => useTopicDetails(pathb64),
  useExecuteSemanticQuery: () => ({ mutate: vi.fn(), isPending: false }),
  useCompileSemanticQuery: () => ({ mutate: vi.fn(), isPending: false })
}));

function Probe() {
  useTopicExplorerContext();
  return <div data-testid='probe-ok' />;
}

describe("TopicExplorerProvider", () => {
  afterEach(() => useTopicDetails.mockReset());

  it("uses the explicit pathb64 prop without an editor context", () => {
    useTopicDetails.mockReturnValue({
      data: undefined,
      isLoading: false,
      error: undefined,
      refetch: vi.fn()
    });
    render(
      <TopicExplorerProvider pathb64='dG9waWM='>
        <Probe />
      </TopicExplorerProvider>
    );
    expect(screen.getByTestId("probe-ok")).toBeInTheDocument();
    expect(useTopicDetails).toHaveBeenCalledWith("dG9waWM=");
  });

  // Regression (PR #2620): the topic explorer has the same keyed-provider
  // contract as the view explorer — switching topics must reset selection so a
  // field from one topic doesn't leak into the next topic's query.
  it("clears field selection when the topic (pathb64) changes", () => {
    useTopicDetails.mockReturnValue({
      data: {
        views: [
          {
            view_name: "orders",
            name: "orders",
            dimensions: [{ name: "amount", type: "number" }],
            measures: []
          }
        ],
        topic: { name: "sales", views: ["orders"], base_view: "orders" }
      },
      isLoading: false,
      error: undefined,
      refetch: vi.fn()
    });

    let ctx!: ReturnType<typeof useTopicExplorerContext>;
    function Capture() {
      ctx = useTopicExplorerContext();
      return <div data-testid='dims'>{ctx.selectedDimensions.join(",")}</div>;
    }

    const { rerender } = render(
      <TopicExplorerProvider pathb64='topic-a'>
        <Capture />
      </TopicExplorerProvider>
    );

    act(() => ctx.toggleDimension("orders.amount"));
    expect(screen.getByTestId("dims").textContent).toBe("orders.amount");

    rerender(
      <TopicExplorerProvider pathb64='topic-b'>
        <Capture />
      </TopicExplorerProvider>
    );
    expect(screen.getByTestId("dims").textContent).toBe("");
  });
});
