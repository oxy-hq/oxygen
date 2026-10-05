// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { STAGING_HELD_LIMIT, useStagingHeld } from "@/hooks/api/customApps/useCustomApps";
import { CustomAppsService } from "@/services/api/customApps";

/**
 * The held list's polling: it asks for the limit the banner reads as a
 * truncation mark, and a 404 (the caller may not open staging) is terminal —
 * the interval stops rather than asking again every 10s for an answer that
 * cannot change.
 */

afterEach(() => vi.restoreAllMocks());

const harness = () => {
  const client = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return { client, wrapper };
};

/** What the query's `refetchInterval` function answers for its current state. */
const nextInterval = (client: QueryClient) => {
  const query = client.getQueryCache().getAll()[0];
  // A cache query's `options` are typed as the base `QueryOptions`; the
  // observer options it was built from carry `refetchInterval`.
  const { refetchInterval: interval } = query.options as {
    refetchInterval?: number | false | ((q: typeof query) => number | false | undefined);
  };
  return typeof interval === "function" ? interval(query) : interval;
};

describe("useStagingHeld", () => {
  it("requests the limit it exports, and keeps polling after a list", async () => {
    const list = vi.spyOn(CustomAppsService, "listStagingHeld").mockResolvedValue([]);
    const { client, wrapper } = harness();
    const { result } = renderHook(() => useStagingHeld("app-1", true), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(list).toHaveBeenCalledWith("app-1", STAGING_HELD_LIMIT);
    expect(nextInterval(client)).toBe(10_000);
  });

  it("stops polling once the list answers 404", async () => {
    vi.spyOn(CustomAppsService, "listStagingHeld").mockRejectedValue({
      response: { status: 404 }
    });
    const { client, wrapper } = harness();
    const { result } = renderHook(() => useStagingHeld("app-1", true), { wrapper });

    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(nextInterval(client)).toBe(false);
  });

  it("keeps polling through any other error", async () => {
    vi.spyOn(CustomAppsService, "listStagingHeld").mockRejectedValue(new Error("network down"));
    const { client, wrapper } = harness();
    const { result } = renderHook(() => useStagingHeld("app-1", true), { wrapper });

    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(nextInterval(client)).toBe(10_000);
  });
});
