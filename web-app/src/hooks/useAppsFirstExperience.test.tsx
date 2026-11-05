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

const auth = vi.hoisted(() => ({
  value: null as null | { isLocalMode: boolean; authConfig: { apps_first_experience?: boolean } }
}));
vi.mock("@/contexts/AuthContext", () => ({ useOptionalAuth: () => auth.value }));

import useAppsFirstExperience, { useAppsFirstExperienceState } from "./useAppsFirstExperience";

const forOrg = vi.mocked(FeatureFlagsService.forOrg);

function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

const org = { id: "org-1", name: "Acme", slug: "acme", role: "member" as const };
const flags = (enabled: boolean) => ({
  flags: { apps_first_experience: { variant: enabled ? "on" : "off", enabled } }
});

describe("useAppsFirstExperience", () => {
  beforeEach(() => {
    forOrg.mockReset();
    useCurrentOrg.setState({ org: null, role: null });
    auth.value = { isLocalMode: false, authConfig: { apps_first_experience: false } };
  });

  it("reads off outside an AuthProvider and in local mode, without asking", () => {
    auth.value = null;
    useCurrentOrg.setState({ org, role: "member" });
    const { result, rerender } = renderHook(() => useAppsFirstExperience(), {
      wrapper: wrapper()
    });
    expect(result.current).toBe(false);
    auth.value = { isLocalMode: true, authConfig: { apps_first_experience: true } };
    rerender();
    expect(result.current).toBe(false);
    expect(forOrg).not.toHaveBeenCalled();
  });

  it("with no org, reads the auth config", () => {
    auth.value = { isLocalMode: false, authConfig: { apps_first_experience: true } };
    const { result } = renderHook(() => useAppsFirstExperience(), { wrapper: wrapper() });
    expect(result.current).toBe(true);
    expect(forOrg).not.toHaveBeenCalled();
  });

  it("with an org, the org's own value wins over the auth config", async () => {
    forOrg.mockResolvedValue(flags(true));
    useCurrentOrg.setState({ org, role: "member" });
    const { result } = renderHook(() => useAppsFirstExperience(), { wrapper: wrapper() });
    expect(result.current).toBe(false);
    await waitFor(() => expect(result.current).toBe(true));
    expect(forOrg).toHaveBeenCalledWith("org-1");
  });

  it("an org held back reads off though the auth config says on", async () => {
    auth.value = { isLocalMode: false, authConfig: { apps_first_experience: true } };
    forOrg.mockResolvedValue(flags(false));
    const { result } = renderHook(() => useAppsFirstExperience("org-2"), { wrapper: wrapper() });
    expect(result.current).toBe(true);
    await waitFor(() => expect(result.current).toBe(false));
    expect(forOrg).toHaveBeenCalledWith("org-2");
  });

  it("keeps the auth config when the org's flags fail to load", async () => {
    auth.value = { isLocalMode: false, authConfig: { apps_first_experience: true } };
    forOrg.mockRejectedValue(new Error("boom"));
    useCurrentOrg.setState({ org, role: "member" });
    const { result } = renderHook(() => useAppsFirstExperience(), { wrapper: wrapper() });
    await waitFor(() => expect(forOrg).toHaveBeenCalled());
    expect(result.current).toBe(true);
  });

  it("is pending only while an org's own answer is on its way", async () => {
    let resolve: (v: ReturnType<typeof flags>) => void = () => {};
    forOrg.mockReturnValue(new Promise((r) => (resolve = r)));
    const noOrg = renderHook(() => useAppsFirstExperienceState(), { wrapper: wrapper() });
    expect(noOrg.result.current.isPending).toBe(false);
    const { result } = renderHook(() => useAppsFirstExperienceState("org-3"), {
      wrapper: wrapper()
    });
    expect(result.current).toEqual({ enabled: false, isPending: true });
    resolve(flags(true));
    await waitFor(() => expect(result.current).toEqual({ enabled: true, isPending: false }));
  });
});
