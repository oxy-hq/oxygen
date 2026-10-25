// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CustomAppsService } from "@/services/api/customApps";
import type { AppAvailabilityReading, CustomApp } from "@/types/apps";
import { Availability } from ".";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: { list: vi.fn() }
}));

const reading: AppAvailabilityReading = {
  verdict: "no_opinion",
  objective: 0.99,
  windows: [{ window_minutes: 5, total: 0, failed: 0, failure_ratio: null }],
  open_builds: {
    status: "none_seen",
    live_sessions: 0,
    older_sessions: 0,
    older_builds: 0,
    window_minutes: 1440
  }
};

const renderPanel = (availability?: AppAvailabilityReading) =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Availability app={{ id: "app-1", org_slug: "acme", slug: "store", availability }} />
    </QueryClientProvider>
  );

describe("Availability panel", () => {
  beforeEach(() => {
    vi.mocked(CustomAppsService.list).mockResolvedValue({ items: [], next_offset: null });
  });
  afterEach(() => {
    cleanup();
    vi.resetAllMocks();
  });

  it("renders the reading the list row carries", () => {
    renderPanel(reading);

    expect(screen.getByTestId("admin-app-availability-verdict-pill").textContent).toMatch(
      /no data|no opinion/i
    );
    expect(screen.getByTestId("admin-app-open-builds")).toBeTruthy();
  });

  it("reads the app's row when the caller has no reading", async () => {
    vi.mocked(CustomAppsService.list).mockResolvedValue({
      items: [{ id: "app-1", availability: reading } as unknown as CustomApp],
      next_offset: null
    });

    renderPanel(undefined);

    await waitFor(() => expect(screen.getByTestId("admin-app-availability")).toBeTruthy());
    expect(CustomAppsService.list).toHaveBeenCalledWith({ id: "app-1", limit: 1 });
  });

  it("says there is no reading when the row carries none", async () => {
    renderPanel(undefined);

    await waitFor(() => expect(screen.getByTestId("admin-app-availability-none")).toBeTruthy());
  });
});
