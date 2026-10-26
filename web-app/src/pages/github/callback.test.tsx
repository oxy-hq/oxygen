// @vitest-environment jsdom

import { cleanup, render, waitFor } from "@testing-library/react";
import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import GitHubCallback from "./callback";

const { completeCallback } = vi.hoisted(() => ({ completeCallback: vi.fn() }));

vi.mock("@/services/api", () => ({
  AuthService: { githubAuth: vi.fn() },
  GitHubApiService: { completeCallback }
}));
vi.mock("@/hooks/auth/useGitHubAuth", () => ({ validateGitHubAuthState: () => false }));
// The real module reads a build-time define that vitest does not set.
vi.mock("@/hooks/auth/oauthProxyState", () => ({ oauthStateToken: (s: string) => s }));

/** A refusal from `POST /user/github/callback` as axios rejects with it. */
const refused = (status: number, data: unknown = "") =>
  new AxiosError(
    `Request failed with status code ${status}`,
    "ERR_BAD_REQUEST",
    undefined,
    undefined,
    { status, data } as AxiosResponse
  );

const postMessage = vi.fn();

beforeEach(() => {
  window.history.pushState({}, "", "/github/callback?state=s&installation_id=7");
  Object.defineProperty(window, "opener", { value: { postMessage }, configurable: true });
  vi.spyOn(window, "close").mockImplementation(() => {});
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

/** What the popup tells the window that opened it, after the callback fails. */
async function postedAfter(error: unknown) {
  completeCallback.mockRejectedValueOnce(error);
  render(<GitHubCallback />);
  await waitFor(() => expect(postMessage).toHaveBeenCalled());
  return postMessage.mock.calls[0][0];
}

describe("GitHubCallback", () => {
  it("hands the opener the code and the sentence of an account that must be reconnected", async () => {
    const message = await postedAfter(
      refused(409, {
        code: "github_account_not_connected",
        error: "Connect your GitHub account to choose one of its installations."
      })
    );

    expect(message).toEqual({
      type: "github-callback-error",
      code: "github_account_not_connected",
      reason: "Connect your GitHub account to choose one of its installations."
    });
  });

  it.each([
    [403, /owner or admin/i],
    [404, /isn't one your GitHub account can access/i],
    [400, /expired or is no longer valid/i]
  ])("words a bare %i refusal for a person, not as axios's status line", async (status, words) => {
    const message = await postedAfter(refused(status));

    expect(message.type).toBe("github-callback-error");
    expect(message.reason).not.toMatch(/status code/i);
    expect(message.reason).toMatch(words);
  });
});
