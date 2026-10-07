// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { AxiosError, type AxiosResponse } from "axios";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import queryKeys from "@/hooks/api/queryKey";
import type { Token } from "@/types/apiToken";

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("sonner", () => ({
  toast: {
    success: (...args: unknown[]) => toastSuccess(...args),
    error: (...args: unknown[]) => toastError(...args)
  }
}));

import {
  listedRevokeError,
  retryUnlessRefused,
  useRevokeListedToken
} from "./useRevokeListedToken";

const LIST_KEY = ["someStaffList"] as const;
const NOT_FOUND = "It is gone, or outside your access.";

const refusal = (status: number) =>
  new AxiosError("Request failed", undefined, undefined, undefined, { status } as AxiosResponse);

/** Revoke through the hook, and hand back what the query client was asked to read again. */
const revokeWith = async (revoke: (id: string) => Promise<Token>) => {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { result } = renderHook(
    () =>
      useRevokeListedToken({
        revoke,
        listKey: LIST_KEY,
        errorMessage: (error) => listedRevokeError(error, NOT_FOUND)
      }),
    { wrapper }
  );
  await act(async () => {
    // A refusal rejects: what the hook does about it is what is under test.
    await result.current.mutateAsync({ id: "t1", name: "laptop" }).catch(() => undefined);
  });
  return invalidate;
};

const refetched = (invalidate: Awaited<ReturnType<typeof revokeWith>>) =>
  invalidate.mock.calls.map(([filters]) => filters?.queryKey);

afterEach(() => {
  vi.clearAllMocks();
});

describe("useRevokeListedToken", () => {
  it("revokes by id, says so by name, and reads the list and the owner's own list again", async () => {
    const revoke = vi.fn().mockResolvedValue({ id: "t1", status: "revoked" });
    const invalidate = await revokeWith(revoke);
    expect(revoke).toHaveBeenCalledWith("t1");
    expect(toastSuccess).toHaveBeenCalledWith('Revoked "laptop"');
    expect(toastError).not.toHaveBeenCalled();
    expect(refetched(invalidate)).toEqual([LIST_KEY, queryKeys.userToken.all]);
  });

  it("reads the list again when the revoke is refused: the row on screen was stale", async () => {
    const invalidate = await revokeWith(vi.fn().mockRejectedValue(refusal(404)));
    expect(toastError).toHaveBeenCalledWith(NOT_FOUND);
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(refetched(invalidate)).toEqual([LIST_KEY, queryKeys.userToken.all]);
  });

  it("reads the list again after a server error, and says to try again", async () => {
    const invalidate = await revokeWith(vi.fn().mockRejectedValue(refusal(500)));
    expect(toastError).toHaveBeenCalledWith("Couldn't revoke the token. Try again.");
    expect(refetched(invalidate)).toEqual([LIST_KEY, queryKeys.userToken.all]);
  });

  it("adds no second message to a 403, and still reads the list again", async () => {
    const invalidate = await revokeWith(vi.fn().mockRejectedValue(refusal(403)));
    expect(toastError).not.toHaveBeenCalled();
    expect(refetched(invalidate)).toEqual([LIST_KEY, queryKeys.userToken.all]);
  });
});

describe("retryUnlessRefused", () => {
  it("never asks again after a 403: the capability gate does not change on a retry", () => {
    expect(retryUnlessRefused(0, refusal(403))).toBe(false);
  });

  it("asks again up to three times for anything else", () => {
    expect(retryUnlessRefused(0, refusal(500))).toBe(true);
    expect(retryUnlessRefused(2, new Error("Network Error"))).toBe(true);
    expect(retryUnlessRefused(3, refusal(500))).toBe(false);
  });
});
