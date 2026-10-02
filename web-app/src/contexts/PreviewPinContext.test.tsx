// @vitest-environment jsdom

/**
 * The preview pin is a request-routing decision: while a page is pinned to a
 * revision, every surface must ask for that preview's branch label
 * (`useCurrentWorkspaceBranch().branchName` → `?branch=`), and not one render
 * may ask for live data in between — a flash of live numbers under a "Preview"
 * bar is the failure mode that makes the bar a lie. A shared link carries only
 * the revision, so until its label is known the page must not render at all.
 *
 * These drive the REAL `useCurrentWorkspaceBranch` through the real provider in
 * a real router, and record every `branchName` it produced.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { createElement } from "react";
import { MemoryRouter, Route, Routes, useLocation, useNavigate, useParams } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspacePreview } from "@/types/workspace";

const mocks = vi.hoisted(() => ({
  staff: true as boolean | undefined,
  list: vi.fn<(workspaceId: string) => Promise<WorkspacePreview[]>>()
}));

vi.mock("@/hooks/useCanUsePreviews", () => ({ default: () => mocks.staff }));
vi.mock("@/services/api/previews", () => ({ PreviewService: { list: mocks.list } }));
vi.mock("@/pages/ide", () => ({ useIDE: () => ({ insideIDE: false }) }));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({
    workspace: {
      id: "ws-1",
      active_branch: { name: "main" },
      default_branch: "main",
      protected_branches: ["main"]
    }
  })
}));
vi.mock("@/stores/useIdeBranch", () => ({
  default: () => ({ getCurrentBranch: () => undefined })
}));

import useCurrentWorkspaceBranch from "@/hooks/useCurrentWorkspaceBranch";
import { PreviewPinProvider, usePreviewPin } from "./PreviewPinContext";

const row = (over: Partial<WorkspacePreview> = {}): WorkspacePreview => ({
  branch: "feat/x",
  status: "ready",
  revision_id: "rev-1",
  sha: "abc1234def",
  error: null,
  created_by: null,
  updated_at: "2026-09-28T10:00:00Z",
  compiled_at: "2026-09-28T10:00:00Z",
  checks: null,
  ...over
});

/** Every `branchName` the page produced, in render order. */
let seen: string[] = [];
let navigateTo: (to: string | number) => void = () => {};
let exitPreview: () => void = () => {};

function Page() {
  const { branchName } = useCurrentWorkspaceBranch();
  const { exit } = usePreviewPin();
  seen.push(branchName);
  exitPreview = exit;
  return createElement("span", { "data-testid": "branch" }, branchName || "(live)");
}

function Pending() {
  const { status } = usePreviewPin();
  return createElement("span", { "data-testid": "pending" }, status);
}

function Url() {
  const location = useLocation();
  const navigate = useNavigate();
  navigateTo = (to) => {
    void (typeof to === "number" ? navigate(to) : navigate(to));
  };
  return createElement("span", { "data-testid": "url" }, `${location.pathname}${location.search}`);
}

/** Mirrors `WorkspaceLayout`: the provider's workspace is the route's `:wsId`. */
function Layout() {
  const { wsId } = useParams<{ wsId: string }>();
  return (
    <div>
      <PreviewPinProvider workspaceId={wsId ?? ""} fallback={<Pending />}>
        <Page />
      </PreviewPinProvider>
      <Url />
    </div>
  );
}

let client: QueryClient;

function renderAt(entries: string[]) {
  return render(
    createElement(
      QueryClientProvider,
      { client },
      createElement(
        MemoryRouter,
        { initialEntries: entries, initialIndex: entries.length - 1 },
        createElement(
          Routes,
          null,
          createElement(Route, { path: "/acme/workspaces/:wsId/*", element: createElement(Layout) })
        )
      )
    )
  );
}

const branch = () => screen.getByTestId("branch").textContent;
const url = () => screen.getByTestId("url").textContent ?? "";
const PINNED = "/acme/workspaces/ws-1/home?preview=rev-1";

