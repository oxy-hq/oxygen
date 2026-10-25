// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CustomAppsService } from "@/services/api/customApps";
import type { AppAvailabilityReading, CustomApp } from "@/types/apps";
import type { LifecycleAppRef } from "./deriveLifecycle";
import { useAppLifecycle } from "./useAppLifecycle";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: {
    listBuilds: vi.fn(),
    fleetHealth: vi.fn(),
    listStagingHeld: vi.fn(),
    list: vi.fn()
  }
}));

const burning: AppAvailabilityReading = {
  verdict: "burning",
  severity: "page",
  burn_rate: 50,
  long_window_minutes: 60,
  short_window_minutes: 5,
  objective: 0.99,
  windows: []
};

const row = (availability?: AppAvailabilityReading): LifecycleAppRef => ({
  id: "app-1",
  org_slug: "acme",
  slug: "store",
  published_at: "2026-09-19T17:30:27Z",
  last_promoted_at: null,
  last_promoted_by_email: null,
  availability
});

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    {children}
  </QueryClientProvider>
);

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(CustomAppsService.listBuilds).mockResolvedValue({ builds: [] });
  vi.mocked(CustomAppsService.listStagingHeld).mockResolvedValue([]);
  vi.mocked(CustomAppsService.list).mockResolvedValue({ items: [], next_offset: null });
  vi.mocked(CustomAppsService.fleetHealth).mockResolvedValue({
    apps: [],
    summary: { down: 0, degraded: 0, not_measured: 0, quiet: 0, operational: 0 },
    total: 0,
    has_more: false,
    evaluated_at: "2026-10-24T00:00:00Z",
    observability_configured: true
  });
});

describe("useAppLifecycle availability", () => {
  it("takes the verdict from the list row and makes no per-app availability request", async () => {
    // Stable between renders, as a registry row is: the hook memoises on it.
    const app = row(burning);
    const { result } = renderHook(() => useAppLifecycle(app), { wrapper });

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    await waitFor(() => expect(CustomAppsService.fleetHealth).toHaveBeenCalled());

    expect(result.current.verdict).toMatchObject({
      status: "down",
      label: "Burning",
      source: "availability"
    });
  });

  it("takes a newer reading from the app's own row, not from the availability endpoint", async () => {
    const cleared = { ...burning, verdict: "healthy", severity: undefined } as const;
    vi.mocked(CustomAppsService.list).mockResolvedValue({
      items: [{ ...row(cleared), name: "Store" } as unknown as CustomApp],
      next_offset: null
    });
    const app = row(burning);
    const { result } = renderHook(() => useAppLifecycle(app), { wrapper });

    await waitFor(() => expect(result.current.verdict.status).toBe("operational"));

    expect(CustomAppsService.list).toHaveBeenCalledWith({ id: "app-1", limit: 1 });
  });

  it("takes a reading from the app's row when the caller had none", async () => {
    vi.mocked(CustomAppsService.list).mockResolvedValue({
      items: [{ ...row(burning), name: "Store" } as unknown as CustomApp],
      next_offset: null
    });
    const app = row(undefined);
    const { result } = renderHook(() => useAppLifecycle(app), { wrapper });

    await waitFor(() => expect(result.current.verdict.source).toBe("availability"));

    expect(CustomAppsService.list).toHaveBeenCalledWith({ id: "app-1", limit: 1 });
    expect(result.current.verdict.status).toBe("down");
  });
});
