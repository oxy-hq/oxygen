import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
  patch: vi.fn(),
  delete: vi.fn()
}));
vi.mock("./axios", () => ({ apiClient: api }));

import { NotificationsService } from "./notifications";

beforeEach(() => {
  api.get.mockReset().mockResolvedValue({ data: {} });
  api.post.mockReset().mockResolvedValue({ data: {} });
  api.put.mockReset().mockResolvedValue({ data: {} });
  api.patch.mockReset().mockResolvedValue({ data: {} });
  api.delete.mockReset().mockResolvedValue({ data: "" });
});

describe("NotificationsService.inbox", () => {
  it("sends only the bell's two params when nothing narrows it", async () => {
    await NotificationsService.inbox({ unreadOnly: false, limit: 50 });
    expect(api.get).toHaveBeenCalledWith("/notifications", {
      params: { unread_only: false, limit: 50 }
    });
  });

  it("sends the filter and the cursor; an app source is also app_id", async () => {
    await NotificationsService.inbox({
      unreadOnly: true,
      limit: 50,
      source: "app-1",
      category: "operations/food-safety",
      before: "n-9"
    });
    expect(api.get).toHaveBeenCalledWith("/notifications", {
      params: {
        unread_only: true,
        limit: 50,
        source: "app-1",
        app_id: "app-1",
        category: "operations/food-safety",
        before: "n-9"
      }
    });
  });

  it("narrows to Oxygen by source alone", async () => {
    await NotificationsService.inbox({
      unreadOnly: false,
      limit: 50,
      source: "oxygen",
      kind: "oxy.anomaly"
    });
    expect(api.get).toHaveBeenCalledWith("/notifications", {
      params: { unread_only: false, limit: 50, source: "oxygen", kind: "oxy.anomaly" }
    });
  });
});

describe("NotificationsService writes", () => {
  it("marks read what the filter covers, as query params", async () => {
    await NotificationsService.markAllRead({ source: "app-1", thread: "t-1" });
    expect(api.post).toHaveBeenCalledWith("/notifications/read-all", undefined, {
      params: { source: "app-1", app_id: "app-1", thread: "t-1" }
    });
  });

  it("marks everything read with no params", async () => {
    await NotificationsService.markAllRead();
    expect(api.post).toHaveBeenCalledWith("/notifications/read-all", undefined, { params: {} });
  });

  it("patches settings against a version with If-Match, and without one when it has none", async () => {
    const patch = { global: { paused_until: null } };
    await NotificationsService.updateSettings(patch, "v7");
    await NotificationsService.updateSettings(patch);
    expect(api.patch).toHaveBeenNthCalledWith(1, "/notifications/settings", patch, {
      headers: { "If-Match": "v7" }
    });
    expect(api.patch).toHaveBeenNthCalledWith(2, "/notifications/settings", patch, {
      headers: undefined
    });
  });

  it("patches and removes a device by its escaped id", async () => {
    await NotificationsService.updateDevice("d/1", { enabled: false });
    await NotificationsService.removeDevice("d/1");
    expect(api.patch).toHaveBeenCalledWith("/notifications/devices/d%2F1", { enabled: false });
    expect(api.delete).toHaveBeenCalledWith("/notifications/devices/d%2F1");
  });

  it("answers a registration with the device's id, or null from a server that sends none", async () => {
    const input = { platform: "web" as const, token: "https://push.example/sub/abc" };
    api.post.mockResolvedValueOnce({ data: { id: "d-9" } });
    await expect(NotificationsService.registerDevice(input)).resolves.toBe("d-9");
    api.post.mockResolvedValueOnce({ data: "" });
    await expect(NotificationsService.registerDevice(input)).resolves.toBeNull();
    expect(api.post).toHaveBeenCalledWith("/notifications/devices", input);
  });

  it("runs an action by id, escaping both ids", async () => {
    api.post.mockResolvedValue({ data: { ok: true, url: "https://x/y" } });
    await expect(NotificationsService.runAction("n 1", "a/b")).resolves.toEqual({
      ok: true,
      url: "https://x/y"
    });
    expect(api.post).toHaveBeenCalledWith("/notifications/n%201/actions/a%2Fb");
  });
});

describe("NotificationsService reads", () => {
  it("reads the catalogue", async () => {
    await NotificationsService.catalog();
    expect(api.get).toHaveBeenCalledWith("/notifications/catalog");
  });

  it("reads the settings document and the devices", async () => {
    await NotificationsService.settings();
    await NotificationsService.devices();
    expect(api.get).toHaveBeenNthCalledWith(1, "/notifications/settings");
    expect(api.get).toHaveBeenNthCalledWith(2, "/notifications/devices");
  });
});
