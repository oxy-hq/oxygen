// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { StrictMode } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "@/contexts/AuthContext";
import { leaveTo, PENDING_INVITE_TOKEN_KEY } from "@/hooks/auth/postLoginRedirect";
import { AuthService } from "@/services/api";
import type { AuthConfigResponse, AuthResponse, UserInfo } from "@/types/auth";
import TokenLogin from "./TokenLogin";

/**
 * `/token-login#ticket=…` signs a browser in with a one-time ticket.
 *
 * The seams are the two things a test cannot do for real: the redeem call, and
 * the full page load the sign-in leaves by (jsdom cannot navigate). Everything
 * between them stays real — the hook, AuthProvider and its storage, the
 * teardown `logout` shares, and the destination rule shared with `/dev-login` —
 * so what is asserted is what the browser is left holding.
 *
 * Rendered under StrictMode, as the app is: its double-invoked mount is what
 * the page's latch exists for, and a ticket only works once.
 */
vi.mock("@/services/api", () => ({
  AuthService: { redeemBrowserTicket: vi.fn(), validateReturnTo: vi.fn() }
}));
vi.mock("@/hooks/auth/postLoginRedirect", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/auth/postLoginRedirect")>()),
  leaveTo: vi.fn()
}));

const redeem = vi.mocked(AuthService.redeemBrowserTicket);

// Whether a stored session is live is its `exp` against the clock, so the clock
// is pinned. Only `Date` is faked: `findBy` and `waitFor` keep their real timers.
const NOW = new Date("2026-10-07T12:00:00Z");
const IN_AN_HOUR = NOW.getTime() / 1000 + 3600;
const AN_HOUR_AGO = NOW.getTime() / 1000 - 3600;

const segment = (value: unknown) =>
  btoa(JSON.stringify(value)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const jwt = (header: object, claims: object) => `${segment(header)}.${segment(claims)}.signature`;

/** What every ordinary login signs: no `kid`. */
const HUMAN_HEADER = { alg: "HS256", typ: "JWT" };
/** What a redeemed ticket signs: the API token's id, as `tok:<id>`. */
const TOKEN_HEADER = { ...HUMAN_HEADER, kid: "tok:6f1c2a4e-9b1d-4c53-8a0e-2f6d1b7c9e10" };

const MAYA: UserInfo = {
  id: "user-maya",
  email: "maya@acme.test",
  name: "Maya",
  is_owner: false,
  is_app_admin: false
};
const BOT_OWNER: UserInfo = { ...MAYA, id: "user-sam", email: "sam@acme.test", name: "Sam" };

/** What the server answers a good ticket with. */
const REDEEMED: AuthResponse = {
  token: jwt(TOKEN_HEADER, { sub: BOT_OWNER.id, exp: IN_AN_HOUR }),
  user: BOT_OWNER,
  orgs: [{ id: "org-1", name: "Acme", slug: "acme", role: "member" }]
};

const BRANCHES = JSON.stringify({ projectBranches: { "project-1": "maya/wip" } });
const APP_URL = "https://acme--store.customer-apps.example.com/";
const AUTH_CONFIG = { auth_enabled: true, mode: "cloud" } as AuthConfigResponse;

/** Leave a session in the browser, with the per-user state a session accumulates. */
function storeSession(header: object, exp: number, user: UserInfo = MAYA): string {
  const token = jwt(header, { sub: user.id, exp });
  localStorage.setItem("auth_token", token);
  localStorage.setItem("user", JSON.stringify(user));
  localStorage.setItem("ide-branch-storage", BRANCHES);
  return token;
}

/** Navigate the browser to `url` and mount the app's routes that matter here. */
function open(url: string) {
  window.history.replaceState(null, "", url);
  return render(
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <AuthProvider authConfig={AUTH_CONFIG}>
          <MemoryRouter initialEntries={["/token-login"]}>
            <Routes>
              <Route path='/token-login' element={<TokenLogin />} />
              <Route path='/login' element={<div data-testid='at-login' />} />
              <Route path='/' element={<div data-testid='at-home' />} />
            </Routes>
          </MemoryRouter>
        </AuthProvider>
      </QueryClientProvider>
    </StrictMode>
  );
}

/**
 * Give a redeem that was started the chance to reach the service. React Query
 * calls the mutation function a few microtasks after `mutate`, so "it was not
 * called" is only worth asserting one turn of the event loop later.
 */
const settle = () => act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));

const refusal = (status: number, data: unknown) =>
  new AxiosError(`Request failed with status code ${status}`, "ERR_BAD_REQUEST", undefined, null, {
    status,
    data
  } as AxiosResponse);

