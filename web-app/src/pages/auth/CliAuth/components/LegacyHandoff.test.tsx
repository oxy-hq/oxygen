// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LegacyRequest } from "../cliAuthRequest";
import type { CliSession } from "../cliSession";
import LegacyHandoff from "./LegacyHandoff";

vi.setConfig({ testTimeout: 20000 });

// A state a query string must escape, so the handoff is seen to carry it whole.
const REQUEST: LegacyRequest = { kind: "legacy", port: "53124", state: "a b&c" };
const HANDOFF = "http://127.0.0.1:53124/callback?token=session.jwt&state=a%20b%26c";

// Every navigation the card makes goes through `leaveTo`: to the login page, or to the loopback.
const leaveTo = vi.fn();
let readSession: () => Promise<CliSession | null>;
vi.mock("../cliSession", () => ({
  leaveTo: (url: string) => leaveTo(url),
  readCliSession: () => readSession()
}));

const signedInAs = (email?: string) => {
  readSession = () => Promise.resolve({ token: "session.jwt", email });
};

// jsdom never reports its document focused, and the button arms only on a page that is.
const pageFocused = vi.spyOn(document, "hasFocus");
const windowListeners = vi.spyOn(window, "addEventListener");

beforeEach(() => {
  // Advancing with real time keeps `findBy` and `waitFor` working under the fake clock.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  pageFocused.mockReturnValue(true);
  signedInAs("luong@example.com");
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  windowListeners.mockClear();
  leaveTo.mockReset();
});

const open = () => render(<LegacyHandoff request={REQUEST} />);
const card = () => screen.getByTestId("cli-auth-card");
const handOverButton = () => screen.findByTestId("cli-auth-confirm");

/** Time with the card in front of the person. A full second of it arms the button. */
const attend = async (ms = 1000) => {
  // First whatever the last render left to run, so the count has started before time moves.
  await act(async () => {});
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
};

