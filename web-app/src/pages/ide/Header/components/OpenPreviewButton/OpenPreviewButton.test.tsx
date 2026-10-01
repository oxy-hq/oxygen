// @vitest-environment jsdom

/**
 * "Open preview" beside the IDE's branch picker: it must make a preview exist,
 * wait out the compile where the person can see it, and only then enter the
 * preview — never open a branch that is still compiling or that failed.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { MemoryRouter, Route, Routes, useLocation, useParams } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspacePreview } from "@/types/workspace";

const mocks = vi.hoisted(() => ({
  staff: true,
  rows: [] as WorkspacePreview[],
  get: vi.fn(),
  post: vi.fn(),
  toastError: vi.fn(),
  ideGit: { branch: "feat/x", isOnMain: false }
}));

vi.mock("@/hooks/useCanUsePreviews", () => ({ default: () => mocks.staff }));
vi.mock("@/services/api/axios", () => ({
  apiClient: { get: mocks.get, post: mocks.post, delete: vi.fn() }
}));
vi.mock("sonner", () => ({ toast: { error: mocks.toastError, success: vi.fn() } }));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: (select?: (s: unknown) => unknown) => {
    const state = { org: { slug: "acme" } };
    return select ? select(state) : state;
  }
}));
// GitActions' neighbours, for the placement test: only the button is real.
vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ isLocalMode: false }) }));
vi.mock("@/pages/ide/Header/context/IdeGitContext", () => ({
  useIdeGit: () => ({
    workspaceId: "ws-1",
    branch: mocks.ideGit.branch,
    isOnMain: mocks.ideGit.isOnMain,
    gitState: { caps: { can_switch_branch: true, can_browse_history: false } },
    refresh: vi.fn()
  })
}));
vi.mock("@/pages/ide/Header/components/BranchPopover/WorkspaceBranchSwitcher", () => ({
  WorkspaceBranchSwitcher: ({ trigger }: { trigger: ReactNode }) => trigger
}));
vi.mock("@/pages/ide/Header/components/BranchInfo", () => ({ BranchInfo: () => null }));
vi.mock("@/pages/ide/Header/components/GitActions/ActionsRow", () => ({ ActionsRow: () => null }));

import { AxiosError, AxiosHeaders, type InternalAxiosRequestConfig } from "axios";
import { PreviewPinProvider } from "@/contexts/PreviewPinContext";
import { PREVIEW_POLL_MS } from "@/hooks/api/workspaces/usePreviews";
import { GitActions } from "../GitActions";
import { OpenPreviewButton } from "./index";

const LIST = "/ws-1/previews";

/** What the backend (#3378) answers when it refuses a create or refresh. */
function apiError(status: number, data: unknown) {
  const config = { headers: new AxiosHeaders() } as InternalAxiosRequestConfig;
  return new AxiosError(
    `Request failed with status code ${status}`,
    "ERR_BAD_REQUEST",
    config,
    null,
    {
      status,
      statusText: "",
      headers: new AxiosHeaders(),
      config,
      data
    }
  );
}
// Pinned to the READY row's revision — never to the branch.
const HOME_PINNED = "/acme/workspaces/ws-1/home?preview=rev-7";

const row = (over: Partial<WorkspacePreview>): WorkspacePreview => ({
  branch: "feat/x",
  status: "ready",
  revision_id: "rev-7",
  sha: "abc1234",
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

function Layout({ children }: { children: ReactNode }) {
  const { wsId } = useParams<{ wsId: string }>();
  return (
    <PreviewPinProvider workspaceId={wsId ?? ""}>
      {children}
      <Url />
    </PreviewPinProvider>
  );
}

function renderAt(path: string, ui: ReactNode) {
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
          createElement(Route, {
            path: "/acme/workspaces/:wsId/*",
            element: createElement(Layout, null, ui)
          })
        )
      )
    )
  );
}

const renderButton = (path = "/acme/workspaces/ws-1/ide/files") =>
  renderAt(path, createElement(OpenPreviewButton, { workspaceId: "ws-1", branch: "feat/x" }));

const button = () => screen.getByTestId("ide-open-preview");
const url = () => screen.getByTestId("url").textContent;
const posts = () => mocks.post.mock.calls.map(([path, body, config]) => ({ path, body, config }));

