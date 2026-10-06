// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import CliAuth from ".";
import type { CliSession } from "./cliSession";

vi.setConfig({ testTimeout: 20000 });

const CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const PKCE_URL = `/cli-auth?port=53124&state=nonce&code_challenge=${CHALLENGE}&hostname=luong-mbp`;
const LEGACY_URL = "/cli-auth?port=53124&state=nonce";

// Every navigation the page makes goes through `leaveTo`, so a test can see where it would go.
const leaveTo = vi.fn();
let session: CliSession | null = { token: "session.jwt", email: "luong@example.com" };
vi.mock("./cliSession", () => ({
  leaveTo: (url: string) => leaveTo(url),
  readCliSession: () => Promise.resolve(session)
}));

type MutateOptions = { onSuccess?: (data: { code: string }) => void; onError?: (e: Error) => void };
const mutate = vi.fn();
let mutation: { isPending: boolean; isError: boolean; error: Error | null } = {
  isPending: false,
  isError: false,
  error: null
};
// The mutation hook is the seam: this file is about when the page asks, and where it then goes.
vi.mock("@/hooks/api/cliAuth/useAuthorizeCli", () => ({
  default: () => ({ mutate, ...mutation })
}));

afterEach(() => {
  cleanup();
  leaveTo.mockReset();
  mutate.mockReset();
  session = { token: "session.jwt", email: "luong@example.com" };
  mutation = { isPending: false, isError: false, error: null };
});

const open = (url: string) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <CliAuth />
    </MemoryRouter>
  );

describe("/cli-auth with a code_challenge (PKCE)", () => {
  it("names the computer and waits: no request and no redirect without a click", async () => {
    open(PKCE_URL);
    expect(await screen.findByTestId("cli-auth-hostname")).toHaveTextContent("luong-mbp");
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("luong@example.com");
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("authorizes on confirm, then sends oxyc a code and never the session token", async () => {
    const user = userEvent.setup({ delay: null });
    open(PKCE_URL);
    await user.click(await screen.findByTestId("cli-auth-confirm"));

    expect(mutate).toHaveBeenCalledTimes(1);
    const [body, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    expect(body).toEqual({ code_challenge: CHALLENGE, hostname: "luong-mbp" });

    options.onSuccess?.({ code: "single-use" });
    expect(leaveTo).toHaveBeenCalledWith(
      "http://127.0.0.1:53124/callback?code=single-use&state=nonce"
    );
    expect(leaveTo.mock.calls.flat().join(" ")).not.toContain("session.jwt");
    await waitFor(() =>
      expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("Login complete")
    );
  });

  it("authorizes nothing when the person cancels", async () => {
    const user = userEvent.setup({ delay: null });
    open(PKCE_URL);
    await user.click(await screen.findByTestId("cli-auth-cancel"));
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("Nothing was authorized");
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("sends a signed-out person to login, to come back to the same PKCE request", async () => {
    session = null;
    open(PKCE_URL);
    await waitFor(() => expect(leaveTo).toHaveBeenCalledTimes(1));
    const target = leaveTo.mock.calls[0][0] as string;
    expect(target.startsWith("/login?return_to=")).toBe(true);
    const returnTo = new URL(decodeURIComponent(target.slice("/login?return_to=".length)));
    expect(returnTo.pathname).toBe("/cli-auth");
    expect(returnTo.searchParams.get("code_challenge")).toBe(CHALLENGE);
    expect(returnTo.searchParams.get("hostname")).toBe("luong-mbp");
  });

  it("goes back through login when the session lapsed before the click", async () => {
    const user = userEvent.setup({ delay: null });
    open(PKCE_URL);
    await user.click(await screen.findByTestId("cli-auth-confirm"));
    const [, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    options.onError?.(
      new AxiosError("Unauthorized", "ERR", undefined, undefined, { status: 401 } as AxiosResponse)
    );
    expect(leaveTo.mock.calls[0][0]).toContain("/login?return_to=");
  });

  it("says so when authorizing fails, and leaves the button to try again", async () => {
    mutation = {
      isPending: false,
      isError: true,
      error: new AxiosError("Server error", "ERR", undefined, undefined, {
        status: 500
      } as AxiosResponse)
    };
    open(PKCE_URL);
    expect(await screen.findByTestId("cli-auth-error")).toBeInTheDocument();
    expect(screen.getByTestId("cli-auth-confirm")).toBeEnabled();
    expect(leaveTo).not.toHaveBeenCalled();
  });
});

describe("/cli-auth without a code_challenge (older oxyc)", () => {
  it("hands the session token to the loopback at once, with no confirm step", async () => {
    open(LEGACY_URL);
    await waitFor(() =>
      expect(leaveTo).toHaveBeenCalledWith(
        "http://127.0.0.1:53124/callback?token=session.jwt&state=nonce"
      )
    );
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
    expect(mutate).not.toHaveBeenCalled();
  });

  it("sends a signed-out person to login with the legacy return URL", async () => {
    session = null;
    open(LEGACY_URL);
    await waitFor(() => expect(leaveTo).toHaveBeenCalledTimes(1));
    expect(leaveTo.mock.calls[0][0]).toBe(
      `/login?return_to=${encodeURIComponent(`${window.location.origin}/cli-auth?port=53124&state=nonce`)}`
    );
  });
});

describe("/cli-auth with nothing usable", () => {
  it("fails without contacting anything", () => {
    open("/cli-auth?port=evil.example.com&state=nonce");
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("CLI login failed");
    expect(leaveTo).not.toHaveBeenCalled();
    expect(mutate).not.toHaveBeenCalled();
  });

  it("refuses a PKCE request with no hostname rather than falling back to the token handoff", () => {
    open(`/cli-auth?port=53124&state=nonce&code_challenge=${CHALLENGE}`);
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("CLI login failed");
    expect(leaveTo).not.toHaveBeenCalled();
  });
});
