// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SandboxApp, TokenOptions } from "@/types/apiToken";
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

const sandboxApp = (id: string, org: string, slug: string, name: string): SandboxApp => ({
  id,
  org_id: `org-${org}`,
  org_slug: org,
  org_name: org.charAt(0).toUpperCase() + org.slice(1),
  slug,
  name
});

const mintOptions = (apps: SandboxApp[]): TokenOptions => ({
  orgs: [],
  can_platform: true,
  can_partner: false,
  sandbox_agent: { default_hours: 8, max_hours: 168, max_apps: 5 },
  sandbox_apps: apps
});

const MINTABLE = [
  sandboxApp("a1", "acme", "store-ops", "Store Ops"),
  sandboxApp("a4", "globex", "pos", "POS")
];

type OptionsQuery = {
  data?: TokenOptions;
  isLoading: boolean;
  isError: boolean;
  refetch: () => void;
};
const loaded = (apps: SandboxApp[] = MINTABLE): OptionsQuery => ({
  data: mintOptions(apps),
  isLoading: false,
  isError: false,
  refetch: vi.fn()
});
let tokenOptions: OptionsQuery = loaded();
// Only the mint approval reads this: what the signed-in person may mint for.
vi.mock("@/hooks/api/userTokens/useUserTokens", () => ({
  useTokenOptions: () => tokenOptions
}));

// jsdom never reports its document focused. A mint approval counts a second of being visible and
// focused before Approve can be pressed, so a test says which the page is.
const pageFocused = vi.spyOn(document, "hasFocus");
const windowListeners = vi.spyOn(window, "addEventListener");

