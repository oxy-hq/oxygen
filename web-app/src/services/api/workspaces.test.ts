// @vitest-environment jsdom

import type { AxiosAdapter } from "axios";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { apiClient } from "./axios";
import { WorkspaceService } from "./workspaces";

/**
 * The shell's workspace is `/meta` plus `/git-state`. `/git-state` is the Factory's,
 * so with the apps-first experience the second half is asked only of someone `/meta`
 * says may use the Factory.
 */

let requested: string[] = [];
let factoryAccess = true;
const original = apiClient.defaults.adapter;

beforeEach(() => {
  requested = [];
  factoryAccess = true;
  const adapter: AxiosAdapter = async (config) => {
    requested.push(config.url ?? "");
    const data = config.url?.endsWith("/meta")
      ? { id: "ws-1", name: "Acme", factory_access: factoryAccess }
      : { active_branch: { name: "main" }, git_mode: "local" };
    return { data, status: 200, statusText: "OK", headers: {}, config };
  };
  apiClient.defaults.adapter = adapter;
});
afterEach(() => {
  apiClient.defaults.adapter = original;
});

describe("WorkspaceService.getWorkspace", () => {
  it("asks for both halves as before with the flag off", async () => {
    factoryAccess = false;
    const ws = await WorkspaceService.getWorkspace("ws-1");
    expect(requested.sort()).toEqual(["/ws-1/git-state", "/ws-1/meta"]);
    expect(ws.active_branch?.name).toBe("main");
  });

  it("leaves the git half out for someone who may not use the Factory", async () => {
    factoryAccess = false;
    const ws = await WorkspaceService.getWorkspace("ws-1", { gitStateOnlyForFactory: true });
    expect(requested).toEqual(["/ws-1/meta"]);
    expect(ws.active_branch).toBeNull();
    expect(ws.factory_access).toBe(false);
  });

  it("still reads the git half for a Factory user", async () => {
    const ws = await WorkspaceService.getWorkspace("ws-1", { gitStateOnlyForFactory: true });
    expect(requested).toEqual(["/ws-1/meta", "/ws-1/git-state"]);
    expect(ws.active_branch?.name).toBe("main");
  });
});
