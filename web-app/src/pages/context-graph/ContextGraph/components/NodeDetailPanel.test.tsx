// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render as rtlRender, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ContextGraphNode } from "@/types/contextGraph";

vi.mock("react-router-dom", () => ({
  useNavigate: () => vi.fn()
}));

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "test-project" }, branchName: "main" })
}));

vi.mock("@/services/api/files", () => ({
  FileService: {
    getFile: vi.fn(() => new Promise(() => {})) // never resolves by default
  }
}));

vi.mock("react-syntax-highlighter", () => ({
  Prism: ({ children }: { children: string }) => (
    <pre data-testid='syntax-highlighter'>{children}</pre>
  )
}));

vi.mock("react-syntax-highlighter/dist/esm/styles/prism", () => ({
  oneDark: {},
  oneLight: {}
}));

vi.mock("@/components/ui/panel", () => ({
  Panel: ({ children, className }: { children: React.ReactNode; className?: string }) => (
    <div className={className}>{children}</div>
  ),
  PanelHeader: ({
    title,
    subtitle,
    onClose,
    actions
  }: {
    title: string;
    subtitle?: string;
    onClose?: () => void;
    actions?: React.ReactNode;
  }) => (
    <div>
      <span>{title}</span>
      <span>{subtitle}</span>
      {actions}
      <button type='button' onClick={onClose}>
        Close
      </button>
    </div>
  ),
  PanelContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>
}));

const { NodeDetailPanel } = await import("./NodeDetailPanel");

// The panel reads the node's file through `useFile`, a React Query hook.
const render = (ui: React.ReactElement) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return rtlRender(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
};

afterEach(() => cleanup());

const makeNode = (overrides: Partial<ContextGraphNode> = {}): ContextGraphNode => ({
  id: "test-node",
  type: "agent",
  label: "Test Agent",
  data: { name: "test-agent", ...overrides.data },
  ...overrides
});

describe("NodeDetailPanel", () => {
  it("returns null when node is null", () => {
    const { container } = render(<NodeDetailPanel node={null} onClose={vi.fn()} />);
    expect(container.innerHTML).toBe("");
  });

  it("renders node label and type", () => {
    render(<NodeDetailPanel node={makeNode()} onClose={vi.fn()} />);
    expect(screen.getByText("Test Agent")).toBeInTheDocument();
    expect(screen.getByText("Agent")).toBeInTheDocument();
  });

  it("shows path when node.data.path exists", () => {
    render(
      <NodeDetailPanel
        node={makeNode({ data: { name: "a", path: "agents/test.agent.yml" } })}
        onClose={vi.fn()}
      />
    );
    expect(screen.getByText("agents/test.agent.yml")).toBeInTheDocument();
  });

  it("shows description when node.data.description exists", () => {
    render(
      <NodeDetailPanel
        node={makeNode({ data: { name: "a", description: "A test agent" } })}
        onClose={vi.fn()}
      />
    );
    expect(screen.getByText("A test agent")).toBeInTheDocument();
  });

  it("shows 'Open in IDE' button for file node types", () => {
    render(
      <NodeDetailPanel
        node={makeNode({ type: "agent", data: { name: "a", path: "agents/test.yml" } })}
        onClose={vi.fn()}
      />
    );
    expect(screen.getByTitle("Open in IDE")).toBeInTheDocument();
  });

  it("does not show 'Open in IDE' button for non-file node types", () => {
    render(<NodeDetailPanel node={makeNode({ type: "table" })} onClose={vi.fn()} />);
    expect(screen.queryByTitle("Open in IDE")).not.toBeInTheDocument();
  });

  it("shows file contents section for file node types", () => {
    render(
      <NodeDetailPanel
        node={makeNode({ type: "agent", data: { name: "a", path: "agents/test.yml" } })}
        onClose={vi.fn()}
      />
    );
    expect(screen.getByText("File Contents")).toBeInTheDocument();
  });
});
