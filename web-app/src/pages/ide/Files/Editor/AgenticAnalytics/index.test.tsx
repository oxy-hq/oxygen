// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FilesSubViewMode } from "../../FilesSidebar/constants";

// --- Module mocks ---

vi.mock("../../FilesContext", () => ({
  useFilesContext: vi.fn()
}));

vi.mock("../contexts/useEditorContext", () => ({
  useEditorContext: vi.fn()
}));

vi.mock("../usePreviewRefresh", () => ({
  usePreviewRefresh: vi.fn()
}));

vi.mock("@/components/FileEditor/useFileEditorContext", () => ({
  useFileEditorContext: vi.fn()
}));

// Stub heavy components
vi.mock("../components/EditorPageWrapper", () => ({
  default: ({
    headerPrefixAction,
    customEditor
  }: {
    headerPrefixAction?: React.ReactNode;
    customEditor?: React.ReactNode;
  }) => (
    <div data-testid='editor-page-wrapper'>
      {headerPrefixAction && <div data-testid='header-prefix'>{headerPrefixAction}</div>}
      {customEditor ? (
        <div data-testid='custom-editor'>{customEditor}</div>
      ) : (
        <div data-testid='yaml-editor' />
      )}
    </div>
  )
}));

vi.mock("./PreviewSection", () => ({
  default: () => <div data-testid='preview-section' />
}));

vi.mock("@/components/agentic/AgenticAnalyticsForm", () => ({
  AgenticAnalyticsForm: () => <div data-testid='agentic-form' />,
  yamlToForm: (d: unknown) => d
}));

// --- Imports after mocks ---

import { useFileEditorContext } from "@/components/FileEditor/useFileEditorContext";
import { useFilesContext } from "../../FilesContext";
import { useEditorContext } from "../contexts/useEditorContext";
import { usePreviewRefresh } from "../usePreviewRefresh";
import AgenticAnalyticsEditor from "./index";

const setupMocks = (subViewMode: FilesSubViewMode = FilesSubViewMode.OBJECTS) => {
  vi.mocked(useEditorContext).mockReturnValue({
    pathb64: "dGVzdA==",
    gitEnabled: false,
    filePath: "test.agentic.yml",
    fileType: "ANALYTICS_AGENT" as never,
    project: {} as never,
    branchName: "main",
    isMainEditMode: true
  });
  vi.mocked(usePreviewRefresh).mockReturnValue({
    previewKey: "key",
    refreshPreview: vi.fn()
  });
  vi.mocked(useFilesContext).mockReturnValue({
    filesSubViewMode: subViewMode
  } as never);
  vi.mocked(useFileEditorContext).mockReturnValue({
    state: { content: "llm:\n  ref: claude\n" },
    actions: { setContent: vi.fn() }
  } as never);
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("AgenticAnalyticsEditor — default view mode", () => {
  it("renders form editor by default when in OBJECTS sidebar mode", () => {
    setupMocks(FilesSubViewMode.OBJECTS);
    render(<AgenticAnalyticsEditor />);
    expect(screen.getByTestId("custom-editor")).toBeInTheDocument();
    expect(screen.queryByTestId("yaml-editor")).not.toBeInTheDocument();
  });

  it("renders YAML editor by default when in FILES sidebar mode", () => {
    setupMocks(FilesSubViewMode.FILES);
    render(<AgenticAnalyticsEditor />);
    expect(screen.getByTestId("yaml-editor")).toBeInTheDocument();
    expect(screen.queryByTestId("custom-editor")).not.toBeInTheDocument();
  });
});

describe("AgenticAnalyticsEditor — view mode toggle", () => {
  it("renders the ViewModeToggle in the header", () => {
    setupMocks(FilesSubViewMode.OBJECTS);
    render(<AgenticAnalyticsEditor />);
    expect(screen.getByTestId("header-prefix")).toBeInTheDocument();
  });

  // The toggle is a Radix tab list, which selects a tab on mousedown / focus, not
  // on `click`. `userEvent.click` sends the whole pointer sequence a browser
  // does; a bare `fireEvent.click` sends only the last event and selects nothing.
  it("switches from form to YAML editor when Editor tab is clicked", async () => {
    setupMocks(FilesSubViewMode.OBJECTS);
    render(<AgenticAnalyticsEditor />);
    // Initially shows form
    expect(screen.getByTestId("custom-editor")).toBeInTheDocument();
    // Click the Editor tab (aria-label="Editor view")
    await userEvent.click(screen.getByRole("tab", { name: "Editor view" }));
    expect(screen.getByTestId("yaml-editor")).toBeInTheDocument();
    expect(screen.queryByTestId("custom-editor")).not.toBeInTheDocument();
  });

  it("switches from YAML editor to form when Form tab is clicked", async () => {
    setupMocks(FilesSubViewMode.FILES);
    render(<AgenticAnalyticsEditor />);
    // Initially shows YAML editor
    expect(screen.getByTestId("yaml-editor")).toBeInTheDocument();
    // Click the Form tab (aria-label="Form view")
    await userEvent.click(screen.getByRole("tab", { name: "Form view" }));
    expect(screen.getByTestId("custom-editor")).toBeInTheDocument();
    expect(screen.queryByTestId("yaml-editor")).not.toBeInTheDocument();
  });
});
