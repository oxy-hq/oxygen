// @vitest-environment jsdom
import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthService, FrontlineService } from "@/services/api";
import {
  classifyCrewSignInError,
  kioskDeviceQueryOptions,
  resolveCrewDestination,
  returnToPointsAtCustomApp
} from "./useFrontline";

vi.mock("@/services/api", () => ({
  AuthService: { validateReturnTo: vi.fn() },
  FrontlineService: { deviceStatus: vi.fn() }
}));

const APP_URL = "https://acme--store.customer-apps.example.com/";
const KIOSK_APP_URL = "https://acme.example.com/a/store/";

const httpError = (status: number) => ({ response: { status } });

describe("classifyCrewSignInError", () => {
  // The server folds every sign-in refusal into one 401 on purpose; the page
  // must not grow a vocabulary the server chose not to have.
  it("reads a 401 as a mismatch, whatever the underlying reason", () => {
    expect(classifyCrewSignInError(httpError(401))).toBe("mismatch");
  });

  it("reads a 429 as rate-limited", () => {
    expect(classifyCrewSignInError(httpError(429))).toBe("rate_limited");
  });

  it("reads everything else — 503, network, garbage — as unavailable", () => {
    expect(classifyCrewSignInError(httpError(503))).toBe("unavailable");
    expect(classifyCrewSignInError(new Error("Network Error"))).toBe("unavailable");
    expect(classifyCrewSignInError(undefined)).toBe("unavailable");
  });
});

describe("returnToPointsAtCustomApp", () => {
  it("matches the custom-app subdomain scheme", () => {
    expect(returnToPointsAtCustomApp(APP_URL)).toBe(true);
  });

  it("matches the app-host path scheme, absolute or relative", () => {
    expect(returnToPointsAtCustomApp("https://app.example.com/customer-apps/acme/store/")).toBe(
      true
    );
    expect(returnToPointsAtCustomApp("/customer-apps/acme/store/")).toBe(true);
  });

  it("ignores destinations inside the main product", () => {
    expect(returnToPointsAtCustomApp("https://app.example.com/ide")).toBe(false);
    expect(returnToPointsAtCustomApp("/threads/abc")).toBe(false);
  });

  it("ignores a missing or unparsable value", () => {
    expect(returnToPointsAtCustomApp(null)).toBe(false);
    expect(returnToPointsAtCustomApp(undefined)).toBe(false);
    expect(returnToPointsAtCustomApp("")).toBe(false);
    expect(returnToPointsAtCustomApp("http://[not a url")).toBe(false);
  });
});

describe("resolveCrewDestination", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("prefers the return_to the app sent the worker here with", async () => {
    vi.mocked(AuthService.validateReturnTo).mockResolvedValue(true);
    await expect(resolveCrewDestination(APP_URL, KIOSK_APP_URL)).resolves.toBe(APP_URL);
    expect(AuthService.validateReturnTo).toHaveBeenCalledTimes(1);
    expect(AuthService.validateReturnTo).toHaveBeenCalledWith(APP_URL);
  });

  it("falls back to the kiosk's enrolled app, and validates that one too", async () => {
    vi.mocked(AuthService.validateReturnTo).mockResolvedValue(true);
    await expect(resolveCrewDestination(undefined, KIOSK_APP_URL)).resolves.toBe(KIOSK_APP_URL);
    expect(AuthService.validateReturnTo).toHaveBeenCalledWith(KIOSK_APP_URL);
  });

  it("falls through to the kiosk's app when the server rejects the return_to", async () => {
    vi.mocked(AuthService.validateReturnTo)
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true);
    await expect(resolveCrewDestination("https://evil.example.com/", KIOSK_APP_URL)).resolves.toBe(
      KIOSK_APP_URL
    );
  });

  it("resolves to null when neither destination survives", async () => {
    vi.mocked(AuthService.validateReturnTo).mockResolvedValue(false);
    await expect(resolveCrewDestination(APP_URL, KIOSK_APP_URL)).resolves.toBeNull();
    await expect(resolveCrewDestination(undefined, null)).resolves.toBeNull();
  });
});

describe("the kiosk probe remembers whether this browser is a kiosk", () => {
  const probe = () => new QueryClient().fetchQuery(kioskDeviceQueryOptions);
  const remembered = () => localStorage.getItem("oxy_kiosk_browser");

  afterEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it("remembers a bound answer, and an unbound one never forgets it", async () => {
    vi.mocked(FrontlineService.deviceStatus).mockResolvedValueOnce({
      bound: true,
      org: "acme",
      orgName: "Acme",
      device: "Front counter",
      returnTo: null
    });
    await probe();
    expect(remembered()).toBe("1");

    // Forgetting is for leaving and revoking (`forgetKioskBrowser`), which
    // know it happened. A probe's "not bound" may be a kiosk whose lookup
    // misread, and forgetting on it would switch the sign-out check off.
    vi.mocked(FrontlineService.deviceStatus).mockResolvedValueOnce({ bound: false });
    await probe();
    expect(remembered()).toBe("1");
  });

  it("does not start remembering on an unbound answer", async () => {
    vi.mocked(FrontlineService.deviceStatus).mockResolvedValueOnce({ bound: false });
    await probe();
    expect(remembered()).toBeNull();
  });

  it("leaves the memory alone when the probe fails, and reads as unbound", async () => {
    localStorage.setItem("oxy_kiosk_browser", "1");
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.mocked(FrontlineService.deviceStatus).mockRejectedValueOnce(new Error("Network Error"));
    await expect(probe()).resolves.toEqual({ bound: false });
    expect(remembered()).toBe("1");
    warn.mockRestore();
  });
});
