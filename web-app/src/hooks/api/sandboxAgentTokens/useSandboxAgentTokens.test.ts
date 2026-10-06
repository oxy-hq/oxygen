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

import { SandboxAgentTokensService } from "@/services/api/sandboxAgentTokens";
import { revokeErrorMessage } from "./useSandboxAgentTokens";

const refusal = (status: number) =>
  new AxiosError("Request failed", undefined, undefined, undefined, {
    status,
    data: { error: "not found" }
  } as AxiosResponse);

afterEach(() => {
  get.mockReset();
  post.mockReset();
});

describe("SandboxAgentTokensService", () => {
  it("reads the list out of the `tokens` envelope the route answers", async () => {
    get.mockResolvedValue({ data: { tokens: [{ id: "t1" }, { id: "t2" }] } });
    await expect(SandboxAgentTokensService.list()).resolves.toEqual([{ id: "t1" }, { id: "t2" }]);
    expect(get).toHaveBeenCalledWith("/admin/sandbox-agent-tokens");
  });

  it("revokes by id and hands back the token as it now is", async () => {
    post.mockResolvedValue({ data: { id: "t1", status: "revoked" } });
    await expect(SandboxAgentTokensService.revoke("t1")).resolves.toEqual({
      id: "t1",
      status: "revoked"
    });
    expect(post).toHaveBeenCalledWith("/admin/sandbox-agent-tokens/t1/revoke");
  });
});

describe("revokeErrorMessage", () => {
  it("says a 404 may be a token outside the caller's organizations, not only a missing one", () => {
    expect(revokeErrorMessage(refusal(404))).toMatch(/no longer exists, or it is outside/);
  });

  it("adds nothing to a 403, which the API client has already reported", () => {
    expect(revokeErrorMessage(refusal(403))).toBeNull();
  });

  it("falls back for a server error and for a request that never arrived", () => {
    expect(revokeErrorMessage(refusal(500))).toBe("Couldn't revoke the token. Try again.");
    expect(revokeErrorMessage(new Error("Network Error"))).toBe(
      "Couldn't revoke the token. Try again."
    );
  });
});
