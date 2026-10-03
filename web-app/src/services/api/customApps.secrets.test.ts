// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

const post = vi.fn();
vi.mock("./axios", () => ({ apiClient: { post: (...args: unknown[]) => post(...args) } }));

import { CustomAppsService } from "./customApps";

/**
 * Setting an app secret is an upsert, and the server says which it was: `201`
 * when it stored a key the app did not have, `204` when it replaced the value
 * of one it did. Both routes reach the same handler.
 */
describe("CustomAppsService — what an app-secret write did", () => {
  beforeEach(() => post.mockReset());

  it("reads 201 as created and 204 as updated on the workspace route", async () => {
    post.mockResolvedValueOnce({ status: 201, data: "" });
    await expect(
      CustomAppsService.setWorkspaceAppSecret("ws-1", "app-1", "API_KEY", "v")
    ).resolves.toBe("created");

    post.mockResolvedValueOnce({ status: 204, data: "" });
    await expect(
      CustomAppsService.setWorkspaceAppSecret("ws-1", "app-1", "API_KEY", "v")
    ).resolves.toBe("updated");

    expect(post).toHaveBeenCalledWith("/ws-1/custom-apps/app-1/secrets", {
      key: "API_KEY",
      value: "v"
    });
  });

  it("reads 201 as created and 204 as updated on the staff route", async () => {
    post.mockResolvedValueOnce({ status: 201, data: "" });
    await expect(CustomAppsService.setSecret("app-1", "API_KEY", "v")).resolves.toBe("created");

    post.mockResolvedValueOnce({ status: 204, data: "" });
    await expect(CustomAppsService.setSecret("app-1", "API_KEY", "v")).resolves.toBe("updated");

    expect(post).toHaveBeenCalledWith("/customer-apps/app-1/secrets", {
      key: "API_KEY",
      value: "v"
    });
  });
});
