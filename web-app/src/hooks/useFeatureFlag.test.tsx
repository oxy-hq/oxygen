// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeatureFlagsService } from "@/services/api/featureFlags";
import useCurrentOrg from "@/stores/useCurrentOrg";

vi.mock("@/services/api/featureFlags", () => ({
  FeatureFlagsService: { forOrg: vi.fn() }
}));

import { useFeatureFlag } from "./useFeatureFlag";

const forOrg = vi.mocked(FeatureFlagsService.forOrg);

function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

const org = { id: "org-1", name: "Acme", slug: "acme", role: "member" as const };

describe("useFeatureFlag", () => {
  beforeEach(() => {
    forOrg.mockReset();
    useCurrentOrg.setState({ org: null, role: null });
  });

  it("reads off with no org, and never asks the server", () => {
    const { result } = renderHook(() => useFeatureFlag("new-home"), { wrapper: wrapper() });
    expect(result.current).toEqual({ variant: undefined, enabled: false, isLoading: false });
    expect(forOrg).not.toHaveBeenCalled();
  });

  it("reads off while loading, then serves what the server says", async () => {
    forOrg.mockResolvedValue({ flags: { "new-home": { variant: "on", enabled: true } } });
    useCurrentOrg.setState({ org, role: "member" });
    const { result } = renderHook(() => useFeatureFlag("new-home"), { wrapper: wrapper() });
    expect(result.current).toEqual({ variant: undefined, enabled: false, isLoading: true });
    await waitFor(() => expect(result.current.enabled).toBe(true));
    expect(result.current).toEqual({ variant: "on", enabled: true, isLoading: false });
  });

  it("reads off for a key the server did not return", async () => {
    forOrg.mockResolvedValue({ flags: {} });
    useCurrentOrg.setState({ org, role: "member" });
    const { result } = renderHook(() => useFeatureFlag("missing"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current).toEqual({ variant: undefined, enabled: false, isLoading: false });
  });

  it("reads off when the request fails", async () => {
    forOrg.mockRejectedValue(new Error("boom"));
    useCurrentOrg.setState({ org, role: "member" });
    const { result } = renderHook(() => useFeatureFlag("new-home"), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current).toEqual({ variant: undefined, enabled: false, isLoading: false });
  });
});
