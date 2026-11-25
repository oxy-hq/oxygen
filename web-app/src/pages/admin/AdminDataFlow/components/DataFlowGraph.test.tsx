// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DataFlow } from "@/types/dataFlow";

vi.mock("@/components/graph/elkLayout", () => ({ layoutColumns: vi.fn() }));

// jsdom lays nothing out, so the canvas is a stub that lists where each node sits.
vi.mock("@xyflow/react", () => ({
  ReactFlowProvider: ({ children }: { children: ReactNode }) => <>{children}</>,
  ReactFlow: ({ nodes }: { nodes: { id: string; position: { x: number; y: number } }[] }) => (
    <ul data-testid='stub-canvas'>
      {nodes.map((n) => (
        <li key={n.id}>{`${n.id}@${n.position.x},${n.position.y}`}</li>
      ))}
    </ul>
  ),
  Background: () => null,
  BackgroundVariant: { Dots: "dots" },
  Controls: () => null,
  Handle: () => null,
  MarkerType: { ArrowClosed: "arrowclosed" },
  Position: { Left: "left", Right: "right" }
}));

import { layoutColumns } from "@/components/graph/elkLayout";
import { DataFlowGraph } from "./DataFlowGraph";

const flow: DataFlow = {
  generated_at: "2026-11-25T00:00:00Z",
  org: { id: "o", name: "Org", slug: "org" },
  workspaces: [],
  truncated: false,
  level: "schema",
  nodes: [
    { id: "source:w:s", type: "source", label: "s", meta: {} },
    { id: "pipe:w:s", type: "pipeline", label: "s", meta: {} }
  ],
  edges: [{ from: "source:w:s", to: "pipe:w:s", kind: "reads", declared: true, via: ["airway"] }]
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("DataFlowGraph", () => {
  it("draws ELK's placement when it answers", async () => {
    vi.mocked(layoutColumns).mockImplementation(async (nodes) =>
      nodes.map((n, i) => ({ ...n, position: { x: i * 10, y: 7 } }))
    );
    render(<DataFlowGraph flow={flow} focus={null} selected={null} onSelect={vi.fn()} />);
    expect(await screen.findByText("pipe:w:s@10,7")).toBeTruthy();
    expect(screen.queryByTestId("admin-data-flow-layout-failed")).toBeNull();
  });

  it("stacks the nodes by column and says why when ELK refuses the graph", async () => {
    vi.mocked(layoutColumns).mockRejectedValue(new Error("partition constraint violated"));
    const quiet = vi.spyOn(console, "error").mockImplementation(() => undefined);
    render(<DataFlowGraph flow={flow} focus={null} selected={null} onSelect={vi.fn()} />);

    const note = await screen.findByTestId("admin-data-flow-layout-failed");
    expect(note.textContent).toContain("partition constraint violated");
    // Not stuck on "Laying out…": the canvas draws, source left of pipeline.
    expect(screen.queryByText(/Laying out/)).toBeNull();
    expect(screen.getByText("source:w:s@0,0")).toBeTruthy();
    expect(screen.getByText(/^pipe:w:s@[1-9]\d*,0$/)).toBeTruthy();
    quiet.mockRestore();
  });
});
