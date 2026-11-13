// @vitest-environment jsdom

import type { AxiosAdapter } from "axios";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { apiClient } from "./axios";
import { WorkspaceService } from "./workspaces";

/**
 * The shell's workspace is `/meta` alone: there is no working copy, so no git
 * half to ask for.
 */

let requested: string[] = [];
const original = apiClient.defaults.adapter;

beforeEach(() => {
  requested = [];
  const adapter: AxiosAdapter = async (config) => {
    requested.push(config.url ?? "");
    const data = { id: "ws-1", name: "Acme", factory_access: true, default_branch: "trunk" };
    return { data, status: 200, statusText: "OK", headers: {}, config };
  };
  apiClient.defaults.adapter = adapter;
});
afterEach(() => {
  apiClient.defaults.adapter = original;
});

describe("WorkspaceService.getWorkspace", () => {
  it("asks only for /meta and keeps its default branch", async () => {
    const ws = await WorkspaceService.getWorkspace("ws-1");
    expect(requested).toEqual(["/ws-1/meta"]);
    expect(ws.active_branch).toBeNull();
    expect(ws.git_mode).toBe("none");
    expect(ws.default_branch).toBe("trunk");
  });
});
