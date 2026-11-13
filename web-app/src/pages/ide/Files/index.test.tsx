// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeBase64 } from "@/libs/encoding";
import { RevisionFilesService } from "@/services/api/revisionFiles";
import FilesPage from ".";

// The Factory's Files page reads the served revision, never a working copy:
// the tree is the compiled listing and a file opens read-only.

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws" }, branchName: "" })
}));
vi.mock("@/services/api/axios", () => ({ apiClient: {} }));
vi.mock("@/components/ui/shadcn/resizable", () => ({
  ResizablePanelGroup: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  ResizablePanel: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  ResizableHandle: () => null
}));
vi.mock("@/components/Code/CodeView", () => ({
  default: ({ value, language }: { value: string; language?: string }) => (
    <pre data-testid='code-view' data-language={language}>
      {value}
    </pre>
  )
}));
vi.mock("../WorkspaceRepositoryPanel", () => ({
  default: () => <div data-testid='workspace-repository-panel' />
}));

const renderAt = (path: string) =>
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path='/ide/files/*' element={<FilesPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );

beforeEach(() => {
  vi.spyOn(RevisionFilesService, "list").mockResolvedValue([
    { path: "config.yml", kind: "config" },
    { path: "semantics/views/orders.view.yml", kind: "view" }
  ]);
  vi.spyOn(RevisionFilesService, "get").mockResolvedValue({
    path: "semantics/views/orders.view.yml",
    kind: "view",
    language: "yaml",
    content: "name: orders\n"
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("Factory Files page", () => {
  it("lists the compiled files and shows the repository until one is open", async () => {
    renderAt("/ide/files");
    expect(await screen.findByText("config.yml")).toBeTruthy();
    expect(screen.getByText("semantics")).toBeTruthy();
    expect(screen.getByTestId("workspace-repository-panel")).toBeTruthy();
    expect(RevisionFilesService.get).not.toHaveBeenCalled();
  });

  it("hints how to clone the workspace with oxyc", async () => {
    renderAt("/ide/files");
    const hint = await screen.findByTestId("ide-files-clone-hint");
    expect(hint.textContent).toContain("oxyc workspace clone 'ws'");
    expect(screen.getByTestId("ide-files-clone-copy")).toBeTruthy();
  });

  it("opens the file the URL names, read-only, with its folders expanded", async () => {
    renderAt(`/ide/files/${encodeBase64("semantics/views/orders.view.yml")}`);
    const viewer = await screen.findByTestId("code-view");
    expect(viewer.textContent).toBe("name: orders\n");
    expect(viewer.dataset.language).toBe("yaml");
    expect(RevisionFilesService.get).toHaveBeenCalledWith("ws", "semantics/views/orders.view.yml");
    expect(screen.getByText("orders.view.yml")).toBeTruthy();
  });

  it("says nothing is compiled on a 503, rather than drawing an empty tree", async () => {
    vi.spyOn(RevisionFilesService, "list").mockRejectedValue(
      Object.assign(new Error("unavailable"), {
        isAxiosError: true,
        response: { status: 503, data: "not compiled" }
      })
    );
    renderAt("/ide/files");
    expect(await screen.findByText(/Nothing is compiled for this workspace yet/)).toBeTruthy();
    expect(screen.queryByText("The served revision has no files.")).toBeNull();
  });

  it("folds a folder away when it is clicked", async () => {
    renderAt("/ide/files");
    fireEvent.click(await screen.findByText("semantics"));
    expect(screen.queryByText("views")).toBeNull();
  });
});
