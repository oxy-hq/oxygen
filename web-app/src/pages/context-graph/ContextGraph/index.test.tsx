// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

// Mock ReactFlow entirely — it requires browser APIs not available in jsdom
vi.mock("@xyflow/react", () => ({
  ReactFlow: ({
    nodes,
    onNodeClick,
    onPaneClick,
    children
  }: {
    nodes: Array<{ id: string; data: { label: string } }>;
    onNodeClick: (event: React.MouseEvent, node: { id: string }) => void;
    onPaneClick: () => void;
    children: React.ReactNode;
  }) => (
    <div data-testid='react-flow'>
      {nodes.map((n) => (
        <div key={n.id} data-testid={`rf-node-${n.id}`} onClick={(e) => onNodeClick(e, n)}>
          {n.data.label}
        </div>
      ))}
      <div data-testid='rf-pane' onClick={onPaneClick} />
      {children}
    </div>
  ),
  ReactFlowProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  Background: () => null,
  BackgroundVariant: { Dots: "dots" },
  Panel: ({
    children,
    className
  }: {
    children: React.ReactNode;
    className?: string;
    position?: string;
  }) => (
    <div data-testid='rf-panel' className={className}>
      {children}
    </div>
  ),
  Handle: () => null,
  Position: { Left: "left", Right: "right" },
  useNodesState: (initial: unknown[]) => {
    const [nodes, setNodes] = useState(initial);
    return [nodes, setNodes, vi.fn()];
  },
  useEdgesState: (initial: unknown[]) => {
    const [edges, setEdges] = useState(initial);
    return [edges, setEdges, vi.fn()];
  }
}));

// Mock NodeDetailPanel to observe props
vi.mock("./components/NodeDetailPanel", () => ({
  NodeDetailPanel: ({ node, onClose }: { node: unknown; onClose: () => void }) => (
    <div data-testid='node-detail-panel' data-has-node={node !== null ? "true" : "false"}>
      {node !== null && (
        <button data-testid='close-panel' onClick={onClose}>
          Close
        </button>
      )}
    </div>
  )
}));

// Mock react-router-dom (may be needed by transitive imports)
vi.mock("react-router-dom", () => ({
  useNavigate: () => vi.fn()
}));

const { ContextGraph } = await import("./index");

afterEach(() => cleanup());

const mockData = {
  nodes: [
    { id: "a1", type: "agent" as const, label: "Agent 1", data: { name: "agent1" } },
    { id: "t1", type: "table" as const, label: "Table 1", data: { name: "table1" } }
  ],
  edges: [{ id: "e1", source: "a1", target: "t1" }],
  tablesUnknown: false
};

describe("ContextGraph orchestrator", () => {
  it("renders ReactFlow with correct number of nodes", () => {
    render(<ContextGraph data={mockData} />);
    expect(screen.getByTestId("react-flow")).toBeInTheDocument();
    expect(screen.getByTestId("rf-node-a1")).toBeInTheDocument();
    expect(screen.getByTestId("rf-node-t1")).toBeInTheDocument();
  });

  it("node click shows NodeDetailPanel", () => {
    render(<ContextGraph data={mockData} />);
    expect(screen.getByTestId("node-detail-panel").dataset.hasNode).toBe("false");

    fireEvent.click(screen.getByTestId("rf-node-a1"));
    expect(screen.getByTestId("node-detail-panel").dataset.hasNode).toBe("true");
  });

  it("pane click hides NodeDetailPanel", () => {
    render(<ContextGraph data={mockData} />);

    fireEvent.click(screen.getByTestId("rf-node-a1"));
    expect(screen.getByTestId("node-detail-panel").dataset.hasNode).toBe("true");

    fireEvent.click(screen.getByTestId("rf-pane"));
    expect(screen.getByTestId("node-detail-panel").dataset.hasNode).toBe("false");
  });
});