describe("PreviewPinProvider → useCurrentWorkspaceBranch", () => {
  beforeEach(() => {
    mocks.staff = true;
    mocks.list.mockReset();
    mocks.list.mockResolvedValue([row()]);
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    seen = [];
  });
  afterEach(cleanup);

  it("sends no branch on a live page, and reads no previews", () => {
    renderAt(["/acme/workspaces/ws-1/home"]);
    expect(branch()).toBe("(live)");
    expect(mocks.list).not.toHaveBeenCalled();
  });

  it("withholds the page until the pinned revision's branch is known, then sends that label", async () => {
    renderAt([PINNED]);
    // Nothing of the page rendered while resolving — so nothing fetched live.
    expect(screen.getByTestId("pending")).toHaveTextContent("resolving");
    expect(seen).toEqual([]);

    expect(await screen.findByTestId("branch")).toHaveTextContent("feat/x");
    expect(seen.every((b) => b === "feat/x")).toBe(true);
    expect(mocks.list).toHaveBeenCalledWith("ws-1");
  });

  it("says the preview is unavailable, and still renders nothing live, when no row has that revision", async () => {
    mocks.list.mockResolvedValue([row({ revision_id: "rev-2" })]);
    renderAt([PINNED]);

    await waitFor(() => expect(screen.getByTestId("pending")).toHaveTextContent("unavailable"));
    expect(seen).toEqual([]);
  });

  it("keeps the label once found, even after the branch's row moves to a newer revision", async () => {
    renderAt([PINNED]);
    await screen.findByTestId("branch");

    mocks.list.mockResolvedValue([row({ revision_id: "rev-2", status: "compiling" })]);
    await act(() => client.invalidateQueries());

    expect(branch()).toBe("feat/x");
    expect(url()).toBe(PINNED);
  });

  it("keeps the pin across an in-app navigation that drops the param, with no live render", async () => {
    renderAt([PINNED]);
    await screen.findByTestId("branch");
    seen = [];

    act(() => navigateTo("/acme/workspaces/ws-1/threads"));

    expect(branch()).toBe("feat/x");
    expect(seen).not.toContain("");
    // And the param is back, so a reload or a copied link keeps the mode.
    expect(url()).toBe("/acme/workspaces/ws-1/threads?preview=rev-1");
  });

  it("drops the pin on Exit preview and stays on the same page, live", async () => {
    renderAt(["/acme/workspaces/ws-1/apps?tab=all&preview=rev-1"]);
    await screen.findByTestId("branch");

    act(() => exitPreview());

    expect(branch()).toBe("(live)");
    expect(url()).toBe("/acme/workspaces/ws-1/apps?tab=all");
    // Navigating on afterwards does not resurrect it.
    act(() => navigateTo("/acme/workspaces/ws-1/home"));
    expect(branch()).toBe("(live)");
    expect(url()).toBe("/acme/workspaces/ws-1/home");
  });

  it("lets the back button return to a page that was never pinned", async () => {
    renderAt(["/acme/workspaces/ws-1/home", PINNED]);
    await screen.findByTestId("branch");
    expect(branch()).toBe("feat/x");

    act(() => navigateTo(-1));

    expect(branch()).toBe("(live)");
    expect(url()).toBe("/acme/workspaces/ws-1/home");
  });

  it("does not carry a pin into another workspace", async () => {
    renderAt([PINNED]);
    await screen.findByTestId("branch");
    seen = [];

    act(() => navigateTo("/acme/workspaces/ws-2/home"));

    expect(branch()).toBe("(live)");
    expect(seen).not.toContain("feat/x");
    expect(url()).toBe("/acme/workspaces/ws-2/home");
  });

  it("does nothing with a ?preview= link for someone who is not Oxy staff", () => {
    mocks.staff = false;
    renderAt([PINNED]);

    expect(branch()).toBe("(live)");
    expect(seen).not.toContain("feat/x");
    expect(mocks.list).not.toHaveBeenCalled();
    // Inert, not rewritten: nothing about the page changes for them.
    expect(url()).toBe(PINNED);
    act(() => navigateTo("/acme/workspaces/ws-1/threads"));
    expect(url()).toBe("/acme/workspaces/ws-1/threads");
  });

  it("waits, rather than rendering live, while it is not yet known whether the viewer is staff", () => {
    mocks.staff = undefined;
    renderAt([PINNED]);
    expect(screen.getByTestId("pending")).toHaveTextContent("resolving");
    expect(seen).toEqual([]);
  });

  it("ignores a ?preview= value that cannot be a revision id", () => {
    renderAt(["/acme/workspaces/ws-1/home?preview=caf%C3%A9%20x"]);
    expect(branch()).toBe("(live)");
    expect(mocks.list).not.toHaveBeenCalled();
  });
});