const signedIn = () => waitFor(() => expect(leaveTo).toHaveBeenCalledTimes(1));

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  redeem.mockResolvedValue(REDEEMED);
  vi.mocked(AuthService.validateReturnTo).mockResolvedValue(false);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.clearAllMocks();
  localStorage.clear();
  sessionStorage.clear();
  window.history.replaceState(null, "", "/");
});

describe("/token-login with no session in the browser", () => {
  it("redeems the ticket once, stores the session and leaves by a full page load", async () => {
    open("/token-login#ticket=tkt-1");
    await signedIn();

    expect(redeem).toHaveBeenCalledTimes(1);
    expect(redeem.mock.calls[0][0]).toBe("tkt-1");
    expect(localStorage.getItem("auth_token")).toBe(REDEEMED.token);
    expect(JSON.parse(localStorage.getItem("user") ?? "null")).toEqual(BOT_OWNER);
    expect(leaveTo).toHaveBeenCalledWith("/");
    expect(screen.queryByTestId("token-login-replace-session")).not.toBeInTheDocument();
  });

  it("takes the ticket out of the address bar before the redeem has answered", async () => {
    redeem.mockReturnValue(new Promise(() => {}));
    open("/token-login?from=agent#ticket=tkt-1&next=%2Fide");

    // Synchronously after mount: the path and query stay, the fragment is gone.
    expect(window.location.hash).toBe("");
    expect(`${window.location.pathname}${window.location.search}`).toBe("/token-login?from=agent");
    expect(window.location.href).not.toContain("tkt-1");

    // …and it was read first: the redeem still gets the ticket.
    await waitFor(() => expect(redeem).toHaveBeenCalledTimes(1));
    expect(redeem.mock.calls[0][0]).toBe("tkt-1");
    expect(screen.getByTestId("token-login-pending")).toHaveTextContent("Signing in…");
  });
});

describe("/token-login over a session already in the browser", () => {
  it("redeems at once over a token session, and clears what its user left behind", async () => {
    storeSession(TOKEN_HEADER, IN_AN_HOUR);
    sessionStorage.setItem(PENDING_INVITE_TOKEN_KEY, "mayas-invite");
    open("/token-login#ticket=tkt-1");
    await signedIn();

    expect(redeem).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem("auth_token")).toBe(REDEEMED.token);
    // The previous user's IDE branch selection does not reach the new session…
    expect(localStorage.getItem("ide-branch-storage")).toBeNull();
    // …and neither does the invite they left pending: it is not followed.
    expect(sessionStorage.getItem(PENDING_INVITE_TOKEN_KEY)).toBeNull();
    expect(leaveTo).toHaveBeenCalledWith("/");
  });

  it("redeems at once over a human session that has expired", async () => {
    storeSession(HUMAN_HEADER, AN_HOUR_AGO);
    open("/token-login#ticket=tkt-1");
    await signedIn();

    expect(redeem).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem("auth_token")).toBe(REDEEMED.token);
  });

  it("asks before replacing a live human session, and redeems only on the click", async () => {
    const mayasToken = storeSession(HUMAN_HEADER, IN_AN_HOUR);
    const user = userEvent.setup();
    open("/token-login#ticket=tkt-1&next=%2Fide");

    const card = screen.getByTestId("token-login-replace-session");
    expect(card).toHaveTextContent("Replace session?");
    expect(card).toHaveTextContent("Signed in as maya@acme.test.");
    // Who the browser would become — the fact that stops a link nobody asked for.
    expect(card).toHaveTextContent("signs this browser in as whoever made the link");
    expect(card).toHaveTextContent("only continue if that was you");
    // Waiting for a click does not mean the ticket waits in the URL.
    expect(window.location.hash).toBe("");

    await settle();
    expect(redeem).not.toHaveBeenCalled();
    expect(localStorage.getItem("auth_token")).toBe(mayasToken);

    await user.click(screen.getByTestId("token-login-confirm"));
    await signedIn();

    expect(redeem).toHaveBeenCalledTimes(1);
    expect(redeem.mock.calls[0][0]).toBe("tkt-1");
    expect(localStorage.getItem("auth_token")).toBe(REDEEMED.token);
    expect(localStorage.getItem("ide-branch-storage")).toBeNull();
    expect(leaveTo).toHaveBeenCalledWith("/ide");
  });

  it("goes home on Cancel, with the person still signed in", async () => {
    const mayasToken = storeSession(HUMAN_HEADER, IN_AN_HOUR);
    const user = userEvent.setup();
    open("/token-login#ticket=tkt-1");

    await user.click(screen.getByTestId("token-login-cancel"));
    expect(await screen.findByTestId("at-home")).toBeInTheDocument();

    await settle();
    expect(redeem).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
    expect(localStorage.getItem("auth_token")).toBe(mayasToken);
    expect(localStorage.getItem("ide-branch-storage")).toBe(BRANCHES);
  });

  it("leaves the person signed in when the ticket they agreed to is refused", async () => {
    const mayasToken = storeSession(HUMAN_HEADER, IN_AN_HOUR);
    redeem.mockRejectedValue(refusal(400, { error: "invalid ticket", code: "invalid_ticket" }));
    const user = userEvent.setup();
    open("/token-login#ticket=tkt-1");

    await user.click(screen.getByTestId("token-login-confirm"));
    expect(await screen.findByTestId("token-login-error")).toBeInTheDocument();

    expect(localStorage.getItem("auth_token")).toBe(mayasToken);
    expect(JSON.parse(localStorage.getItem("user") ?? "null")).toEqual(MAYA);
    expect(localStorage.getItem("ide-branch-storage")).toBe(BRANCHES);
    expect(leaveTo).not.toHaveBeenCalled();
  });
});

