// @vitest-environment jsdom

import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TokenOptions } from "@/types/apiToken";
import CliAuth from ".";
import type { CliSession } from "./cliSession";

/**
 * `/cli-auth` with `kind=agent`: the approval `oxyc tokens create --agent` opens. The sandbox
 * agent approval's own tests (`index.test.tsx`) pin the sheet's shared rules in full; here they
 * are checked to hold for this request too, beside what is this request's alone.
 */

vi.setConfig({ testTimeout: 20000 });

const CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const PKCE_URL = `/cli-auth?port=53124&state=nonce&code_challenge=${CHALLENGE}&hostname=luong-mbp`;

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
vi.mock("@/hooks/api/cliAuth/useAuthorizeCli", () => ({
  default: () => ({ mutate, ...mutation })
}));

type OptionsQuery = {
  data?: TokenOptions;
  isLoading: boolean;
  isError: boolean;
  refetch: () => void;
};
/** What the signed-in person holds: an org member, staff, a partner, or both. */
const holding = (can_platform = false, can_partner = false): OptionsQuery => ({
  data: {
    orgs: [],
    can_platform,
    can_partner,
    agent: { default_hours: 8, max_hours: 168 }
  },
  isLoading: false,
  isError: false,
  refetch: vi.fn()
});
const MEMBER = () => holding();
const STAFF = () => holding(true);
let tokenOptions: OptionsQuery = MEMBER();
vi.mock("@/hooks/api/userTokens/useUserTokens", () => ({
  useTokenOptions: () => tokenOptions
}));

// jsdom never reports its document focused, and Approve arms only on a page that is.
const pageFocused = vi.spyOn(document, "hasFocus");
const windowListeners = vi.spyOn(window, "addEventListener");

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  pageFocused.mockReturnValue(true);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  windowListeners.mockClear();
  leaveTo.mockReset();
  mutate.mockReset();
  session = { token: "session.jwt", email: "luong@example.com" };
  mutation = { isPending: false, isError: false, error: null };
  tokenOptions = MEMBER();
});

const open = (url: string) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <CliAuth />
    </MemoryRouter>
  );

const agentUrl = (query = "hours=8&name=triage%20run") => `${PKCE_URL}&kind=agent&${query}`;

const httpError = (status: number, data: unknown = {}) =>
  new AxiosError("Request failed", "ERR", undefined, undefined, { status, data } as AxiosResponse);

/** Time with the request in front of the person. A full second of it arms Approve. */
const attend = async (ms = 1000) => {
  await act(async () => {});
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
};

const armedApprove = async () => {
  const approve = await screen.findByTestId("cli-auth-confirm");
  await attend();
  expect(approve).toBeEnabled();
  return approve;
};

/** Approve, and hand back what the page asked of the server and how it hears the answer. */
const approved = async () => {
  const user = userEvent.setup({ delay: null });
  await user.click(await armedApprove());
  expect(mutate).toHaveBeenCalledTimes(1);
  return mutate.mock.calls[0] as [{ mint: unknown }, MutateOptions];
};

const sheet = () => screen.getByTestId("cli-auth-card");
const standingBox = () => screen.queryByTestId("cli-auth-agent-standing");

