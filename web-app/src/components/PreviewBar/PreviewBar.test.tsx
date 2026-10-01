// @vitest-environment jsdom

/**
 * The preview bar is the only thing on a pinned page saying "this is a
 * compiled revision of a branch, not what your team sees". So: it must be
 * there whenever the page is pinned, never on a live page, name the branch and
 * the commit, confirm against the pinned REVISION (not merely the branch), and
 * **Exit preview** must actually end the mode.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createElement } from "react";
import { MemoryRouter, Route, Routes, useLocation, useParams } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspacePreview } from "@/types/workspace";

const mocks = vi.hoisted(() => ({
  staff: true,
  list: vi.fn<(workspaceId: string) => Promise<WorkspacePreview[]>>()
}));

vi.mock("@/hooks/useCanUsePreviews", () => ({ default: () => mocks.staff }));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({ workspace: { id: "ws-1", default_branch: "main" } })
}));
vi.mock("@/services/api/previews", () => ({ PreviewService: { list: mocks.list } }));

import { PreviewPinProvider } from "@/contexts/PreviewPinContext";
import queryKeys from "@/hooks/api/queryKey";
import { reportPreviewServed, resetPreviewServed } from "@/libs/utils/preview";
import { PreviewBar } from "./index";

const row = (over: Partial<WorkspacePreview> = {}): WorkspacePreview => ({
  branch: "feat/x",
  status: "ready",
  revision_id: "rev-1",
  sha: "abc1234def5678",
  error: null,
  created_by: { id: "u1", name: "Ada" },
  updated_at: "2026-09-28T10:00:00Z",
  compiled_at: "2026-09-28T10:00:00Z",
  checks: null,
  ...over
});

let client: QueryClient;

function Url() {
  const location = useLocation();
  return createElement("span", { "data-testid": "url" }, `${location.pathname}${location.search}`);
}

function Layout() {
  const { wsId } = useParams<{ wsId: string }>();
  return (
    <div>
      <PreviewPinProvider workspaceId={wsId ?? ""}>
        <PreviewBar />
      </PreviewPinProvider>
      <Url />
    </div>
  );
}

function renderAt(path: string) {
  return render(
    createElement(
      QueryClientProvider,
      { client },
      createElement(
        MemoryRouter,
        { initialEntries: [path] },
        createElement(
          Routes,
          null,
          createElement(Route, { path: "/acme/workspaces/:wsId/*", element: createElement(Layout) })
        )
      )
    )
  );
}

const PINNED = "/acme/workspaces/ws-1/home?preview=rev-1";
const bar = () => screen.findByTestId("preview-bar");

describe("PreviewBar", () => {
  beforeEach(() => {
    mocks.staff = true;
    mocks.list.mockReset();
    mocks.list.mockResolvedValue([row()]);
    resetPreviewServed();
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  });
  afterEach(cleanup);

  it("is absent on a live page, and asks the server nothing", () => {
    renderAt("/acme/workspaces/ws-1/home");
    expect(screen.queryByTestId("preview-bar")).toBeNull();
    expect(mocks.list).not.toHaveBeenCalled();
  });

  it("names the mode, the branch and the compiled commit when the page is pinned", async () => {
    renderAt(PINNED);
    expect(await bar()).toHaveTextContent("Preview · real data, read-only");
    expect(screen.getByTestId("preview-bar-branch")).toHaveTextContent("feat/x");
    expect(screen.getByTestId("preview-bar-sha")).toHaveTextContent("@ abc1234");
    expect(screen.getByTestId("preview-bar-sha")).not.toHaveTextContent("abc1234d");
    expect(screen.getByTestId("preview-bar-exit")).toHaveTextContent("Exit preview");
  });

  it("Exit preview drops the pin and returns the same page to live", async () => {
    renderAt("/acme/workspaces/ws-1/threads?preview=rev-1");

    fireEvent.click(await screen.findByTestId("preview-bar-exit"));

    await waitFor(() => expect(screen.queryByTestId("preview-bar")).toBeNull());
    expect(screen.getByTestId("url")).toHaveTextContent(/^\/acme\/workspaces\/ws-1\/threads$/);
  });

  it("confirms only on the server's word for the pinned revision", async () => {
    renderAt(PINNED);
    const el = await bar();
    expect(el).toHaveAttribute("data-confirmed", "false");
    expect(screen.getByTestId("preview-bar-confirmed")).toHaveTextContent(
      "Not yet confirmed by the server"
    );

    // Same branch, different revision: not a confirmation of THIS preview.
    act(() => reportPreviewServed("feat/x@rev-0"));
    expect(el).toHaveAttribute("data-confirmed", "false");

    act(() => reportPreviewServed("feat/x@rev-1"));
    expect(el).toHaveAttribute("data-confirmed", "true");
    expect(screen.getByTestId("preview-bar-confirmed")).toHaveTextContent(
      "Served from this revision"
    );
  });

  it("confirms a revision the server serves as main's own compile", async () => {
    // When the branch head equals a commit main already compiled, the pinned
    // revision is main's: the server may name `main` in the header. The
    // revision is what was pinned, so that is still this preview.
    renderAt(PINNED);
    const el = await bar();

    act(() => reportPreviewServed("main@rev-1"));

    expect(el).toHaveAttribute("data-confirmed", "true");
    expect(screen.getByTestId("preview-bar-branch")).toHaveTextContent("feat/x");
  });

  it("offers the newer compile when the branch's preview has moved on", async () => {
    mocks.list.mockResolvedValue([row(), row({ branch: "feat/y", revision_id: "rev-9" })]);
    renderAt(PINNED);
    await bar();

    mocks.list.mockResolvedValue([row({ revision_id: "rev-2", sha: "fff0000aaa" })]);
    await act(() => client.invalidateQueries());

    const state = await screen.findByTestId("preview-bar-state");
    expect(state).toHaveTextContent("A newer compile is ready — open it");
    // Still honest about what is on screen until they choose to move.
    expect(screen.getByTestId("preview-bar-sha")).toHaveTextContent("@ abc1234");

    fireEvent.click(state);
    await waitFor(() =>
      expect(screen.getByTestId("url")).toHaveTextContent(
        "/acme/workspaces/ws-1/home?preview=rev-2"
      )
    );
    expect(screen.getByTestId("preview-bar-sha")).toHaveTextContent("@ fff0000");
  });

  it("says when the branch has moved past this revision", async () => {
    mocks.list.mockResolvedValue([row({ status: "stale" })]);
    renderAt(PINNED);
    expect(await screen.findByTestId("preview-bar-state")).toHaveTextContent(
      "The branch has moved on — refresh in Previews"
    );
  });

  it("links to what changed, at the compiled commit, only when the app already knows the repo", async () => {
    renderAt(PINNED);
    await bar();
    expect(screen.queryByTestId("preview-bar-changes")).toBeNull();

    act(() => {
      client.setQueryData(queryKeys.workspaces.revisionInfo("ws-1", "feat/x"), {
        ahead_count: 0,
        behind_count: 0,
        uncommitted_count: 0,
        is_in_conflict: false,
        remote_url: "git@github.com:acme/analytics.git"
      });
    });

    expect(await screen.findByTestId("preview-bar-changes")).toHaveAttribute(
      "href",
      "https://github.com/acme/analytics/compare/main...abc1234def5678"
    );
  });

  it("never shows for someone who is not Oxy staff, even on a ?preview= link", () => {
    mocks.staff = false;
    renderAt(PINNED);
    expect(screen.queryByTestId("preview-bar")).toBeNull();
    expect(mocks.list).not.toHaveBeenCalled();
  });
});
