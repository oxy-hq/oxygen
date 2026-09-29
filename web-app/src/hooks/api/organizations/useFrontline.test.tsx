// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import queryKeys from "@/hooks/api/queryKey";
import { FrontlineService } from "@/services/api/frontline";
import type { KioskDevice } from "@/types/frontline";
import { useRevokeDevice } from "./useFrontline";

vi.mock("@/services/api/frontline", () => ({
  FrontlineService: { revokeDevice: vi.fn() }
}));

const THIS_BROWSER: KioskDevice = {
  bound: true,
  id: "kiosk-1",
  org: "poke-house",
  orgName: "Poke House",
  device: "Front counter",
  returnTo: null
};

const revokeAs = async (probe: KioskDevice | undefined, deviceId: string) => {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  if (probe) client.setQueryData(queryKeys.frontline.device(), probe);
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { result } = renderHook(() => useRevokeDevice(), { wrapper });
  await act(() => result.current.mutateAsync({ orgId: "org-1", deviceId }));
  return invalidate;
};

beforeEach(() => {
  vi.mocked(FrontlineService.revokeDevice).mockResolvedValue(undefined);
  localStorage.setItem("oxy_kiosk_browser", "1");
});
afterEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
});

describe("useRevokeDevice — the kiosk this browser is", () => {
  it("forgets this browser was a kiosk when the revoked kiosk is this browser", async () => {
    const invalidate = await revokeAs(THIS_BROWSER, "kiosk-1");
    expect(localStorage.getItem("oxy_kiosk_browser")).toBeNull();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.frontline.device() });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.org.frontlineDevices("org-1")
    });
  });

  it("keeps it when another kiosk is revoked from this one", async () => {
    await revokeAs(THIS_BROWSER, "kiosk-2");
    expect(localStorage.getItem("oxy_kiosk_browser")).toBe("1");
  });

  it("keeps it when the probe has not said which kiosk this is", async () => {
    await revokeAs(undefined, "kiosk-1");
    expect(localStorage.getItem("oxy_kiosk_browser")).toBe("1");
  });
});
