// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { AxiosError, type AxiosResponse } from "axios";
import type { ReactNode } from "react";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SecretService } from "@/services/secretService";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/services/secretService", () => ({
  SecretService: { createSecret: vi.fn() }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { useCreateSecret } from "./useSecretMutations";

const createSecret = vi.mocked(SecretService.createSecret);

/** The error axios rejects with when the server answers `status` with `body`. */
const answered = (status: number, body: unknown) =>
  new AxiosError(`Request failed with status code ${status}`, "ERR_BAD_REQUEST", undefined, null, {
    status,
    data: body
  } as AxiosResponse);

/** Creates a secret named TOAST_CLIENT_SECRET, and returns what it rejected with. */
const createFailing = async () => {
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const { result } = renderHook(() => useCreateSecret(), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    )
  });
  let failure: unknown = null;
  await act(async () => {
    try {
      await result.current.mutateAsync({ name: "TOAST_CLIENT_SECRET", value: "s3cret" });
    } catch (error) {
      failure = error;
    }
  });
  return failure;
};

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

describe("useCreateSecret when the secret is refused", () => {
  it("names the secret and says why, in the server's words", async () => {
    createSecret.mockRejectedValue(
      answered(409, { error: "Secret with name 'TOAST_CLIENT_SECRET' already exists" })
    );

    expect(await createFailing()).toBeInstanceOf(AxiosError);

    // It used to say only "Failed to create secret", for any secret and any reason.
    expect(vi.mocked(toast.error).mock.calls).toEqual([
      [
        "Failed to create secret TOAST_CLIENT_SECRET",
        { description: "Secret with name 'TOAST_CLIENT_SECRET' already exists" }
      ]
    ]);
  });

  it("says what failed when the server gives no reason", async () => {
    createSecret.mockRejectedValue(answered(502, "<html>Bad Gateway</html>"));

    await createFailing();

    expect(vi.mocked(toast.error).mock.calls).toEqual([
      [
        "Failed to create secret TOAST_CLIENT_SECRET",
        { description: "Request failed with status code 502" }
      ]
    ]);
  });
});