describe("LegacyHandoff, signed in: it asks before anything is sent", () => {
  it("says what is asked and by what, and sends nothing while it waits", async () => {
    open();
    const handOver = await handOverButton();
    expect(card()).toHaveAttribute("data-status", "confirm");
    expect(card()).toHaveTextContent("A program on this computer is asking for your sign-in");
    expect(card()).toHaveTextContent(
      "Whatever receives it can do everything you can as luong@example.com, for as long as the sign-in lasts."
    );
    expect(screen.getByTestId("cli-auth-caution")).toHaveTextContent(
      "Continue only if you just ran oxyc login or oxy login on this computer."
    );
    expect(screen.getByTestId("cli-auth-upgrade")).toHaveTextContent(
      "A current oxyc asks for a token you can revoke instead. To get it, run npm i -g @oxy-hq/cli@latest."
    );
    expect(handOver).toHaveTextContent("Hand over sign-in");

    // However long the card sits there, armed or not, nothing leaves the page.
    await attend(60_000);
    expect(handOver).toBeEnabled();
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("sends the session token to the loopback, with the request's state, on the click", async () => {
    const user = userEvent.setup({ delay: null });
    open();
    const handOver = await handOverButton();
    await attend();
    await user.click(handOver);

    expect(leaveTo).toHaveBeenCalledTimes(1);
    expect(leaveTo).toHaveBeenCalledWith(HANDOFF);
    expect(card()).toHaveAttribute("data-status", "done");
    expect(card()).toHaveTextContent("Login complete");
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
  });

  it("sends nothing when the person cancels, and says so", async () => {
    const user = userEvent.setup({ delay: null });
    open();
    await handOverButton();
    await attend();
    await user.click(screen.getByTestId("cli-auth-cancel"));

    expect(card()).toHaveTextContent("Login cancelled");
    expect(card()).toHaveTextContent("Nothing was sent.");
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
    await attend(60_000);
    expect(leaveTo).not.toHaveBeenCalled();
  });

  it("names the account as one word, and no computer: a request carries no hostname", async () => {
    open();
    const account = await screen.findByTestId("cli-auth-approver");
    expect(account).toHaveTextContent("luong@example.com");
    expect(account).toHaveClass("break-words");
    expect(account).not.toHaveClass("break-all");
    expect(screen.queryByTestId("cli-auth-hostname")).not.toBeInTheDocument();
  });

  it("names no account when the session has no email to show", async () => {
    signedInAs(undefined);
    open();
    await handOverButton();
    expect(screen.queryByTestId("cli-auth-approver")).not.toBeInTheDocument();
    expect(card()).toHaveTextContent(
      "Whatever receives it can do everything you can, for as long as the sign-in lasts."
    );
  });
});

describe("LegacyHandoff: the button arms after a second in front of the person", () => {
  it("is off for the first second, so a click already on its way lands on nothing", async () => {
    const user = userEvent.setup({ delay: null });
    open();
    const handOver = await handOverButton();
    expect(handOver).toBeDisabled();
    await user.click(handOver);
    await attend(600);
    expect(handOver).toBeDisabled();
    await user.click(handOver);
    // Not even a click the browser would not deliver to a disabled button.
    fireEvent.click(handOver);
    expect(leaveTo).not.toHaveBeenCalled();

    await attend(400);
    expect(handOver).toBeEnabled();
    await user.click(handOver);
    expect(leaveTo).toHaveBeenCalledWith(HANDOFF);
  });

  it("never arms a page that opened in the background", async () => {
    const user = userEvent.setup({ delay: null });
    pageFocused.mockReturnValue(false);
    open();
    const handOver = await handOverButton();
    await attend(5000);
    expect(handOver).toBeDisabled();
    await user.click(handOver);
    expect(leaveTo).not.toHaveBeenCalled();

    pageFocused.mockReturnValue(true);
    fireEvent.focus(window);
    expect(handOver).toBeDisabled();
    await attend();
    expect(handOver).toBeEnabled();
  });

  it("turns off when the window loses focus, and counts a fresh second on return", async () => {
    open();
    const handOver = await handOverButton();
    await attend();
    expect(handOver).toBeEnabled();

    pageFocused.mockReturnValue(false);
    fireEvent.blur(window);
    expect(handOver).toBeDisabled();
    await attend(5000);
    expect(handOver).toBeDisabled();

    pageFocused.mockReturnValue(true);
    fireEvent.focus(window);
    await attend(600);
    expect(handOver).toBeDisabled();
    await attend(400);
    expect(handOver).toBeEnabled();
  });
});

describe("LegacyHandoff, from the keyboard", () => {
  /**
   * The card's own keydown listener, handed a key as a browser would hand it one. Every event a
   * test dispatches is untrusted, as a script's is, and the card takes none of those: so the one
   * way to stand for a person's keypress is to call the listener.
   */
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
        ...key
      } as KeyboardEvent);
    });

  it("takes focus nowhere on load, and hands over on no key", async () => {
    const user = userEvent.setup({ delay: null });
    open();
    const handOver = await handOverButton();
    expect(handOver).not.toHaveAttribute("aria-keyshortcuts");
    await attend();
    expect(handOver).toBeEnabled();
    expect(document.body).toHaveFocus();

    await user.keyboard("{Enter}");
    await user.keyboard(" ");
    await user.keyboard("{Meta>}{Enter}{/Meta}");
    await user.keyboard("{Control>}{Enter}{/Control}");
    // The same, as keys a person really pressed.
    pressed({ key: "Enter" });
    pressed({ key: "Enter", metaKey: true });
    pressed({ key: "Enter", ctrlKey: true });
    expect(leaveTo).not.toHaveBeenCalled();
    expect(card()).toHaveAttribute("data-status", "confirm");
  });

  it("hands over when the button is focused on purpose and pressed, as any button is", async () => {
    const user = userEvent.setup({ delay: null });
    open();
    const handOver = await handOverButton();
    await attend();

    handOver.focus();
    await user.keyboard("{Enter}");
    expect(leaveTo).toHaveBeenCalledWith(HANDOFF);
  });

  it("cancels on Escape, sending nothing, and shows the key on Cancel", async () => {
    const user = userEvent.setup({ delay: null });
    open();
    const cancel = await screen.findByTestId("cli-auth-cancel");
    expect(cancel).toHaveAttribute("aria-keyshortcuts", "Escape");
    // The hint is decoration: the button is still named "Cancel".
    expect(screen.getByRole("button", { name: "Cancel" })).toBe(cancel);
    // The listener is registered in an effect, which may not have run when the button is found.
    await act(async () => {});

    // What a test dispatches is what a script dispatches: untrusted, and so not a decision.
    await user.keyboard("{Escape}");
    pressed({ key: "Escape", isTrusted: false });
    pressed({ key: "Escape", repeat: true });
    expect(card()).toHaveAttribute("data-status", "confirm");

    pressed({ key: "Escape" });
    expect(card()).toHaveTextContent("Nothing was sent.");
    expect(leaveTo).not.toHaveBeenCalled();
  });
});

describe("LegacyHandoff, signed out", () => {
  const loginAndBack = `/login?return_to=${encodeURIComponent(
    `${window.location.origin}/cli-auth?port=53124&state=a%20b%26c`
  )}`;

  it("goes to login, to come back to the same request, and asks nothing", async () => {
    readSession = () => Promise.resolve(null);
    open();
    await waitFor(() => expect(leaveTo).toHaveBeenCalledTimes(1));
    expect(leaveTo).toHaveBeenCalledWith(loginAndBack);
    expect(card()).toHaveAttribute("data-status", "working");
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
  });

  it("goes to login when the session can't be read", async () => {
    readSession = () => Promise.reject(new Error("offline"));
    open();
    await waitFor(() => expect(leaveTo).toHaveBeenCalledTimes(1));
    expect(leaveTo).toHaveBeenCalledWith(loginAndBack);
    expect(screen.queryByTestId("cli-auth-confirm")).not.toBeInTheDocument();
  });
});
