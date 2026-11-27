// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CustomAppsService } from "@/services/api/customApps";
import { CONSOLE_BUILDS_POLL_MS, useAppBuilds } from "./useAppBuilds";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: { listBuilds: vi.fn() }
}));

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    {children}
  </QueryClientProvider>
);

afterEach(() => {
  vi.useRealTimers();
  vi.mocked(CustomAppsService.listBuilds).mockReset();
});

describe("useAppBuilds", () => {
  // A publish from CI moved staging and an open console kept showing the build before.
  it("re-reads the builds while the console watches them", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const list = vi.mocked(CustomAppsService.listBuilds).mockResolvedValue([] as never);
    renderHook(() => useAppBuilds("app-1", { watch: true }), { wrapper });
    await waitFor(() => expect(list).toHaveBeenCalledTimes(1));
    await act(() => vi.advanceTimersByTimeAsync(CONSOLE_BUILDS_POLL_MS));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  });

  it("reads them once for every other reader", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const list = vi.mocked(CustomAppsService.listBuilds).mockResolvedValue([] as never);
    renderHook(() => useAppBuilds("app-1"), { wrapper });
    await waitFor(() => expect(list).toHaveBeenCalledTimes(1));
    await act(() => vi.advanceTimersByTimeAsync(CONSOLE_BUILDS_POLL_MS * 3));
    expect(list).toHaveBeenCalledTimes(1);
  });
});
