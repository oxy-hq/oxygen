// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PENDING_INVITE_TOKEN_KEY } from "@/hooks/auth/postLoginRedirect";
import { errorContent } from "./errorContent";

// What the real `logout` does: an awaited server round-trip, then a teardown that
// clears sessionStorage.
const logoutLike = async () => {
  await Promise.resolve();
  sessionStorage.clear();
};

// Lets every pending promise callback (and the ones they queue) run.
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const wrongAccount = (signOut: () => Promise<void>) =>
  errorContent(403, { token: "invite-token", retry: vi.fn(), signIn: vi.fn(), signOut });

describe("errorContent — wrong account (403)", () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it("keeps the pending invite token after sign-out has cleared sessionStorage", async () => {
    const signOut = vi.fn(logoutLike);
    const content = wrongAccount(signOut);

    expect(content.primaryAction?.label).toBe("Sign out");
    content.primaryAction?.onClick();
    expect(signOut).toHaveBeenCalledTimes(1);

    // Assert only once sign-out has finished: checking earlier would see a token that
    // the teardown is about to wipe.
    await settle();
    expect(sessionStorage.getItem(PENDING_INVITE_TOKEN_KEY)).toBe("invite-token");
  });

  it("does not store the token before sign-out has finished", () => {
    const content = wrongAccount(vi.fn(logoutLike));

    content.primaryAction?.onClick();

    expect(sessionStorage.getItem(PENDING_INVITE_TOKEN_KEY)).toBeNull();
  });

  it("logs a failed sign-out instead of leaving the rejection unhandled", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const failure = new Error("teardown failed");
    const content = wrongAccount(vi.fn(() => Promise.reject(failure)));

    content.primaryAction?.onClick();
    await settle();

    expect(consoleError).toHaveBeenCalledWith("Sign out failed", failure);
    consoleError.mockRestore();
  });
});
