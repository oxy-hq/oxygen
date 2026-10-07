import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";

const get = vi.fn();
const post = vi.fn();
vi.mock("@/services/api/axios", () => ({
  apiClient: {
    get: (...args: unknown[]) => get(...args),
    post: (...args: unknown[]) => post(...args)
  }
}));

import { STANDING_TOKEN_LIST_LIMIT, StandingTokensService } from "@/services/api/standingTokens";
import { revokeErrorMessage } from "./useStandingTokens";

const refusal = (status: number, data: unknown = { error: "not found" }) =>
  new AxiosError("Request failed", undefined, undefined, undefined, {
    status,
    data
  } as AxiosResponse);

afterEach(() => {
  get.mockReset();
  post.mockReset();
});

describe("StandingTokensService", () => {
  it("reads the list out of the `tokens` envelope the route answers", async () => {
    get.mockResolvedValue({ data: { tokens: [{ id: "t1" }, { id: "t2" }] } });
    await expect(StandingTokensService.list()).resolves.toEqual([{ id: "t1" }, { id: "t2" }]);
    expect(get).toHaveBeenCalledWith("/admin/standing-tokens");
  });

  it("revokes by id and hands back the token as it now is", async () => {
    post.mockResolvedValue({ data: { id: "t1", status: "revoked" } });
    await expect(StandingTokensService.revoke("t1")).resolves.toEqual({
      id: "t1",
      status: "revoked"
    });
    expect(post).toHaveBeenCalledWith("/admin/standing-tokens/t1/revoke");
  });

  it("reads a list as cut short at the 500 the route returns at most", () => {
    expect(STANDING_TOKEN_LIST_LIMIT).toBe(500);
  });
});

describe("revokeErrorMessage", () => {
  it("says a 404 is a token that is no longer in the list, and nothing about scope", () => {
    expect(revokeErrorMessage(refusal(404))).toBe("This token is no longer in this list.");
  });

  it("adds nothing to a 403, which the API client has already reported", () => {
    expect(revokeErrorMessage(refusal(403, undefined))).toBeNull();
    expect(revokeErrorMessage(refusal(403, { code: "session_required" }))).toBeNull();
  });

  it("falls back for a server error and for a request that never arrived", () => {
    expect(revokeErrorMessage(refusal(500))).toBe("Couldn't revoke the token. Try again.");
    expect(revokeErrorMessage(new Error("Network Error"))).toBe(
      "Couldn't revoke the token. Try again."
    );
  });
});