describe("/token-login when the sign-in fails", () => {
  it("says the link didn't work when the server refuses the ticket", async () => {
    redeem.mockRejectedValue(refusal(400, { error: "invalid ticket", code: "invalid_ticket" }));
    const user = userEvent.setup();
    open("/token-login#ticket=used-already");

    const card = await screen.findByTestId("token-login-error");
    expect(card).toHaveTextContent("Link didn't work");
    expect(card).toHaveTextContent("Links work once and expire in 5 minutes.");
    // The fix is a command, set as code: no backticks around it for a reader to copy.
    expect(card.querySelector("code")).toHaveTextContent(/^oxyc login-link$/);
    expect(redeem).toHaveBeenCalledTimes(1);
    expect(leaveTo).not.toHaveBeenCalled();
    expect(localStorage.getItem("auth_token")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Back to sign in" }));
    expect(await screen.findByTestId("at-login")).toBeInTheDocument();
  });

  it.each([
    ["the request never arrives", new AxiosError("Network Error", "ERR_NETWORK")],
    ["the server errors", refusal(502, "Bad Gateway")]
  ])("blames the server, not the link, when %s", async (_case, error) => {
    redeem.mockRejectedValue(error);
    open("/token-login#ticket=tkt-1");

    const card = await screen.findByTestId("token-login-error");
    expect(card).toHaveTextContent("Couldn't reach the server");
    expect(card).not.toHaveTextContent("didn't work");
    expect(card).not.toHaveTextContent("work once");
  });

  it.each([
    ["no fragment", "/token-login"],
    ["a fragment without a ticket", "/token-login#next=%2Fide"],
    ["an empty ticket", "/token-login#ticket="],
    // Parameters belong in the fragment; a ticket in the query string is not one.
    ["a ticket in the query string", "/token-login?ticket=tkt-1"]
  ])("shows the error card and asks the server nothing for %s", async (_case, url) => {
    // Even over a live human session: there is nothing to agree to.
    storeSession(HUMAN_HEADER, IN_AN_HOUR);
    open(url);

    expect(screen.getByTestId("token-login-error")).toHaveTextContent("Link didn't work");
    expect(screen.queryByTestId("token-login-replace-session")).not.toBeInTheDocument();
    await settle();
    expect(redeem).not.toHaveBeenCalled();
  });
});

describe("/token-login destinations", () => {
  it("lands on a same-origin next", async () => {
    open("/token-login#ticket=tkt-1&next=%2Fide%3Ftab%3Druns");
    await signedIn();
    expect(leaveTo).toHaveBeenCalledWith("/ide?tab=runs");
  });

  // The same rule, and the same fallback, as `/dev-login?next=`.
  it.each([
    ["an absolute URL", "https://evil.example.com/"],
    ["a protocol-relative URL", "//evil.example.com/"]
  ])("ignores a next that is %s", async (_case, next) => {
    open(`/token-login#ticket=tkt-1&next=${encodeURIComponent(next)}`);
    await signedIn();
    expect(leaveTo).toHaveBeenCalledWith("/");
  });

  it("follows a return_to the server allows", async () => {
    vi.mocked(AuthService.validateReturnTo).mockResolvedValue(true);
    open(`/token-login#ticket=tkt-1&return_to=${encodeURIComponent(APP_URL)}&next=%2Fide`);
    await signedIn();

    expect(AuthService.validateReturnTo).toHaveBeenCalledWith(APP_URL);
    expect(leaveTo).toHaveBeenCalledWith(APP_URL);
  });

  it("falls back to next when the server rejects the return_to", async () => {
    open(
      `/token-login#ticket=tkt-1&return_to=${encodeURIComponent("https://evil.example.com/")}&next=%2Fide`
    );
    await signedIn();
    expect(leaveTo).toHaveBeenCalledWith("/ide");
  });
});
