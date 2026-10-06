// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CustomApp } from "@/types/apps";
import { DossierBody } from "./index";

// Every section fetches for itself; none of that is what this file is about.
// Issues and Functions are stood in by the one thing each contributes here: a
// way to ask for a function, and a way to see which one is selected.
vi.mock("../Activity", () => ({ Activity: () => null }));
vi.mock("../AppAccessPane", () => ({ AppAccessPane: () => null }));
vi.mock("../AppInfo", () => ({ AppInfo: () => null }));
vi.mock("../AppLogs", () => ({ AppLogs: () => null }));
vi.mock("../AppSettings", () => ({ AppSettings: () => null }));
vi.mock("../Availability", () => ({ Availability: () => null }));
vi.mock("../BuildHistory", () => ({ BuildHistory: () => null }));
vi.mock("../Secrets", () => ({ Secrets: () => null, SecretsBadge: () => null }));
vi.mock("../Functions", () => ({
  Functions: ({ selected }: { selected: string | null }) => (
    <div data-testid='functions-stub'>{selected ?? "none"}</div>
  )
}));
vi.mock("../Issues", () => ({
  IssuesBadge: () => null,
  Issues: ({ onOpenFunction }: { onOpenFunction: (name: string) => void }) => (
    <button type='button' onClick={() => onOpenFunction("upload-report")}>
      open the failing function
    </button>
  )
}));

const APP = { id: "app-id", org_slug: "acme", slug: "bookkeeping" } as CustomApp;

const sectionState = (id: string) =>
  screen.getByTestId(`admin-app-dossier-section-${id}`).getAttribute("data-state");

beforeEach(() => {
  localStorage.clear();
  // jsdom lays nothing out, so it has no scrollIntoView to call.
  Element.prototype.scrollIntoView = vi.fn();
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("DossierBody", () => {
  it("opens the Functions section when an issue asks for its function", () => {
    // Functions is collapsed by default. Selecting a function inside a closed
    // section changes the URL and nothing on screen — a button that looks dead.
    const onFnChange = vi.fn();
    render(<DossierBody app={APP} focusSection='issues' fn={null} onFnChange={onFnChange} />);
    expect(sectionState("functions")).toBe("closed");

    fireEvent.click(screen.getByText("open the failing function"));

    expect(onFnChange).toHaveBeenCalledWith("upload-report");
    expect(sectionState("functions")).toBe("open");
  });

  it("has a linkable Issues section, closed until asked for", () => {
    const { unmount } = render(<DossierBody app={APP} fn={null} />);
    expect(sectionState("issues")).toBe("closed");
    unmount();

    render(<DossierBody app={APP} focusSection='issues' fn={null} />);
    expect(sectionState("issues")).toBe("open");
  });
});
