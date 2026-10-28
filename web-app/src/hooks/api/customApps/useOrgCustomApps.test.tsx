// @vitest-environment jsdom

import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CustomAppSummary } from "@/types/apps";
import useOrgCustomApps from "./useOrgCustomApps";

let workspaces: { id: string; status: string }[] | undefined;
let isLoading = false;
let lists: Map<string, CustomAppSummary[]>;
let pendingIds = new Set<string>();
const askedFor = vi.fn();

vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => ({ data: workspaces, isLoading })
}));
vi.mock("./useWorkspacesCustomApps", () => ({
  useWorkspacesCustomApps: (ids: readonly string[]) => {
    askedFor(ids);
    return {
      appsByWorkspace: new Map(ids.map((id) => [id, lists.get(id) ?? []])),
      isPending: ids.some((id) => pendingIds.has(id))
    };
  }
}));

const app = (id: string): CustomAppSummary => ({
  id,
  slug: id,
  name: id,
  org_slug: "acme",
  url: `/customer-apps/acme/${id}/`,
  published_at: "2026-07-30T00:00:00Z",
  visibility: "org"
});

beforeEach(() => {
  workspaces = [
    { id: "ws-1", status: "ready" },
    { id: "ws-2", status: "ready" },
    { id: "ws-3", status: "cloning" }
  ];
  isLoading = false;
  lists = new Map();
  pendingIds = new Set();
  askedFor.mockClear();
});

describe("useOrgCustomApps", () => {
  it("lists the current workspace's apps first, then the other ready workspaces' in order", () => {
    workspaces = [
      { id: "ws-1", status: "ready" },
      { id: "ws-2", status: "ready" },
      { id: "ws-4", status: "ready" }
    ];
    lists = new Map([
      ["ws-1", [app("a")]],
      ["ws-2", [app("b"), app("c")]],
      ["ws-4", [app("d")]]
    ]);
    const { result } = renderHook(() => useOrgCustomApps("ws-2"));
    expect(result.current.apps.map((a) => a.id)).toEqual(["b", "c", "a", "d"]);
    // The current workspace is asked on its own, then the rest.
    expect(askedFor).toHaveBeenCalledWith(["ws-2"]);
    expect(askedFor).toHaveBeenLastCalledWith(["ws-1", "ws-4"]);
  });

  it("does not ask a workspace that is still cloning", () => {
    renderHook(() => useOrgCustomApps("ws-1"));
    expect(askedFor).toHaveBeenLastCalledWith(["ws-2"]);
  });

  it("lists an app once", () => {
    lists = new Map([
      ["ws-1", [app("a")]],
      ["ws-2", [app("a"), app("b")]]
    ]);
    const { result } = renderHook(() => useOrgCustomApps("ws-1"));
    expect(result.current.apps.map((a) => a.id)).toEqual(["a", "b"]);
  });

  it("still asks the current workspace when there is no workspace list (local mode)", () => {
    workspaces = undefined;
    lists = new Map([["ws-local", [app("a")]]]);
    const { result } = renderHook(() => useOrgCustomApps("ws-local"));
    expect(result.current.apps.map((a) => a.id)).toEqual(["a"]);
    expect(result.current.isPending).toBe(false);
  });

  it("is pending while the workspace list loads", () => {
    isLoading = true;
    expect(renderHook(() => useOrgCustomApps("ws-1")).result.current.isPending).toBe(true);
  });

  it("is pending until the current workspace's own list has settled", () => {
    lists = new Map([["ws-2", [app("b")]]]);
    pendingIds = new Set(["ws-1"]);
    expect(renderHook(() => useOrgCustomApps("ws-1")).result.current.isPending).toBe(true);
  });

  it("shows the current workspace's apps without waiting for a slow one", () => {
    lists = new Map([["ws-1", [app("a")]]]);
    pendingIds = new Set(["ws-2"]);
    const { result } = renderHook(() => useOrgCustomApps("ws-1"));
    expect(result.current.isPending).toBe(false);
    expect(result.current.apps.map((a) => a.id)).toEqual(["a"]);
  });

  it("waits for the others while the current workspace has no apps, so no empty state flashes", () => {
    pendingIds = new Set(["ws-2"]);
    expect(renderHook(() => useOrgCustomApps("ws-1")).result.current.isPending).toBe(true);
    lists = new Map([["ws-2", [app("b")]]]);
    pendingIds = new Set();
    expect(renderHook(() => useOrgCustomApps("ws-1")).result.current.isPending).toBe(false);
  });
});
