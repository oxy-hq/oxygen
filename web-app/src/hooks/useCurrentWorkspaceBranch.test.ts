// @vitest-environment jsdom

/**
 * `branchName` is "" unless a preview is pinned: there is no branch to select,
 * and every surface reads the revision the workspace serves.
 *
 * Empty rather than absent because that is the server's existing contract:
 * `normalize_branch_hint` filters `""` to `None`.
 */

import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  workspace: {
    id: "ws-1",
    active_branch: { name: "feat/x" },
    default_branch: "main"
  } as Record<string, unknown> | undefined,
  previewBranch: null as string | null
}));

vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({ workspace: mocks.workspace })
}));
vi.mock("@/contexts/PreviewPinContext", () => ({
  usePreviewPin: () => ({ branch: mocks.previewBranch })
}));

import useCurrentWorkspaceBranch from "./useCurrentWorkspaceBranch";

describe("useCurrentWorkspaceBranch", () => {
  beforeEach(() => {
    mocks.previewBranch = null;
  });

  it("sends no branch, whatever branch the workspace row names", () => {
    const { result } = renderHook(() => useCurrentWorkspaceBranch());
    expect(result.current.branchName).toBe("");
    expect(result.current.previewBranch).toBeNull();
  });

  it("sends the pinned preview's branch label", () => {
    mocks.previewBranch = "feat/y";
    const { result } = renderHook(() => useCurrentWorkspaceBranch());
    expect(result.current.branchName).toBe("feat/y");
  });
});
