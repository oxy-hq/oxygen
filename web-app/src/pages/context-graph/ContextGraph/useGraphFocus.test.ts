// @vitest-environment jsdom

import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  buildNeighbors,
  computeFocusTypeVisible,
  getConnectedNodes,
  useGraphFocus
} from "./useGraphFocus";

// --- Pure function tests ---

describe("buildNeighbors", () => {
  it("builds bidirectional adjacency map from edges", () => {
    const edges = [
      { source: "a", target: "b" },
      { source: "b", target: "c" }
    ];
    const map = buildNeighbors(edges);
    expect(map.get("a")).toEqual(new Set(["b"]));
    expect(map.get("b")).toEqual(new Set(["a", "c"]));
    expect(map.get("c")).toEqual(new Set(["b"]));
  });
});

describe("getConnectedNodes", () => {
  const neighbors = new Map<string, Set<string>>([
    ["a", new Set(["b", "c"])],
    ["b", new Set(["a", "d"])],
    ["c", new Set(["a"])],
    ["d", new Set(["b", "e"])],
    ["e", new Set(["d"])]
  ]);

  it("returns direct neighbors at depth 1", () => {
    const result = getConnectedNodes(neighbors, ["a"], 1);
    // a itself + direct neighbors b, c
    expect(result).toEqual(new Set(["a", "b", "c"]));
  });

  it("returns full connected cluster with no depth limit", () => {
    const result = getConnectedNodes(neighbors, ["a"]);
    expect(result).toEqual(new Set(["a", "b", "c", "d", "e"]));
  });

  it("returns only start node at depth 0", () => {
    const result = getConnectedNodes(neighbors, ["a"], 0);
    expect(result).toEqual(new Set(["a"]));
  });
});

describe("computeFocusTypeVisible", () => {
  it("returns null for 'auto' focus type", () => {
    expect(computeFocusTypeVisible("auto", [], new Map())).toBeNull();
  });

  it("returns correct node set for a given type", () => {
    const nodes = [
      { id: "n1", type: "agent" },
      { id: "n2", type: "table" },
      { id: "n3", type: "agent" },
      { id: "n4", type: "view" }
    ];
    const neighbors = buildNeighbors([
      { source: "n1", target: "n2" },
      { source: "n3", target: "n2" }
    ]);
    const result = computeFocusTypeVisible("agent", nodes, neighbors);
    // Agents n1, n3 + their connected table n2
    expect(result).toEqual(new Set(["n1", "n2", "n3"]));
    expect(result?.has("n4")).toBe(false);
  });
});

// --- Hook tests ---

