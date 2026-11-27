// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

const get = vi.fn();
const post = vi.fn();
const put = vi.fn();
vi.mock("./axios", () => ({
  apiClient: {
    get: (...args: unknown[]) => get(...args),
    post: (...args: unknown[]) => post(...args),
    put: (...args: unknown[]) => put(...args)
  }
}));

import { AppBuildChecksService } from "./appBuildChecks";
import { CustomAppsService } from "./customApps";

const CHECKS = "/customer-apps/app-1/builds/row%2Fs/checks";

describe("AppBuildChecksService — the requests the promote checks make", () => {
  beforeEach(() => {
    get.mockReset().mockResolvedValue({ data: {} });
    post.mockReset().mockResolvedValue({ data: {} });
    put.mockReset().mockResolvedValue({ data: {} });
  });

  it("asks for staging's attempt exactly as before, and production's by name", async () => {
    await AppBuildChecksService.get("app-1", "row/s");
    expect(get).toHaveBeenLastCalledWith(CHECKS);
    await AppBuildChecksService.get("app-1", "row/s", "staging");
    expect(get).toHaveBeenLastCalledWith(CHECKS);
    await AppBuildChecksService.get("app-1", "row/s", "production");
    expect(get).toHaveBeenLastCalledWith(CHECKS, { params: { environment: "production" } });
  });

  it("reads one file of one attempt as bytes", async () => {
    const bytes = new Blob(["png"]);
    get.mockResolvedValue({ data: bytes });
    const file = await AppBuildChecksService.file("app-1", "row/s", "shot staging.png", {
      attempt: 2,
      environment: "staging"
    });
    expect(file).toBe(bytes);
    expect(get).toHaveBeenCalledWith(`${CHECKS}/files/shot%20staging.png`, {
      params: { attempt: 2, environment: "staging" },
      responseType: "blob"
    });
  });

  it("marks the changed screens of one attempt seen", async () => {
    await AppBuildChecksService.markChangesSeen("app-1", "row/s", 2);
    expect(post).toHaveBeenCalledWith(`${CHECKS}/changes/seen`, { attempt: 2 });
  });

  it("saves an app's promotion checks", async () => {
    const body = {
      required: null,
      seen_required: true,
      production_smoke: true,
      auto_rollback: false
    };
    await CustomAppsService.setPromotionChecks("app-1", body);
    expect(put).toHaveBeenCalledWith("/customer-apps/app-1/promotion-checks", body);
  });
});
