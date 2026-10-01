// @vitest-environment jsdom

/**
 * `x-oxy-preview-revision` is what makes a request a PREVIEW request.
 *
 * `?branch=` cannot: the IDE sends the very same `?branch=feat/x` for its
 * working copy, which must stay editable, while a preview of `feat/x` is
 * read-only. So the backend keys on this header, and the two cases below are
 * the whole contract — pinned sends both (the header naming the pinned
 * revision, `?branch=` its label), the IDE sends only `?branch=`.
 *
 * End to end on the client: the real provider, the real
 * `useCurrentWorkspaceBranch`, a real service (`AgentService`) through the real
 * `apiClient` and its interceptors — only the network adapter is swapped for
 * one that records what would have gone on the wire.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen } from "@testing-library/react";
import type { AxiosAdapter, InternalAxiosRequestConfig } from "axios";
import { createElement } from "react";
import { MemoryRouter, Route, Routes, useNavigate, useParams } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspacePreview } from "@/types/workspace";

const mocks = vi.hoisted(() => ({
  staff: true,
  insideIDE: false,
  ideBranch: undefined as string | undefined,
  sseHeaders: [] as Record<string, string>[],
  list: vi.fn<(workspaceId: string) => Promise<WorkspacePreview[]>>()
}));

vi.mock("@/hooks/useCanUsePreviews", () => ({ default: () => mocks.staff }));
vi.mock("@/services/api/previews", () => ({ PreviewService: { list: mocks.list } }));
vi.mock("@/pages/ide", () => ({ useIDE: () => ({ insideIDE: mocks.insideIDE }) }));
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
  default: () => ({ getCurrentBranch: () => mocks.ideBranch })
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));
vi.mock("@microsoft/fetch-event-source", () => ({
  fetchEventSource: async (_url: string, opts: { headers: Record<string, string> }) => {
    mocks.sseHeaders.push(opts.headers);
  }
}));

import useCurrentWorkspaceBranch from "@/hooks/useCurrentWorkspaceBranch";
import { getActivePreviewRevision } from "@/libs/utils/preview";
import { AgentService } from "@/services/api/agents";
import { apiClient } from "@/services/api/axios";
import fetchSSE from "@/services/api/fetchSSE";
import { PreviewPinProvider, usePreviewPin } from "./PreviewPinContext";

const HEADER = "x-oxy-preview-revision";

interface Sent {
  branchParam: string | undefined;
  previewHeader: string | undefined;
}

let sent: Sent[] = [];
const recordingAdapter: AxiosAdapter = (config: InternalAxiosRequestConfig) => {
  sent.push({
    branchParam: (config.params as Record<string, string> | undefined)?.branch,
    previewHeader: config.headers.get(HEADER)?.toString()
  });
  return Promise.resolve({ data: [], status: 200, statusText: "OK", headers: {}, config });
};

let listAgents: () => Promise<unknown> = async () => undefined;
let exitPreview: () => void = () => {};
let navigateTo: (to: string) => void = () => {};

/** A surface doing what ~160 real ones do: pass `branchName` to its service. */
function Surface() {
  const { branchName } = useCurrentWorkspaceBranch();
  const { exit } = usePreviewPin();
  const navigate = useNavigate();
  listAgents = () => AgentService.listAgents("ws-1", branchName);
  exitPreview = exit;
  navigateTo = navigate;
  return createElement("span", { "data-testid": "surface" });
}

function Layout() {
  const { wsId } = useParams<{ wsId: string }>();
  return (
    <PreviewPinProvider workspaceId={wsId ?? ""}>
      <Surface />
    </PreviewPinProvider>
  );
}

let client: QueryClient;

async function renderAt(path: string) {
  const utils = render(
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
  await screen.findByTestId("surface");
  return utils;
}

const lastSent = async () => {
  await act(() => listAgents());
  return sent[sent.length - 1];
};

const REV = "01928c4e-7f2a-7c1d-9a55-3be0f1d2a911";

describe("x-oxy-preview-revision", () => {
  const originalAdapter = apiClient.defaults.adapter;
  beforeEach(() => {
    mocks.staff = true;
    mocks.insideIDE = false;
    mocks.ideBranch = undefined;
    mocks.sseHeaders = [];
    mocks.list.mockReset();
    mocks.list.mockResolvedValue([
      {
        branch: "feat/x",
        status: "ready",
        revision_id: REV,
        sha: "abc1234",
        error: null,
        created_by: null,
        updated_at: "2026-09-28T10:00:00Z",
        compiled_at: "2026-09-28T10:00:00Z",
        checks: null
      }
    ]);
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    sent = [];
    apiClient.defaults.adapter = recordingAdapter;
  });
  afterEach(() => {
    cleanup();
    apiClient.defaults.adapter = originalAdapter;
  });

  it("names the pinned revision on every request, with ?branch= carrying its label", async () => {
    await renderAt(`/acme/workspaces/ws-1/home?preview=${REV}`);
    expect(await lastSent()).toEqual({ branchParam: "feat/x", previewHeader: REV });

    await act(() => fetchSSE("/api/ws-1/threads/t1/ask", { onMessage: vi.fn() }));
    expect(mocks.sseHeaders.at(-1)).toMatchObject({ [HEADER]: REV });
  });

  it("is NOT sent by the IDE on the same branch — that is the working copy, not a preview", async () => {
    mocks.insideIDE = true;
    mocks.ideBranch = "feat/x";
    await renderAt("/acme/workspaces/ws-1/ide/files");

    expect(await lastSent()).toEqual({ branchParam: "feat/x", previewHeader: undefined });

    await act(() => fetchSSE("/api/ws-1/threads/t1/ask", { onMessage: vi.fn() }));
    expect(mocks.sseHeaders.at(-1)).not.toHaveProperty(HEADER);
  });

  it("is sent from inside the IDE too, once the page is pinned", async () => {
    mocks.insideIDE = true;
    mocks.ideBranch = "feat/other";
    await renderAt(`/acme/workspaces/ws-1/ide/files?preview=${REV}`);
    expect(await lastSent()).toEqual({ branchParam: "feat/x", previewHeader: REV });
  });

  it("stops the moment the preview is exited, and on leaving the workspace", async () => {
    const { unmount } = await renderAt(`/acme/workspaces/ws-1/home?preview=${REV}`);
    expect((await lastSent()).previewHeader).toBe(REV);

    act(() => exitPreview());
    expect(await lastSent()).toEqual({ branchParam: undefined, previewHeader: undefined });

    act(() => navigateTo(`/acme/workspaces/ws-1/home?preview=${REV}`));
    expect(getActivePreviewRevision()).toBe(REV);
    unmount();
    expect(getActivePreviewRevision()).toBeNull();
  });

  it("is never sent for someone who is not Oxy staff, whatever the link says", async () => {
    mocks.staff = false;
    await renderAt(`/acme/workspaces/ws-1/home?preview=${REV}`);
    expect(await lastSent()).toEqual({ branchParam: undefined, previewHeader: undefined });
    expect(getActivePreviewRevision()).toBeNull();
  });
});
