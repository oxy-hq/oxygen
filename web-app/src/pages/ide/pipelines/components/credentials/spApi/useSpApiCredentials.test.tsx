// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SecretService } from "@/services/secretService";
import type { CreateSecretRequest } from "@/types/secret";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/hooks/api/spApi/useSpApiMarketplaces", () => ({
  default: () => ({ data: [{ id: "ATVPDKIKX0DER", name: "United States" }] })
}));
vi.mock("@/services/secretService", () => ({
  SecretService: { createSecret: vi.fn(), listSecrets: vi.fn() }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { useSpApiCredentials } from "./useSpApiCredentials";

const createSecret = vi.mocked(SecretService.createSecret);
const listSecrets = vi.mocked(SecretService.listSecrets);

const created = (request: CreateSecretRequest) => ({
  id: `id-of-${request.name}`,
  name: request.name,
  created_at: "2024-03-05T00:00:00Z",
  updated_at: "2024-03-05T00:00:00Z",
  created_by: "user-1",
  is_active: true
});

/** The wizard's SP-API form, with these values pasted, submitted for one pipeline. */
const submit = async (pasted: { clientSecret?: string; refreshToken?: string }) => {
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const { result } = renderHook(() => useSpApiCredentials(), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    )
  });
  act(() => {
    if (pasted.clientSecret) result.current.setClientSecret(pasted.clientSecret);
    if (pasted.refreshToken) result.current.setRefreshToken(pasted.refreshToken);
  });

  let failure: unknown = null;
  await act(async () => {
    await result.current.persistSecret("orders").catch((error: unknown) => {
      failure = error;
    });
  });
  return failure;
};

const successToasts = () => vi.mocked(toast.success).mock.calls;
const errorToasts = () => vi.mocked(toast.error).mock.calls;

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  listSecrets.mockResolvedValue({ secrets: [], total: 0 });
  createSecret.mockImplementation((_projectId, request) => Promise.resolve(created(request)));
});

afterEach(() => {
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

describe("useSpApiCredentials persistSecret", () => {
  it("says once that both credentials were stored", async () => {
    expect(await submit({ clientSecret: "lwa-secret", refreshToken: "Atzr|token" })).toBeNull();

    expect(createSecret.mock.calls.map(([, request]) => request.name)).toEqual([
      "SP_API_CLIENT_SECRET",
      "SP_API_REFRESH_TOKEN"
    ]);
    expect(successToasts()).toEqual([
      ["Secrets SP_API_CLIENT_SECRET and SP_API_REFRESH_TOKEN created successfully"]
    ]);
    expect(errorToasts()).toEqual([]);
  });

  it("names the one credential it stored when the other is reused", async () => {
    await submit({ refreshToken: "Atzr|token" });

    expect(createSecret).toHaveBeenCalledTimes(1);
    expect(successToasts()).toEqual([["Secret SP_API_REFRESH_TOKEN created successfully"]]);
  });

  it("says nothing when both credentials are reused", async () => {
    await submit({});

    expect(createSecret).not.toHaveBeenCalled();
    expect(successToasts()).toEqual([]);
  });

  it("still says which credential was stored when the second one fails", async () => {
    const refused = new Error("Request failed with status code 409");
    createSecret.mockImplementation((_projectId, request) =>
      request.name === "SP_API_REFRESH_TOKEN"
        ? Promise.reject(refused)
        : Promise.resolve(created(request))
    );

    // The submit fails, so the wizard stops before writing the pipeline.
    expect(await submit({ clientSecret: "lwa-secret", refreshToken: "Atzr|token" })).toBe(refused);

    expect(errorToasts()).toEqual([
      [
        "Failed to create secret SP_API_REFRESH_TOKEN",
        { description: "Request failed with status code 409" }
      ]
    ]);
    expect(successToasts()).toEqual([["Secret SP_API_CLIENT_SECRET created successfully"]]);
  });

  it("says nothing was stored when the first one fails", async () => {
    createSecret.mockRejectedValue(new Error("Request failed with status code 500"));

    await submit({ clientSecret: "lwa-secret", refreshToken: "Atzr|token" });

    expect(createSecret).toHaveBeenCalledTimes(1);
    expect(errorToasts()).toEqual([
      [
        "Failed to create secret SP_API_CLIENT_SECRET",
        { description: "Request failed with status code 500" }
      ]
    ]);
    expect(successToasts()).toEqual([]);
  });
});