describe("useGraphFocus", () => {
  const mockData = {
    nodes: [
      { id: "n1", type: "agent" as const, label: "Agent 1", data: { name: "agent1" } },
      { id: "n2", type: "table" as const, label: "Table 1", data: { name: "table1" } },
      { id: "n3", type: "agent" as const, label: "Agent 2", data: { name: "agent2" } },
      { id: "n4", type: "view" as const, label: "View 1", data: { name: "view1" } }
    ],
    edges: [
      { id: "e1", source: "n1", target: "n2" },
      { id: "e2", source: "n3", target: "n2" }
    ],
    tablesUnknown: false
  };

  afterEach(() => {
    localStorage.clear();
  });

  it("initializes with null focusedNodeId and selectedNode", () => {
    const { result } = renderHook(() =>
      useGraphFocus({ data: mockData, setNodes: vi.fn(), setEdges: vi.fn() })
    );
    expect(result.current.focusedNodeId).toBeNull();
    expect(result.current.selectedNode).toBeNull();
  });

  it("handleNodeClick toggles focusedNodeId on/off", () => {
    const { result } = renderHook(() =>
      useGraphFocus({ data: mockData, setNodes: vi.fn(), setEdges: vi.fn() })
    );

    act(() => {
      result.current.handleNodeClick({} as React.MouseEvent, { id: "n1" });
    });
    expect(result.current.focusedNodeId).toBe("n1");
    expect(result.current.selectedNode?.id).toBe("n1");

    act(() => {
      result.current.handleNodeClick({} as React.MouseEvent, { id: "n1" });
    });
    expect(result.current.focusedNodeId).toBeNull();
    expect(result.current.selectedNode).toBeNull();
  });

  it("handlePaneClick clears both focusedNodeId and selectedNode", () => {
    const { result } = renderHook(() =>
      useGraphFocus({ data: mockData, setNodes: vi.fn(), setEdges: vi.fn() })
    );

    act(() => {
      result.current.handleNodeClick({} as React.MouseEvent, { id: "n1" });
    });
    expect(result.current.focusedNodeId).toBe("n1");

    act(() => {
      result.current.handlePaneClick();
    });
    expect(result.current.focusedNodeId).toBeNull();
    expect(result.current.selectedNode).toBeNull();
  });

  it("changeFocusType updates focusType and clears focus/selection", () => {
    const { result } = renderHook(() =>
      useGraphFocus({ data: mockData, setNodes: vi.fn(), setEdges: vi.fn() })
    );

    act(() => {
      result.current.handleNodeClick({} as React.MouseEvent, { id: "n1" });
    });
    expect(result.current.focusedNodeId).toBe("n1");

    act(() => {
      result.current.changeFocusType("table");
    });
    expect(result.current.focusType).toBe("table");
    expect(result.current.focusedNodeId).toBeNull();
    expect(result.current.selectedNode).toBeNull();
  });

  it("persists focusType to localStorage", () => {
    const { result } = renderHook(() =>
      useGraphFocus({ data: mockData, setNodes: vi.fn(), setEdges: vi.fn() })
    );

    act(() => {
      result.current.changeFocusType("view");
    });
    expect(localStorage.getItem("context-graph-focus-type")).toBe("view");
  });

  // --- useEffect integration: verify setNodes/setEdges updater output ---

  // Helper: build ReactFlow-shaped nodes from mockData
  function toFlowNodes(data: typeof mockData) {
    return data.nodes.map((n) => ({
      id: n.id,
      position: { x: 0, y: 0 },
      data: { ...n.data, type: n.type, opacity: 1, showLeftHandle: false, showRightHandle: false }
    }));
  }

  function toFlowEdges(data: typeof mockData) {
    return data.edges.map((e) => ({
      id: e.id,
      source: e.source,
      target: e.target,
      animated: false,
      style: { stroke: "", strokeWidth: 1, opacity: 0.15, strokeDasharray: "none" }
    }));
  }

  function lastUpdaterResult<T>(mock: ReturnType<typeof vi.fn>, input: T[]): T[] {
    const lastCall = mock.mock.calls[mock.mock.calls.length - 1];
    const updater = lastCall[0];
    return typeof updater === "function" ? updater(input) : updater;
  }

  it("sets connected nodes to opacity 1 and disconnected to opacity 0 on node click", () => {
    const setNodes = vi.fn();
    const setEdges = vi.fn();
    const { result } = renderHook(() => useGraphFocus({ data: mockData, setNodes, setEdges }));

    act(() => {
      result.current.handleNodeClick({} as React.MouseEvent, { id: "n1" });
    });

    const updatedNodes = lastUpdaterResult(setNodes, toFlowNodes(mockData));
    const opacityById = Object.fromEntries(updatedNodes.map((n) => [n.id, n.data.opacity]));

    // n1 (focused) and n2 (connected via e1) should be visible
    expect(opacityById.n1).toBe(1);
    expect(opacityById.n2).toBe(1);
    // n3 connects to n2 but not within depth 1 of n1, n4 is isolated from n1
    expect(opacityById.n3).toBe(0);
    expect(opacityById.n4).toBe(0);
  });

  it("sets edge styles correctly when a node is focused", () => {
    const setNodes = vi.fn();
    const setEdges = vi.fn();
    const { result } = renderHook(() => useGraphFocus({ data: mockData, setNodes, setEdges }));

    act(() => {
      result.current.handleNodeClick({} as React.MouseEvent, { id: "n1" });
    });

    const updatedEdges = lastUpdaterResult(setEdges, toFlowEdges(mockData));
    const e1 = updatedEdges.find((e) => e.id === "e1");
    const e2 = updatedEdges.find((e) => e.id === "e2");
    expect(e1).toBeDefined();
    expect(e2).toBeDefined();
    if (!e1 || !e2) return;

    // e1 connects n1↔n2 — both in connected set, direct edge from focused node
    expect(e1.animated).toBe(true);
    expect(e1.style.opacity).toBe(0.5);
    expect(e1.style.strokeWidth).toBe(1.5);
    expect(e1.style.strokeDasharray).toBe("6 4");

    // e2 connects n3↔n2 — n3 is NOT in connected set (depth 1), so both not visible
    expect(e2.animated).toBe(false);
    expect(e2.style.opacity).toBe(0);
  });

  it("shows handles on source/target nodes of connected edges", () => {
    const setNodes = vi.fn();
    const setEdges = vi.fn();
    const { result } = renderHook(() => useGraphFocus({ data: mockData, setNodes, setEdges }));

    act(() => {
      result.current.handleNodeClick({} as React.MouseEvent, { id: "n1" });
    });

    const updatedNodes = lastUpdaterResult(setNodes, toFlowNodes(mockData));
    const nodeById = Object.fromEntries(updatedNodes.map((n) => [n.id, n.data]));

    // e1: source=n1, target=n2 — n1 gets rightHandle, n2 gets leftHandle
    expect(nodeById.n1.showRightHandle).toBe(true);
    expect(nodeById.n1.showLeftHandle).toBe(false);
    expect(nodeById.n2.showLeftHandle).toBe(true);
  });

  it("resets all nodes to opacity 1 and hides handles when focus is cleared", () => {
    const setNodes = vi.fn();
    const setEdges = vi.fn();
    const { result } = renderHook(() => useGraphFocus({ data: mockData, setNodes, setEdges }));

    // Focus then unfocus
    act(() => {
      result.current.handleNodeClick({} as React.MouseEvent, { id: "n1" });
    });
    act(() => {
      result.current.handlePaneClick();
    });

    const updatedNodes = lastUpdaterResult(setNodes, toFlowNodes(mockData));
    for (const node of updatedNodes) {
      expect(node.data.opacity).toBe(1);
      expect(node.data.showLeftHandle).toBe(false);
      expect(node.data.showRightHandle).toBe(false);
    }
  });

  it("applies focus type filter — connected nodes get 0.5 opacity, unrelated get 0", () => {
    const setNodes = vi.fn();
    const setEdges = vi.fn();
    const { result } = renderHook(() => useGraphFocus({ data: mockData, setNodes, setEdges }));

    act(() => {
      result.current.changeFocusType("agent");
    });

    const updatedNodes = lastUpdaterResult(setNodes, toFlowNodes(mockData));
    const opacityById = Object.fromEntries(updatedNodes.map((n) => [n.id, n.data.opacity]));

    // n1, n3 are agents → opacity 1 (focused type)
    expect(opacityById.n1).toBe(1);
    expect(opacityById.n3).toBe(1);
    // n2 (table) is connected to agents → opacity 0.5
    expect(opacityById.n2).toBe(0.5);
    // n4 (view) has no connection to any agent → opacity 0
    expect(opacityById.n4).toBe(0);
  });

  it("hides edges between non-visible nodes when focus type is active", () => {
    const setNodes = vi.fn();
    const setEdges = vi.fn();
    const { result } = renderHook(() => useGraphFocus({ data: mockData, setNodes, setEdges }));

    act(() => {
      result.current.changeFocusType("view");
    });

    const updatedEdges = lastUpdaterResult(setEdges, toFlowEdges(mockData));
    // n4 is the only view, no edges connect to it — both edges should be hidden
    for (const edge of updatedEdges) {
      expect(edge.style.opacity).toBe(0);
    }
  });
});