describe("/cli-auth with kind=agent", () => {
  it("shows the computer asking, who it acts as, the name, the reach and the lifetime", async () => {
    open(agentUrl());
    expect(await screen.findByTestId("cli-auth-hostname")).toHaveTextContent("luong-mbp");
    expect(sheet().tagName).toBe("MAIN");
    expect(within(sheet()).getByRole("heading", { level: 1 })).toHaveTextContent(
      "Approve an agent token?"
    );
    expect(sheet()).toHaveTextContent(
      "Asked from the computer luong-mbp, for an agent that will act as luong@example.com."
    );

    expect(screen.getByTestId("cli-auth-mint-name")).toHaveTextContent("triage run");
    expect(screen.getByTestId("cli-auth-agent-reach")).toHaveTextContent(
      "Everything you can reach through your organization memberships"
    );
    const lifetime = screen.getByTestId("cli-auth-mint-lifetime");
    expect(lifetime).toHaveTextContent(/^8 hours/);
    expect(lifetime).toHaveTextContent(/until \w+ \d+, \d{4}/);

    const [can, cannot] = within(screen.getByTestId("cli-auth-mint-powers")).getAllByRole("list");
    const acts = (list: HTMLElement) =>
      within(list)
        .getAllByRole("listitem")
        .map((act) => act.textContent);
    expect(acts(can)).toEqual([
      "Do what you can do through the API",
      "Open a browser session as itself"
    ]);
    expect(acts(cannot)).toEqual([
      "Create, extend or revoke tokens",
      "Be extended",
      "Last past the time shown"
    ]);

    await armedApprove();
    expect(screen.queryByTestId("cli-auth-mint-problem")).not.toBeInTheDocument();
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("names the token as the server will when oxyc sent no name, and sends none", async () => {
    open(agentUrl("hours=24"));
    expect(await screen.findByTestId("cli-auth-mint-name")).toHaveTextContent("agent on luong-mbp");
    const [body] = await approved();
    expect(body.mint).toEqual({ kind: "agent", standing: false, expires_in_hours: 24 });
  });

  it("gives the server's default lifetime to a request that named none", async () => {
    open(agentUrl("name=triage%20run"));
    expect(await screen.findByTestId("cli-auth-mint-lifetime")).toHaveTextContent(/^8 hours/);
    const [body] = await approved();
    expect(body.mint).toEqual({
      kind: "agent",
      standing: false,
      expires_in_hours: 8,
      name: "triage run"
    });
  });

  it("approves with the mint object, then sends oxyc a code and never the session", async () => {
    open(agentUrl());
    const [body, options] = await approved();
    expect(body).toEqual({
      code_challenge: CHALLENGE,
      hostname: "luong-mbp",
      mint: { kind: "agent", standing: false, expires_in_hours: 8, name: "triage run" }
    });

    act(() => options.onSuccess?.({ code: "single-use" }));
    expect(leaveTo).toHaveBeenCalledTimes(1);
    expect(leaveTo).toHaveBeenCalledWith(
      "http://127.0.0.1:53124/callback?code=single-use&state=nonce"
    );
    expect(leaveTo.mock.calls.flat().join(" ")).not.toContain("session.jwt");
    expect(sheet()).toHaveAttribute("data-status", "done");
    expect(sheet()).toHaveTextContent("Token approved");
    expect(within(sheet()).queryByRole("button")).not.toBeInTheDocument();
  });
});

describe("/cli-auth with kind=agent: a person who expected a sandbox request sees it is not one", () => {
  it("says so before the request, in a box the sandbox sheet does not have", async () => {
    open(agentUrl());
    const notice = await screen.findByTestId("cli-auth-agent-notice");
    expect(notice).toHaveTextContent("This token acts as you, everywhere you can go");
    expect(notice).toHaveTextContent(
      "It is not a sandbox agent token, which reaches only the sandboxes of the apps it names."
    );
    // Before the rows it qualifies, so it is read first.
    const request = screen.getByTestId("cli-auth-agent");
    expect(notice.compareDocumentPosition(request) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // In the page's own ink: red stays for what is refused.
    expect(notice.className).not.toMatch(/destructive/);
  });

  it("shows the command that should have led here, and nothing of the sandbox request", async () => {
    open(agentUrl());
    const approve = await screen.findByTestId("cli-auth-confirm");
    expect(approve).toHaveTextContent(/^Approve$/);
    expect(screen.getByTestId("cli-auth-mint-command")).toHaveTextContent(
      /^oxyc tokens create --agent$/
    );
    expect(sheet()).toHaveTextContent("Continue only if you just ran this on luong-mbp");
    // Nothing of the sandbox request is on it.
    expect(sheet()).not.toHaveTextContent("Approve a sandbox agent token?");
    expect(sheet()).not.toHaveTextContent("--sandbox-agent");
    expect(screen.queryByTestId("cli-auth-mint-app")).not.toBeInTheDocument();
  });

  it("leaves the sandbox request as it was: no box, and a button that says Approve", async () => {
    tokenOptions = {
      ...MEMBER(),
      data: {
        orgs: [],
        can_platform: true,
        can_partner: false,
        sandbox_agent: { default_hours: 8, max_hours: 168, max_apps: 5 },
        sandbox_apps: [
          {
            id: "a1",
            org_id: "o1",
            org_slug: "acme",
            org_name: "Acme",
            slug: "store-ops",
            name: "Store Ops"
          }
        ]
      }
    };
    open(`${PKCE_URL}&kind=sandbox_agent&apps=acme/store-ops&hours=8&name=x`);
    const approve = await screen.findByTestId("cli-auth-confirm");
    expect(approve).toHaveTextContent(/^Approve$/);
    expect(screen.queryByTestId("cli-auth-agent-notice")).not.toBeInTheDocument();
    expect(screen.getByTestId("cli-auth-mint-command")).toHaveTextContent(
      "oxyc tokens create --sandbox-agent"
    );
  });

  it.each(["cli-auth-hostname", "cli-auth-approver"])("%s wraps as a whole word", async (id) => {
    open(agentUrl());
    const name = await screen.findByTestId(id);
    expect(name).toHaveClass("break-words");
    expect(name).not.toHaveClass("break-all");
  });
});

describe("/cli-auth with kind=agent: staff and partner access", () => {
  const standingOf = (call: number) =>
    (mutate.mock.calls[call] as [{ mint: { standing: boolean } }])[0].mint.standing;

  it("leaves what the agent asked for off: approving without a tick grants the smaller thing", async () => {
    tokenOptions = STAFF();
    open(agentUrl("hours=8&standing=1"));
    const box = await screen.findByTestId("cli-auth-agent-standing");
    expect(box).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("checkbox", { name: /Include my staff access/ })).toBe(box);
    // That the agent asked, and that asking granted nothing.
    expect(screen.getByTestId("cli-auth-agent-standing-asked")).toHaveTextContent(
      /^The agent asked to include your staff access\. It is off unless you tick it\.$/
    );
    expect(screen.getByTestId("cli-auth-agent-standing-adds")).toHaveTextContent(
      /^Adds every organization on this deployment\.$/
    );
    // The command the person should have run names the flag too.
    expect(screen.getByTestId("cli-auth-mint-command")).toHaveTextContent(
      /^oxyc tokens create --agent --standing$/
    );
    expect(screen.getByTestId("cli-auth-confirm")).toHaveTextContent(/^Approve$/);

    const [body] = await approved();
    expect(body.mint).toEqual({ kind: "agent", standing: false, expires_in_hours: 8 });
  });

  it("sends standing only once the person ticks the box, and says so on the button", async () => {
    const user = userEvent.setup({ delay: null });
    tokenOptions = STAFF();
    open(agentUrl("hours=8&standing=1"));
    const box = await screen.findByTestId("cli-auth-agent-standing");
    const approve = screen.getByTestId("cli-auth-confirm");

    await user.click(box);
    expect(box).toHaveAttribute("aria-checked", "true");
    expect(approve).toHaveTextContent(/^Approve with staff access$/);
    await attend();
    await user.click(approve);
    expect(mutate.mock.calls[0][0]).toEqual({
      code_challenge: CHALLENGE,
      hostname: "luong-mbp",
      mint: { kind: "agent", standing: true, expires_in_hours: 8 }
    });

    // Cleared again, the button and the request go back to the smaller thing.
    await user.click(box);
    expect(box).toHaveAttribute("aria-checked", "false");
    expect(approve).toHaveTextContent(/^Approve$/);
    await attend();
    await user.click(approve);
    expect(standingOf(1)).toBe(false);
  });

  it("words the box and the button by what the person holds", async () => {
    const user = userEvent.setup({ delay: null });
    tokenOptions = holding(false, true);
    open(agentUrl("standing=1"));
    await user.click(await screen.findByTestId("cli-auth-agent-standing"));
    expect(screen.getByRole("checkbox", { name: /Include my partner access/ })).toBeInTheDocument();
    expect(screen.getByTestId("cli-auth-agent-standing-asked")).toHaveTextContent(
      "The agent asked to include your partner access. It is off unless you tick it."
    );
    expect(screen.getByTestId("cli-auth-agent-standing-adds")).toHaveTextContent(
      /^Adds your client organizations\.$/
    );
    expect(screen.getByTestId("cli-auth-confirm")).toHaveTextContent(
      /^Approve with partner access$/
    );
    cleanup();

    tokenOptions = holding(true, true);
    open(agentUrl("standing=1"));
    await user.click(await screen.findByTestId("cli-auth-agent-standing"));
    expect(
      screen.getByRole("checkbox", { name: /Include my staff and partner access/ })
    ).toBeInTheDocument();
    expect(screen.getByTestId("cli-auth-agent-standing-adds")).toHaveTextContent(
      /^Adds every organization on this deployment, and your client organizations\.$/
    );
    expect(screen.getByTestId("cli-auth-confirm")).toHaveTextContent(
      /^Approve with staff and partner access$/
    );
  });

  it("counts a fresh second whenever the box changes, so a click on its way approves nothing", async () => {
    const user = userEvent.setup({ delay: null });
    tokenOptions = STAFF();
    open(agentUrl("hours=8&standing=1"));
    const box = await screen.findByTestId("cli-auth-agent-standing");
    const approve = await armedApprove();
    expect(approve).toHaveTextContent(/^Approve$/);

    // Ticked after the button armed: the button now says the larger thing, and is off with it.
    await user.click(box);
    expect(approve).toHaveTextContent(/^Approve with staff access$/);
    expect(approve).toBeDisabled();
    await user.click(approve);
    await attend(600);
    expect(approve).toBeDisabled();
    await user.click(approve);
    expect(mutate).not.toHaveBeenCalled();

    await attend(400);
    expect(approve).toBeEnabled();

    // Cleared: off again, for the same second, in the other direction.
    await user.click(box);
    expect(approve).toHaveTextContent(/^Approve$/);
    expect(approve).toBeDisabled();
    await user.click(approve);
    expect(mutate).not.toHaveBeenCalled();
    await attend();
    expect(approve).toBeEnabled();
    await user.click(approve);
    expect(mutate).toHaveBeenCalledTimes(1);
    expect(standingOf(0)).toBe(false);
  });

  it("says the token will carry none when it was asked for and the person holds none", async () => {
    open(agentUrl("hours=8&standing=1"));
    expect(await screen.findByTestId("cli-auth-agent-standing-none")).toHaveTextContent(
      "Staff or partner access was asked for. You hold neither, so the token will carry none."
    );
    expect(standingBox()).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();

    // Still approvable, and what is asked of the server is what the sheet said.
    const [body] = await approved();
    expect(body.mint).toEqual({ kind: "agent", standing: false, expires_in_hours: 8 });
  });

  it("offers no box when it was not asked for, and tells staff it is left out", async () => {
    tokenOptions = STAFF();
    open(agentUrl("hours=8"));
    expect(await screen.findByTestId("cli-auth-agent-standing-out")).toHaveTextContent(
      "Your staff access is not included."
    );
    expect(standingBox()).not.toBeInTheDocument();
    const [body] = await approved();
    expect(body.mint).toEqual({ kind: "agent", standing: false, expires_in_hours: 8 });
  });

  it("says nothing of standing to a member who was asked for none", async () => {
    open(agentUrl());
    await screen.findByTestId("cli-auth-agent-reach");
    expect(screen.getByTestId("cli-auth-agent-reach")).toHaveTextContent(
      /^Everything you can reach through your organization memberships$/
    );
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });
});

describe("/cli-auth with kind=agent: a request that can't be approved", () => {
  /** Refused on the sheet: the title says so, Approve is off for good, and nothing was asked. */
  const refusedOnSheet = async () => {
    const problem = await screen.findByTestId("cli-auth-mint-problem");
    expect(sheet().tagName).toBe("MAIN");
    expect(within(sheet()).getByRole("heading", { level: 1 })).toHaveTextContent(
      "This request can't be approved as it stands"
    );
    const approve = screen.getByTestId("cli-auth-confirm");
    await attend(5000);
    expect(approve).toBeDisabled();
    await userEvent.setup({ delay: null }).click(approve);
    expect(mutate).not.toHaveBeenCalled();
    // Nothing is on offer, so there is nothing to consent to.
    expect(screen.queryByTestId("cli-auth-mint-powers")).not.toBeInTheDocument();
    expect(screen.queryByTestId("cli-auth-agent-notice")).not.toBeInTheDocument();
    return problem;
  };

  it("marks a lifetime past the limit where it stands", async () => {
    open(agentUrl("hours=500"));
    const problem = await refusedOnSheet();
    expect(problem).toHaveTextContent("Run oxyc again with the request put right.");
    const lifetime = screen.getByTestId("cli-auth-mint-lifetime");
    expect(lifetime).toHaveTextContent("500 hours");
    expect(lifetime).toHaveTextContent("An agent token lasts at most 168 hours (7 days).");
  });

  it.each([
    ["hours=0", "0 hours", "An agent token lasts at least 1 hour."],
    ["hours=soon", '"soon"', "Not a whole number of hours."],
    ["hours=1.5", '"1.5"', "Not a whole number of hours."]
  ])("refuses %s", async (query, shown, why) => {
    open(agentUrl(query));
    await refusedOnSheet();
    const lifetime = screen.getByTestId("cli-auth-mint-lifetime");
    expect(lifetime).toHaveTextContent(shown);
    expect(lifetime).toHaveTextContent(why);
  });

  it("refuses a name longer than a token's may be", async () => {
    open(agentUrl(`hours=8&name=${"x".repeat(101)}`));
    await refusedOnSheet();
    expect(screen.getByTestId("cli-auth-mint-name")).toHaveTextContent(
      "The token's name is longer than 100 characters."
    );
  });

  it("offers no box on a request it refuses, even to staff who asked for standing", async () => {
    tokenOptions = STAFF();
    open(agentUrl("hours=500&standing=1"));
    await refusedOnSheet();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.getByTestId("cli-auth-agent-reach")).toHaveTextContent(
      "The agent asked to include your staff access."
    );
    expect(screen.getByTestId("cli-auth-agent-reach")).not.toHaveTextContent("unless you tick");
  });

  it("refuses on a server that can't create agent tokens, and says running oxyc again won't help yet", async () => {
    tokenOptions = {
      ...MEMBER(),
      data: { orgs: [], can_platform: true, can_partner: false }
    };
    open(agentUrl());
    const problem = await refusedOnSheet();
    expect(problem).toHaveTextContent(
      "Oxygen here can't create agent tokens yet. Run oxyc again once it has been updated."
    );
    // There is no request to lay out: nothing on it is the person's or oxyc's to put right.
    expect(screen.queryByTestId("cli-auth-agent")).not.toBeInTheDocument();
  });

  /** A link the page can't read as one request: refused on the sheet, with nothing to press. */
  const unreadable = (reason: string) => {
    expect(sheet().tagName).toBe("MAIN");
    expect(sheet()).toHaveAttribute("data-status", "error");
    expect(within(sheet()).getByRole("heading", { level: 1 })).toHaveTextContent(
      "Token request failed"
    );
    expect(sheet()).toHaveTextContent(reason);
    expect(within(sheet()).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  };

  it("refuses a kind it doesn't know, and never offers an agent token in its place", () => {
    open(`${PKCE_URL}&kind=agent_admin&hours=8`);
    unreadable("This link asks for a kind of token this page can't approve.");
  });

  it.each([
    ["kind twice", "kind=agent&kind=sandbox_agent&apps=acme/store-ops"],
    ["an agent token with apps", "kind=agent&apps=acme/store-ops&hours=8"],
    ["a sandbox agent token with standing", "kind=sandbox_agent&apps=acme/store-ops&standing=1"]
  ])("refuses a link that asks for both kinds: %s", (_, query) => {
    open(`${PKCE_URL}&${query}`);
    unreadable("This link asks for two kinds of token at once, so nothing on it can be approved.");
  });

  it("never hands the session token to an agent link that lost its code_challenge", async () => {
    open("/cli-auth?port=53124&state=nonce&kind=agent&hours=8");
    unreadable("This link is incomplete.");
    await act(async () => {
      await Promise.resolve();
    });
    expect(leaveTo).not.toHaveBeenCalled();
  });
});

describe("/cli-auth with kind=agent: when the server refuses the approval", () => {
  it("shows the server's words for a request it calls invalid, and stays on the page", async () => {
    open(agentUrl());
    const [, options] = await approved();
    act(() =>
      options.onError?.(
        httpError(400, {
          error: "an agent token lasts 1 to 168 hours",
          code: "invalid_agent_token"
        })
      )
    );
    expect(screen.getByTestId("cli-auth-error")).toHaveTextContent(
      /^an agent token lasts 1 to 168 hours$/
    );
    expect(leaveTo).not.toHaveBeenCalled();
    expect(sheet()).toHaveTextContent("Approve an agent token?");
  });

  it("shows an organization's lifetime cap in the server's words", async () => {
    open(agentUrl());
    const [, options] = await approved();
    const error =
      "the expiry is past the 3-day token lifetime an organization this token reaches allows";
    act(() =>
      options.onError?.(httpError(400, { error, code: "exceeds_policy", max_lifetime_days: 3 }))
    );
    expect(screen.getByTestId("cli-auth-error")).toHaveTextContent(error);
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("treats session_required as a refusal: no code, no redirect, no trip to login", async () => {
    open(agentUrl());
    const [, options] = await approved();
    act(() => options.onError?.(httpError(403, { code: "session_required", error: "no" })));
    expect(screen.getByTestId("cli-auth-error")).toHaveTextContent(
      "Creating a token needs a browser session. Sign in again, then retry."
    );
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("says it couldn't approve when the server fails some other way", async () => {
    open(agentUrl());
    const [, options] = await approved();
    act(() => options.onError?.(httpError(500, { error: "boom" })));
    expect(screen.getByTestId("cli-auth-error")).toHaveTextContent(
      "Couldn't approve the request. Try again, or run the oxyc command again for a new link."
    );
    expect(screen.getByTestId("cli-auth-error")).not.toHaveTextContent("boom");
  });

  it("goes back through login when the session lapsed, to return to the same request", async () => {
    tokenOptions = STAFF();
    open(agentUrl("hours=8&standing=1&name=triage%20run"));
    const [, options] = await approved();
    act(() => options.onError?.(httpError(401)));
    expect(leaveTo).toHaveBeenCalledTimes(1);
    const target = leaveTo.mock.calls[0][0] as string;
    expect(target.startsWith("/login?return_to=")).toBe(true);
    const back = new URL(decodeURIComponent(target.slice("/login?return_to=".length)));
    expect(back.pathname).toBe("/cli-auth");
    expect(back.searchParams.get("kind")).toBe("agent");
    expect(back.searchParams.get("standing")).toBe("1");
    expect(back.searchParams.get("hours")).toBe("8");
    expect(back.searchParams.get("name")).toBe("triage run");
    expect(screen.queryByTestId("cli-auth-error")).not.toBeInTheDocument();
  });
});

describe("/cli-auth with kind=agent: the sheet's rules hold as they do for a sandbox request", () => {
  /** The page's own keydown listener, handed a key as a browser would hand it one. */
  const pageKeydown = () =>
    windowListeners.mock.calls.filter(([type]) => type === "keydown").at(-1)?.[1] as
      | ((event: KeyboardEvent) => void)
      | undefined;
  const pressed = (key: Partial<KeyboardEvent>) =>
    act(() => {
      pageKeydown()?.({
        isTrusted: true,
        repeat: false,
        defaultPrevented: false,
        preventDefault: () => {},
        ...key
      } as KeyboardEvent);
    });

  it("sends a signed-out person to login, to come back to this request and not to a login", async () => {
    session = null;
    open(agentUrl("hours=8&standing=1"));
    await waitFor(() => expect(leaveTo).toHaveBeenCalledTimes(1));
    const target = leaveTo.mock.calls[0][0] as string;
    const back = new URL(decodeURIComponent(target.slice("/login?return_to=".length)));
    expect(back.searchParams.get("code_challenge")).toBe(CHALLENGE);
    expect(back.searchParams.get("kind")).toBe("agent");
    expect(back.searchParams.get("standing")).toBe("1");
  });

  it("checks the session on the sheet, with nothing to press", () => {
    session = new Promise(() => {}) as unknown as CliSession;
    open(agentUrl());
    expect(sheet()).toHaveAttribute("data-status", "working");
    expect(sheet()).toHaveTextContent("Checking your session…");
    expect(within(sheet()).queryByRole("button")).not.toBeInTheDocument();
  });

  it("keeps Approve off for the first second in front of the person", async () => {
    const user = userEvent.setup({ delay: null });
    open(agentUrl());
    const approve = await screen.findByTestId("cli-auth-confirm");
    expect(approve).toBeDisabled();
    await attend(600);
    expect(approve).toBeDisabled();
    await user.click(approve);
    expect(mutate).not.toHaveBeenCalled();
    await attend(400);
    expect(approve).toBeEnabled();
  });

  it("never arms a page that opened in the background", async () => {
    pageFocused.mockReturnValue(false);
    open(agentUrl());
    const approve = await screen.findByTestId("cli-auth-confirm");
    await attend(5000);
    expect(approve).toBeDisabled();
  });

  it("approves on no key, and takes focus nowhere on load", async () => {
    const user = userEvent.setup({ delay: null });
    tokenOptions = STAFF();
    open(agentUrl("hours=8&standing=1"));
    const approve = await armedApprove();
    expect(approve).not.toHaveAttribute("aria-keyshortcuts");
    expect(document.body).toHaveFocus();

    await user.keyboard("{Meta>}{Enter}{/Meta}");
    await user.keyboard("{Enter}");
    await user.keyboard(" ");
    pressed({ key: "Enter", metaKey: true });
    pressed({ key: "Enter" });
    expect(mutate).not.toHaveBeenCalled();
    // A stray key ticked no box either: staff access stays off.
    expect(screen.getByTestId("cli-auth-agent-standing")).toHaveAttribute("aria-checked", "false");
    expect(approve).toHaveTextContent(/^Approve$/);
  });

  it("approves when Approve is focused on purpose and pressed", async () => {
    const user = userEvent.setup({ delay: null });
    open(agentUrl());
    const approve = await armedApprove();
    approve.focus();
    await user.keyboard("{Enter}");
    expect(mutate).toHaveBeenCalledTimes(1);
  });

  it("cancels on Escape and on Cancel, creating nothing", async () => {
    open(agentUrl());
    const cancel = await screen.findByTestId("cli-auth-cancel");
    expect(cancel).toHaveAttribute("aria-keyshortcuts", "Escape");
    await act(async () => {});
    // A script's Escape is not a person's.
    pressed({ key: "Escape", isTrusted: false });
    expect(sheet()).toHaveTextContent("Approve an agent token?");
    pressed({ key: "Escape" });
    expect(sheet()).toHaveAttribute("data-status", "error");
    expect(sheet()).toHaveTextContent(
      "No token was created. oxyc will stop waiting on its own; you can close this tab."
    );
    cleanup();

    const user = userEvent.setup({ delay: null });
    open(agentUrl());
    await user.click(await screen.findByTestId("cli-auth-cancel"));
    expect(sheet()).toHaveTextContent("Request declined");
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("holds every control while the approval is in flight", async () => {
    mutation = { isPending: true, isError: false, error: null };
    tokenOptions = STAFF();
    open(agentUrl("hours=8&standing=1"));
    await screen.findByTestId("cli-auth-confirm");
    await attend();
    expect(pageKeydown()).toBeUndefined();
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    expect(screen.getByTestId("cli-auth-cancel")).toBeDisabled();
    expect(screen.getByTestId("cli-auth-agent-standing")).toBeDisabled();
  });

  it("stays unapprovable while what the person holds loads, and when it can't be loaded", async () => {
    tokenOptions = { isLoading: true, isError: false, refetch: vi.fn() };
    open(agentUrl());
    expect(await screen.findByTestId("cli-auth-mint-loading")).toBeInTheDocument();
    await attend(5000);
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    // The kind of request is already known, so the sheet already says which it is.
    expect(screen.getByTestId("cli-auth-agent-notice")).toBeInTheDocument();
    cleanup();

    const refetch = vi.fn();
    tokenOptions = { isLoading: false, isError: true, refetch };
    const user = userEvent.setup({ delay: null });
    open(agentUrl());
    const failed = await screen.findByTestId("cli-auth-mint-options-error");
    expect(failed).toHaveTextContent(
      "Couldn't load what your account holds, so the request can't be checked."
    );
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    await user.click(within(failed).getByRole("button", { name: "Try again" }));
    expect(refetch).toHaveBeenCalledTimes(1);
  });
});