describe("IDE → Open preview", () => {
  beforeEach(() => {
    mocks.staff = true;
    mocks.rows = [];
    mocks.ideGit = { branch: "feat/x", isOnMain: false };
    mocks.toastError.mockReset();
    mocks.get.mockReset();
    mocks.post.mockReset();
    mocks.get.mockImplementation(() => Promise.resolve({ data: { items: mocks.rows } }));
    // The server's side of create/refresh: the row goes to compiling.
    mocks.post.mockImplementation(() => {
      const item = row({ status: "compiling", compiled_at: null, revision_id: null, sha: null });
      mocks.rows = [item];
      return Promise.resolve({ data: { item } });
    });
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("creates a missing preview, shows it compiling, and opens it once ready", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    renderButton();
    // Nothing is read until the button is pressed.
    expect(mocks.get).not.toHaveBeenCalled();

    fireEvent.click(button());

    await waitFor(() => expect(button()).toHaveAttribute("data-phase", "waiting"));
    expect(button()).toHaveTextContent("Compiling preview…");
    expect(button()).toBeDisabled();
    expect(posts()).toEqual([{ path: LIST, body: { branch: "feat/x" }, config: undefined }]);
    expect(url()).toBe("/acme/workspaces/ws-1/ide/files");

    // Still compiling on the next poll: keep waiting, don't open.
    await act(() => vi.advanceTimersByTimeAsync(PREVIEW_POLL_MS));
    expect(url()).toBe("/acme/workspaces/ws-1/ide/files");

    mocks.rows = [row({ status: "ready" })];
    await act(() => vi.advanceTimersByTimeAsync(PREVIEW_POLL_MS));

    await waitFor(() => expect(url()).toBe(HOME_PINNED));
  });

  it("opens a ready preview straight away, without recompiling it", async () => {
    mocks.rows = [row({ status: "ready" })];
    renderButton();

    fireEvent.click(button());

    await waitFor(() => expect(url()).toBe(HOME_PINNED));
    expect(mocks.post).not.toHaveBeenCalled();
  });

  it.each([
    ["failed", "rather than opening the failure"],
    ["stale", "rather than opening a revision the branch has moved past"]
  ] as const)("recompiles a %s preview %s", async (status, _why) => {
    mocks.rows = [row({ status, error: status === "failed" ? "boom" : null })];
    renderButton();

    fireEvent.click(button());

    await waitFor(() =>
      expect(posts()).toEqual([
        { path: `${LIST}/refresh`, body: undefined, config: { params: { branch: "feat/x" } } }
      ])
    );
    await waitFor(() => expect(button()).toHaveAttribute("data-phase", "waiting"));
    expect(url()).toBe("/acme/workspaces/ws-1/ide/files");
  });

  it("stops waiting and says why when the compile fails", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    renderButton();
    fireEvent.click(button());
    await waitFor(() => expect(button()).toHaveAttribute("data-phase", "waiting"));

    mocks.rows = [row({ status: "failed", error: "orders.view.yml: unknown measure" })];
    await act(() => vi.advanceTimersByTimeAsync(PREVIEW_POLL_MS));

    await waitFor(() => expect(button()).toHaveAttribute("data-phase", "idle"));
    expect(mocks.toastError).toHaveBeenCalledWith(
      "The preview of feat/x failed to compile.",
      expect.objectContaining({ description: "orders.view.yml: unknown measure" })
    );
    expect(url()).toBe("/acme/workspaces/ws-1/ide/files");
  });

  it("says why it cannot compile beside the button, in the server's words — not as a toast", async () => {
    const message = "The workspace has no checkout of feat/x. Open it in the IDE first.";
    mocks.post.mockRejectedValue(apiError(409, { code: "cannot_compile", message }));
    renderButton();

    fireEvent.click(button());

    expect(await screen.findByTestId("ide-open-preview-error")).toHaveTextContent(message);
    expect(button()).toHaveAttribute("data-phase", "idle");
    expect(mocks.toastError).not.toHaveBeenCalled();
    expect(url()).toBe("/acme/workspaces/ws-1/ide/files");

    // Pressing again clears it while it tries again.
    mocks.post.mockImplementation(() => new Promise(() => {}));
    fireEvent.click(button());
    await waitFor(() => expect(screen.queryByTestId("ide-open-preview-error")).toBeNull());
  });

  it("creates the preview when the refresh finds the branch was never previewed", async () => {
    mocks.rows = [row({ status: "stale", revision_id: null })];
    mocks.post.mockImplementation((path: string) => {
      if (path === `${LIST}/refresh`) {
        return Promise.reject(apiError(404, { code: "preview_not_found" }));
      }
      const item = row({ status: "ready", revision_id: "rev-7" });
      mocks.rows = [item];
      return Promise.resolve({ data: { item } });
    });
    renderButton();

    fireEvent.click(button());

    await waitFor(() => expect(url()).toBe(HOME_PINNED));
    expect(posts().map((p) => p.path)).toEqual([`${LIST}/refresh`, LIST]);
    expect(mocks.toastError).not.toHaveBeenCalled();
  });

  it("opens straight away when a refresh of an unchanged head returns the ready revision", async () => {
    mocks.rows = [row({ status: "stale" })];
    // Head unchanged since the last compile: the server answers with the
    // existing ready revision instead of queueing a compile. It answers after
    // a real round trip — an instant mock lets React fold every intermediate
    // phase into one paint, which would hide a "Compiling…" shown in flight.
    mocks.post.mockImplementation(
      () =>
        new Promise((resolve) =>
          setTimeout(
            () => resolve({ data: { item: row({ status: "ready", revision_id: "rev-7" }) } }),
            30
          )
        )
    );
    renderButton();
    // Every phase the button ever showed. Recorded per DOM change rather than
    // sampled, so a phase that came and went is still caught — and from added
    // nodes as well as attribute changes, because toggling its tooltip makes
    // the button a NEW element whenever it goes busy or idle.
    const shown: string[] = [];
    const SELECTOR = '[data-testid="ide-open-preview"]';
    const note = (el: Element) => {
      const phase = el.getAttribute("data-phase");
      if (phase) shown.push(phase);
    };
    const record = (records: MutationRecord[]) => {
      for (const r of records) {
        if (r.type === "attributes") {
          // Both ends: the phase it left, and the one it moved to (a phase can
          // end by the button unmounting, which records no further change).
          if (r.oldValue) shown.push(r.oldValue);
          note(r.target as Element);
        }
        for (const n of Array.from(r.addedNodes)) {
          if (!(n instanceof Element)) continue;
          if (n.matches(SELECTOR)) note(n);
          n.querySelectorAll(SELECTOR).forEach(note);
        }
      }
    };
    const observer = new MutationObserver(record);
    observer.observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["data-phase"],
      attributeOldValue: true
    });

    fireEvent.click(button());

    await waitFor(() => expect(url()).toBe(HOME_PINNED));
    // `disconnect` drops records still queued; take them first.
    record(observer.takeRecords());
    observer.disconnect();
    expect(shown).toContain("requesting");
    // It never told anyone it was compiling.
    expect(shown).not.toContain("waiting");
  });

  it("is hidden while this branch is already the pinned preview", async () => {
    mocks.rows = [row({ status: "ready" })];
    renderButton("/acme/workspaces/ws-1/ide/files?preview=rev-7");
    // The page renders only once the pin has resolved to its branch, so the
    // URL probe appearing proves the pinned state was reached — without it,
    // "no button" would pass on a page that simply had not rendered yet.
    expect(await screen.findByTestId("url")).toHaveTextContent("?preview=rev-7");
    expect(screen.queryByTestId("ide-open-preview")).toBeNull();
  });

  it("is never shown to someone who is not Oxy staff", () => {
    mocks.staff = false;
    renderButton();
    expect(screen.queryByTestId("ide-open-preview")).toBeNull();
  });
});

describe("IDE git actions", () => {
  beforeEach(() => {
    mocks.staff = true;
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  });
  afterEach(cleanup);

  const renderGitActions = () =>
    renderAt("/acme/workspaces/ws-1/ide/files", createElement(GitActions));

  it("offers Open preview beside the branch picker on a non-default branch", () => {
    mocks.ideGit = { branch: "feat/x", isOnMain: false };
    renderGitActions();
    expect(screen.getByTestId("ide-open-preview")).toBeInTheDocument();
  });

  it("offers nothing to preview on the default branch — it is what's live", () => {
    mocks.ideGit = { branch: "main", isOnMain: true };
    renderGitActions();
    expect(screen.queryByTestId("ide-open-preview")).toBeNull();
  });
});