beforeEach(() => {
  // Advancing with real time keeps `findBy` and `waitFor` working under the fake clock.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  pageFocused.mockReturnValue(true);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  windowListeners.mockClear();
  Reflect.deleteProperty(document, "visibilityState");
  leaveTo.mockReset();
  mutate.mockReset();
  session = { token: "session.jwt", email: "luong@example.com" };
  mutation = { isPending: false, isError: false, error: null };
  tokenOptions = loaded();
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

const mintUrl = (apps = "acme/store-ops,globex/pos", hours = "8") =>
  `${PKCE_URL}&kind=sandbox_agent&apps=${apps}&hours=${hours}&name=refunds%20task`;

const httpError = (status: number, data: unknown = {}) =>
  new AxiosError("Request failed", "ERR", undefined, undefined, { status, data } as AxiosResponse);

const appLine = (ref: string) =>
  screen
    .getAllByTestId("cli-auth-mint-app")
    .find((line) => line.dataset.appRef === ref) as HTMLElement;

/** Time with the request in front of the person. A full second of it arms Approve. */
const attend = async (ms = 1000) => {
  // First whatever the last render left to run, so the count has started before time moves.
  await act(async () => {});
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
};

/** Approve, once the page has counted its second. */
const armedApprove = async () => {
  const approve = await screen.findByTestId("cli-auth-confirm");
  await attend();
  expect(approve).toBeEnabled();
  return approve;
};

describe("/cli-auth with kind=sandbox_agent (a CLI mint)", () => {
  it("shows the computer asking, the apps and the lifetime, and waits for a click", async () => {
    open(mintUrl());
    expect(await screen.findByTestId("cli-auth-hostname")).toHaveTextContent("luong-mbp");
    const card = screen.getByTestId("cli-auth-card");
    expect(card).toHaveTextContent("Approve a sandbox agent token?");
    expect(card).toHaveTextContent("for an agent that will act as luong@example.com.");

    // The slugs oxyc sent, beside the apps they name.
    expect(appLine("acme/store-ops")).toHaveTextContent("acme/store-ops");
    expect(appLine("acme/store-ops")).toHaveTextContent("Store Ops in Acme");
    expect(appLine("globex/pos")).toHaveTextContent("POS in Globex");
    const lifetime = screen.getByTestId("cli-auth-mint-lifetime");
    expect(lifetime).toHaveTextContent(/^8 hours/);
    expect(lifetime).toHaveTextContent(/until \w+ \d+, \d{4}/);
    expect(screen.getByTestId("cli-auth-mint-name")).toHaveTextContent("refunds task");

    // What it can and cannot do, one act per line, then the command that should have led here.
    const [can, cannot] = within(screen.getByTestId("cli-auth-mint-powers")).getAllByRole("list");
    expect(
      within(can)
        .getAllByRole("listitem")
        .map((act) => act.textContent)
    ).toEqual([
      "Create up to three dev sandboxes of these apps",
      "Publish into them",
      "Call their functions",
      "Run their checks",
      "Read their logs",
      "Set their secrets"
    ]);
    expect(within(cannot).getAllByRole("listitem")).toHaveLength(5);
    expect(cannot).toHaveTextContent("Reach production or staging");
    expect(card).toHaveTextContent("Continue only if you just ran this on luong-mbp");
    expect(card).toHaveTextContent("oxyc tokens create --sandbox-agent");

    await armedApprove();
    expect(screen.queryByTestId("cli-auth-mint-problem")).not.toBeInTheDocument();
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("approves with the mint object, then sends oxyc a code as a login does", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await user.click(await armedApprove());

    expect(mutate).toHaveBeenCalledTimes(1);
    const [body, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    expect(body).toEqual({
      code_challenge: CHALLENGE,
      hostname: "luong-mbp",
      mint: { kind: "sandbox_agent", apps: ["a1", "a4"], expires_in_hours: 8, name: "refunds task" }
    });

    act(() => options.onSuccess?.({ code: "single-use" }));
    expect(leaveTo).toHaveBeenCalledWith(
      "http://127.0.0.1:53124/callback?code=single-use&state=nonce"
    );
    expect(leaveTo.mock.calls.flat().join(" ")).not.toContain("session.jwt");
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("Token approved");
  });

  it("won't approve an app it can't resolve, and says which", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl("acme/store-ops,acme/ghost"));
    expect(await screen.findByTestId("cli-auth-mint-problem")).toHaveTextContent(
      "Run oxyc again with the request put right."
    );
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent(
      "This request can't be approved as it stands"
    );
    // The reason sits on the app that is wrong, and on no other.
    expect(appLine("acme/ghost")).toHaveAttribute("data-resolved", "false");
    expect(appLine("acme/ghost")).toHaveTextContent(
      "Not found, or not one you can create a token for."
    );
    expect(appLine("acme/store-ops")).toHaveAttribute("data-resolved", "true");
    expect(appLine("acme/store-ops")).not.toHaveTextContent("Not found");
    // Nothing is on offer, so there is nothing to consent to.
    expect(screen.queryByTestId("cli-auth-mint-powers")).not.toBeInTheDocument();

    const approve = screen.getByTestId("cli-auth-confirm");
    expect(approve).toBeDisabled();
    await user.click(approve);
    expect(mutate).not.toHaveBeenCalled();
  });

  it("won't approve a lifetime or an app count out of range, and says why", async () => {
    open(mintUrl("acme/store-ops", "500"));
    await screen.findByTestId("cli-auth-mint-problem");
    const lifetime = screen.getByTestId("cli-auth-mint-lifetime");
    expect(lifetime).toHaveTextContent("500 hours");
    expect(lifetime).toHaveTextContent("A sandbox agent token lasts at most 168 hours (7 days).");
    expect(screen.queryByTestId("cli-auth-mint-app-count")).not.toBeInTheDocument();
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    cleanup();

    const six = ["a", "b", "c", "d", "e", "f"].map((slug, index) =>
      sandboxApp(`id-${index}`, "acme", slug, slug.toUpperCase())
    );
    tokenOptions = loaded(six);
    open(mintUrl(six.map((each) => `acme/${each.slug}`).join(",")));
    const count = await screen.findByTestId("cli-auth-mint-app-count");
    expect(count).toHaveTextContent("6 apps");
    expect(count).toHaveTextContent("A sandbox agent token covers at most 5 apps.");
    expect(screen.getByTestId("cli-auth-mint-problem")).toBeInTheDocument();
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
  });

  it("approves nothing for someone who may mint for no app", async () => {
    tokenOptions = loaded([]);
    open(mintUrl());
    await screen.findByTestId("cli-auth-mint-problem");
    for (const ref of ["acme/store-ops", "globex/pos"]) {
      expect(appLine(ref)).toHaveAttribute("data-resolved", "false");
      expect(appLine(ref)).toHaveTextContent("Not found, or not one you can create a token for.");
    }
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
  });

  it("stays unapprovable while the apps load, and when they can't be loaded", async () => {
    tokenOptions = { isLoading: true, isError: false, refetch: vi.fn() };
    open(mintUrl());
    expect(await screen.findByTestId("cli-auth-mint-loading")).toBeInTheDocument();
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    cleanup();

    const refetch = vi.fn();
    tokenOptions = { isLoading: false, isError: true, refetch };
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    const failed = await screen.findByTestId("cli-auth-mint-options-error");
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    await user.click(within(failed).getByRole("button", { name: "Try again" }));
    expect(refetch).toHaveBeenCalledTimes(1);
  });

  it("mints nothing when the person cancels", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await user.click(await screen.findByTestId("cli-auth-cancel"));
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("No token was created");
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("sends a signed-out person to login, to come back to the same mint and not to a login", async () => {
    session = null;
    open(mintUrl());
    await waitFor(() => expect(leaveTo).toHaveBeenCalledTimes(1));
    const target = leaveTo.mock.calls[0][0] as string;
    expect(target.startsWith("/login?return_to=")).toBe(true);
    const returnTo = new URL(decodeURIComponent(target.slice("/login?return_to=".length)));
    expect(returnTo.pathname).toBe("/cli-auth");
    expect(returnTo.searchParams.get("code_challenge")).toBe(CHALLENGE);
    expect(returnTo.searchParams.get("kind")).toBe("sandbox_agent");
    expect(returnTo.searchParams.get("apps")).toBe("acme/store-ops,globex/pos");
    expect(returnTo.searchParams.get("hours")).toBe("8");
    expect(returnTo.searchParams.get("name")).toBe("refunds task");
  });

  it("names the app the server refuses at approval, and stays on the page", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await user.click(await armedApprove());
    const [, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    act(() => options.onError?.(httpError(404, { code: "app_not_found", app_id: "a4" })));

    expect(screen.getByTestId("cli-auth-error")).toHaveTextContent(
      "Oxygen couldn't find POS in Globex for you."
    );
    expect(screen.getByTestId("cli-auth-error")).toHaveTextContent("Run oxyc again without it.");
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("says in the server's words when an organization's lifetime cap refuses the approval", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await user.click(await armedApprove());
    const [, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    act(() =>
      options.onError?.(
        httpError(400, {
          error:
            "the expiry is past the 3-day token lifetime an organization this token reaches allows",
          code: "exceeds_policy",
          max_lifetime_days: 3
        })
      )
    );

    expect(screen.getByTestId("cli-auth-error")).toHaveTextContent(
      "the expiry is past the 3-day token lifetime an organization this token reaches allows"
    );
    // No code was issued, so nothing goes to oxyc.
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("goes back through login when the session lapsed before the click", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await user.click(await armedApprove());
    const [, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    act(() => options.onError?.(httpError(401)));
    const target = leaveTo.mock.calls[0][0] as string;
    expect(target).toContain("/login?return_to=");
    expect(decodeURIComponent(target)).toContain("kind=sandbox_agent");
    expect(screen.queryByTestId("cli-auth-error")).not.toBeInTheDocument();
  });
});

const MINT = {
  kind: "sandbox_agent",
  apps: ["a1", "a4"],
  expires_in_hours: 8,
  name: "refunds task"
};

const stillAsking = () =>
  expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("Approve a sandbox agent token?");

describe("/cli-auth with kind=sandbox_agent: every state is the same sheet", () => {
  /** The sheet: a page with one heading under the product's name, and no card around it. */
  const sheet = (status: string, title: string) => {
    const page = screen.getByTestId("cli-auth-card");
    expect(page).toHaveAttribute("data-status", status);
    expect(page.tagName).toBe("MAIN");
    expect(page).toHaveTextContent(/^Oxygen/);
    expect(within(page).getByRole("heading", { level: 1 })).toHaveTextContent(title);
    return page;
  };

  it("checks the session on the sheet, with nothing to press", async () => {
    // A session that is still being read.
    session = new Promise(() => {}) as unknown as CliSession;
    open(mintUrl());
    const page = sheet("working", "Checking your session…");
    expect(page).toHaveTextContent("Making sure you are signed in before oxyc asks for a token.");
    expect(within(page).queryByRole("button")).not.toBeInTheDocument();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("asks on the sheet, then says approved on it", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await screen.findByTestId("cli-auth-confirm");
    sheet("confirm", "Approve a sandbox agent token?");

    await user.click(await armedApprove());
    const [, options] = mutate.mock.calls[0] as [unknown, MutateOptions];
    act(() => options.onSuccess?.({ code: "single-use" }));
    const page = sheet("done", "Token approved");
    expect(page).toHaveTextContent(
      "Returning to your terminal, where oxyc prints the token once. You can close this tab."
    );
    expect(within(page).queryByRole("button")).not.toBeInTheDocument();
  });

  it("says declined on the sheet", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await user.click(await screen.findByTestId("cli-auth-cancel"));
    const page = sheet("error", "Request declined");
    expect(page).toHaveTextContent(
      "No token was created. oxyc will stop waiting on its own; you can close this tab."
    );
    expect(within(page).queryByRole("button")).not.toBeInTheDocument();
  });

  it("leaves a login on its card: only the mint is a sheet", async () => {
    open(PKCE_URL);
    await screen.findByTestId("cli-auth-confirm");
    expect(screen.getByTestId("cli-auth-card").tagName).not.toBe("MAIN");
  });
});

describe("/cli-auth with kind=sandbox_agent: Approve arms after a second in front of the person", () => {
  const leaveWindow = () => {
    pageFocused.mockReturnValue(false);
    fireEvent.blur(window);
  };
  const returnToWindow = () => {
    pageFocused.mockReturnValue(true);
    fireEvent.focus(window);
  };
  const showTab = (visible: boolean) => {
    Object.defineProperty(document, "visibilityState", {
      value: visible ? "visible" : "hidden",
      configurable: true
    });
    fireEvent(document, new Event("visibilitychange"));
  };

  it("is off for the first second, so a click already on its way lands on nothing", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    const approve = await screen.findByTestId("cli-auth-confirm");
    expect(approve).toBeDisabled();
    await attend(600);
    expect(approve).toBeDisabled();
    await user.click(approve);
    expect(mutate).not.toHaveBeenCalled();

    await attend(400);
    expect(approve).toBeEnabled();
    await user.click(approve);
    expect(mutate).toHaveBeenCalledTimes(1);
    expect((mutate.mock.calls[0] as [{ mint: unknown }])[0].mint).toEqual(MINT);
  });

  it("turns off when the window loses focus, and counts a fresh second on return", async () => {
    open(mintUrl());
    const approve = await armedApprove();

    leaveWindow();
    expect(approve).toBeDisabled();
    // Away for as long as it likes: the second is of attention, not of time.
    await attend(5000);
    expect(approve).toBeDisabled();

    returnToWindow();
    expect(approve).toBeDisabled();
    await attend(600);
    expect(approve).toBeDisabled();
    await attend(400);
    expect(approve).toBeEnabled();
  });

  it("turns off when the tab is hidden, and counts a fresh second once it shows", async () => {
    open(mintUrl());
    const approve = await armedApprove();

    showTab(false);
    expect(approve).toBeDisabled();
    await attend(5000);
    expect(approve).toBeDisabled();

    showTab(true);
    expect(approve).toBeDisabled();
    await attend(600);
    expect(approve).toBeDisabled();
    await attend(400);
    expect(approve).toBeEnabled();
  });

  it("never arms a page that opened in the background", async () => {
    pageFocused.mockReturnValue(false);
    open(mintUrl());
    const approve = await screen.findByTestId("cli-auth-confirm");
    await attend(5000);
    expect(approve).toBeDisabled();

    returnToWindow();
    await attend();
    expect(approve).toBeEnabled();
  });

  it("never arms a request that can't be approved", async () => {
    open(mintUrl("acme/store-ops,acme/ghost"));
    const approve = await screen.findByTestId("cli-auth-confirm");
    await attend(5000);
    expect(approve).toBeDisabled();
  });
});

describe("/cli-auth with kind=sandbox_agent, from the keyboard", () => {
  /**
   * The page's own keydown listener, handed a key as a browser would hand it one. Every event a
   * test dispatches is untrusted, as a script's is, and the page takes none of those: so the
   * one way to stand for a person's keypress is to call the listener.
   */
  const pageKeydown = () =>
    windowListeners.mock.calls.filter(([type]) => type === "keydown").at(-1)?.[1] as
      | ((event: KeyboardEvent) => void)
      | undefined;
  /**
   * Whatever the last render left to run. The page registers its listener in an effect, and
   * under fake timers that effect may not have run yet when the button is first found.
   */
  const settled = () => act(async () => {});
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

  it("approves on no chord, before the second is up or after it", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    const approve = await screen.findByTestId("cli-auth-confirm");
    // No shortcut is offered: none on the button, none announced.
    expect(approve).not.toHaveAttribute("aria-keyshortcuts");
    expect(screen.queryByTestId("submit-chord-hint")).not.toBeInTheDocument();

    const chords = async () => {
      await user.keyboard("{Meta>}{Enter}{/Meta}");
      await user.keyboard("{Control>}{Enter}{/Control}");
      await user.keyboard("{Enter}");
      // The same, as keys a person really pressed.
      pressed({ key: "Enter", metaKey: true });
      pressed({ key: "Enter", ctrlKey: true });
      pressed({ key: "Enter" });
    };
    await chords();
    expect(mutate).not.toHaveBeenCalled();

    await attend();
    expect(approve).toBeEnabled();
    await chords();
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
    stillAsking();
  });

  it("takes focus nowhere on load, so a stray key reaches no button", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await armedApprove();
    expect(document.body).toHaveFocus();

    await user.keyboard("{Enter}");
    await user.keyboard(" ");
    expect(mutate).not.toHaveBeenCalled();
    stillAsking();
  });

  it("approves when Approve is focused on purpose and pressed, as any button is", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    const approve = await armedApprove();

    approve.focus();
    await user.keyboard("{Enter}");
    expect(mutate).toHaveBeenCalledTimes(1);
    expect((mutate.mock.calls[0] as [{ mint: unknown }])[0].mint).toEqual(MINT);
  });

  it("cancels on Escape, minting nothing, and shows the key on Cancel", async () => {
    open(mintUrl());
    const cancel = await screen.findByTestId("cli-auth-cancel");
    expect(cancel).toHaveAttribute("aria-keyshortcuts", "Escape");
    expect(cancel).toHaveTextContent("Esc");
    // The hint is decoration: the button is still named "Cancel".
    expect(screen.getByRole("button", { name: "Cancel" })).toBe(cancel);

    await settled();
    pressed({ key: "Escape" });
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("No token was created");
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("ignores an Escape a script sent, and one from a held key", async () => {
    const user = userEvent.setup({ delay: null });
    open(mintUrl());
    await screen.findByTestId("cli-auth-cancel");
    await settled();

    // What a test dispatches is what a script dispatches: untrusted.
    await user.keyboard("{Escape}");
    stillAsking();
    pressed({ key: "Escape", isTrusted: false });
    stillAsking();
    pressed({ key: "Escape", repeat: true });
    stillAsking();
    pressed({ key: "Escape", defaultPrevented: true });
    stillAsking();

    // The listener the cases above reached is the live one.
    pressed({ key: "Escape" });
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("No token was created");
  });

  it("listens for no key while a request is in flight", async () => {
    mutation = { isPending: true, isError: false, error: null };
    open(mintUrl());
    await screen.findByTestId("cli-auth-confirm");
    await attend();

    expect(pageKeydown()).toBeUndefined();
    expect(screen.getByTestId("cli-auth-confirm")).toBeDisabled();
    expect(screen.getByTestId("cli-auth-cancel")).toBeDisabled();
    stillAsking();
  });
});

describe("/cli-auth with a kind it can't mint", () => {
  it("refuses an unknown kind instead of offering a login in its place", () => {
    open(`${PKCE_URL}&kind=service_account&apps=acme/store-ops`);
    const card = screen.getByTestId("cli-auth-card");
    expect(card).toHaveAttribute("data-status", "error");
    expect(card).toHaveTextContent("Token request failed");
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
    expect(mutate).not.toHaveBeenCalled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("never hands the session token to a mint link that lost its code_challenge", async () => {
    open(`${LEGACY_URL}&kind=sandbox_agent&apps=acme/store-ops`);
    expect(screen.getByTestId("cli-auth-card")).toHaveTextContent("Token request failed");
    // The legacy handoff navigates on its own once the session is read; give it the chance to.
    await act(async () => {
      await Promise.resolve();
    });
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
