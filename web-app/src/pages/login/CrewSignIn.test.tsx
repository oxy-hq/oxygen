// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FrontlineService } from "@/services/api";
import type { BoundKioskDevice, FrontlineStaff } from "@/types/frontline";
import CrewSignIn from "./CrewSignIn";
import { readRecentCrew, recentCrewKey } from "./recentCrew";

vi.mock("@/services/api", () => ({
  AuthService: { validateReturnTo: vi.fn() },
  FrontlineService: { login: vi.fn() }
}));

const login = vi.mocked(FrontlineService.login);

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
});

const CLOVIS = { id: "loc-clovis", name: "Clovis" };

const kiosk = (location: BoundKioskDevice["location"]): BoundKioskDevice => ({
  bound: true,
  org: "poke",
  orgName: "Poke",
  device: "Front counter",
  location,
  returnTo: null
});

const MARIA: FrontlineStaff = { identifier: "maria", name: "Maria" };
const DEVON: FrontlineStaff = { identifier: "devon", name: "Devon" };

const signInPage = (
  device: BoundKioskDevice,
  staff: FrontlineStaff[],
  { isRosterError = false }: { isRosterError?: boolean } = {}
) =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <CrewSignIn
        device={device}
        staff={staff}
        isRosterLoading={false}
        isRosterError={isRosterError}
        adminSignIn={<button type='button'>Sign in as an admin</button>}
      />
    </QueryClientProvider>
  );

const httpError = (status: number) =>
  Object.assign(new Error(`${status}`), { response: { status } });

/** Tap a name on the board and hand back the PIN step it opens. */
const pick = (identifier: string) => {
  fireEvent.click(screen.getByTestId(`login-crew-staff-${identifier}`));
  return screen.getByTestId("login-crew-pin-step");
};

const pinBox = () => screen.getByTestId("login-crew-pin") as HTMLInputElement;

const tapKeys = (step: HTMLElement, ...keys: string[]) => {
  for (const key of keys) {
    fireEvent.click(within(step).getByRole("button", { name: key }));
  }
};

describe("CrewSignIn", () => {
  it("tells a store's tablet with nobody on its roster so, instead of an ID box that cannot work", () => {
    // A store's tablet signs in exactly the people its picker shows. With nobody
    // rostered there, an ID and a right PIN typed into the box read as "that
    // didn't match" every time — the worker's own store included, once the
    // lockout it used to cost had armed.
    signInPage(kiosk(CLOVIS), []);

    expect(screen.queryByTestId("login-crew-identifier")).toBeNull();
    expect(screen.queryByTestId("login-crew-pin")).toBeNull();
    expect(screen.queryByTestId("login-crew-submit")).toBeNull();
    const note = screen.getByTestId("login-crew-nobody-here");
    expect(note).toHaveTextContent("Nobody is set up to sign in at Clovis yet.");
    expect(note).toHaveTextContent(/manager has to add the crew to this store/);
    expect(screen.getByText("Crew sign-in isn't ready on this tablet yet")).toBeTruthy();
  });

  it("keeps the ID box on a kiosk with no place, whose sign-in admits the whole org", () => {
    // `null`: enrolled without a place. `undefined`: a server older than the
    // operating graph, which never narrowed anything.
    for (const location of [null, undefined]) {
      signInPage(kiosk(location), []);
      expect(screen.getByTestId("login-crew-identifier")).toBeTruthy();
      expect(screen.getByTestId("login-crew-pin")).toBeTruthy();
      expect(screen.getByText("Enter your ID and PIN")).toBeTruthy();
      expect(screen.queryByTestId("login-crew-nobody-here")).toBeNull();
      cleanup();
    }
  });

  it("keeps the ID box when a store's roster could not be read, since its crew can still sign in", () => {
    // A failed read says nothing about who works here. The box is the way in
    // for the store's own crew until the picker loads again.
    signInPage(kiosk(CLOVIS), [], { isRosterError: true });
    expect(screen.getByTestId("login-crew-identifier")).toBeTruthy();
    expect(screen.queryByTestId("login-crew-nobody-here")).toBeNull();
  });

  it("signs in through the ID box with the ID and PIN as typed", async () => {
    login.mockResolvedValue({ token: "t", name: "Maria", expires_in: 60 });
    signInPage(kiosk(null), []);
    await userEvent.type(screen.getByTestId("login-crew-identifier"), "maria");
    await userEvent.type(pinBox(), "4821");
    fireEvent.click(screen.getByTestId("login-crew-submit"));
    await waitFor(() =>
      expect(login.mock.calls[0]?.[0]).toEqual({ org: "poke", identifier: "maria", pin: "4821" })
    );
  });

  it("shows a store's crew as names to tap, and asks for a PIN only once one is tapped", () => {
    // Guard: the explanation is for an EMPTY roster, not every store's tablet.
    signInPage(kiosk(CLOVIS), [MARIA, DEVON]);
    expect(screen.getByTestId("login-crew-staff-maria")).toBeTruthy();
    expect(screen.getByText("Tap your name and enter your PIN")).toBeTruthy();
    expect(screen.queryByTestId("login-crew-nobody-here")).toBeNull();
    expect(screen.queryByTestId("login-crew-identifier")).toBeNull();
    // Nothing to type a PIN for yet, so nothing takes room from the names.
    expect(screen.queryByTestId("login-crew-pin")).toBeNull();
    expect(screen.queryByTestId("login-crew-submit")).toBeNull();

    const step = pick("maria");
    expect(step).toHaveTextContent("Signing in as");
    expect(step).toHaveTextContent("Maria");
    expect(step).toHaveTextContent("maria");
    expect(within(step).getByTestId("login-crew-pin")).toHaveAttribute("inputmode", "numeric");
    expect(within(step).getByTestId("login-crew-submit")).toHaveTextContent("Sign in");
    for (const key of ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "Delete", "Clear"]) {
      expect(within(step).getByRole("button", { name: key })).toBeTruthy();
    }
  });

  it("keeps the admin's way in at the top of the screen", () => {
    signInPage(kiosk(CLOVIS), [MARIA]);
    expect(screen.getByRole("button", { name: "Sign in as an admin" })).toBeTruthy();
  });

  it("moves focus into the PIN step, and back to the name when it is not you", () => {
    signInPage(kiosk(CLOVIS), [MARIA, DEVON]);
    const step = pick("maria");
    expect(step.contains(document.activeElement)).toBe(true);

    fireEvent.click(within(step).getByRole("button", { name: "Not you? Choose another name" }));
    expect(screen.queryByTestId("login-crew-pin-step")).toBeNull();
    expect(screen.getByTestId("login-crew-staff-maria")).toHaveAttribute("data-state", "off");
    expect(document.activeElement).toBe(screen.getByTestId("login-crew-staff-maria"));
  });

  it("closes the PIN step on Escape", () => {
    signInPage(kiosk(CLOVIS), [MARIA]);
    const step = pick("maria");
    fireEvent.keyDown(step, { key: "Escape" });
    expect(screen.queryByTestId("login-crew-pin-step")).toBeNull();
  });

  it("types a PIN on the keypad, and Delete and Clear take it back", () => {
    signInPage(kiosk(CLOVIS), [MARIA]);
    const step = pick("maria");
    tapKeys(step, "4", "8", "2", "1");
    expect(pinBox().value).toBe("4821");
    tapKeys(step, "Delete");
    expect(pinBox().value).toBe("482");
    tapKeys(step, "Clear");
    expect(pinBox().value).toBe("");
  });

  it("stops the keypad at the longest PIN there is", () => {
    signInPage(kiosk(CLOVIS), [MARIA]);
    const step = pick("maria");
    tapKeys(step, ..."1234567890".split(""));
    expect(pinBox().value).toBe("12345678");
  });

  it("still takes a PIN typed on a keyboard, into the box or anywhere in the step", async () => {
    login.mockResolvedValue({ token: "t", name: "Maria", expires_in: 60 });
    signInPage(kiosk(CLOVIS), [MARIA]);
    const step = pick("maria");

    // Focus sits on the step, not the box, so a tablet's own keyboard does not
    // slide up over the keypad; a hardware keyboard still types straight in.
    for (const key of ["4", "8", "2", "9"]) {
      fireEvent.keyDown(document.activeElement ?? step, { key });
    }
    fireEvent.keyDown(document.activeElement ?? step, { key: "Backspace" });
    expect(pinBox().value).toBe("482");

    await userEvent.type(pinBox(), "1");
    expect(pinBox().value).toBe("4821");

    fireEvent.click(within(step).getByTestId("login-crew-submit"));
    await waitFor(() =>
      expect(login.mock.calls[0]?.[0]).toEqual({ org: "poke", identifier: "maria", pin: "4821" })
    );
  });

  it("switches to another name without keeping the first one's digits", () => {
    signInPage(kiosk(CLOVIS), [MARIA, DEVON]);
    tapKeys(pick("maria"), "4", "8");
    const step = pick("devon");
    expect(step).toHaveTextContent("Devon");
    expect(pinBox().value).toBe("");
  });

  it("still asks for a PIN of at least four digits", async () => {
    signInPage(kiosk(CLOVIS), [MARIA]);
    const step = pick("maria");
    fireEvent.click(within(step).getByTestId("login-crew-submit"));
    expect(await within(step).findByText("Enter your PIN.")).toBeTruthy();
    tapKeys(step, "4", "8");
    fireEvent.click(within(step).getByTestId("login-crew-submit"));
    expect(await within(step).findByText("Your PIN is at least 4 digits.")).toBeTruthy();
    expect(login).not.toHaveBeenCalled();
  });

  it("says a wrong PIN didn't match, clears it, and remembers nobody", async () => {
    login.mockRejectedValue(httpError(401));
    signInPage(kiosk(CLOVIS), [MARIA]);
    const step = pick("maria");
    tapKeys(step, "4", "8", "1", "2");
    fireEvent.click(within(step).getByTestId("login-crew-submit"));

    expect(await screen.findByTestId("login-crew-error")).toHaveTextContent(
      "That didn't match. Try again."
    );
    expect(pinBox().value).toBe("");
    expect(step.contains(document.activeElement)).toBe(true);
    expect(readRecentCrew(kiosk(CLOVIS))).toEqual([]);

    // A fresh attempt starts clean; the old "didn't match" goes with the old PIN.
    tapKeys(step, "4");
    expect(screen.queryByTestId("login-crew-error")).toBeNull();
  });

  it("holds Sign in down after the kiosk is rate-limited", async () => {
    login.mockRejectedValue(httpError(429));
    signInPage(kiosk(CLOVIS), [MARIA]);
    const step = pick("maria");
    tapKeys(step, "4", "8", "2", "1");
    fireEvent.click(within(step).getByTestId("login-crew-submit"));

    expect(await screen.findByTestId("login-crew-error")).toHaveTextContent(
      "Too many attempts on this kiosk. Wait a minute."
    );
    expect(within(step).getByTestId("login-crew-submit")).toBeDisabled();
  });

  it("remembers who signed in on this kiosk, and never their PIN", async () => {
    login.mockResolvedValue({ token: "t", name: "Maria", expires_in: 60 });
    signInPage(kiosk(CLOVIS), [MARIA, DEVON]);
    tapKeys(pick("maria"), "4", "8", "2", "1");
    fireEvent.click(screen.getByTestId("login-crew-submit"));

    // This kiosk has no app to open, so the page says who is signed in.
    expect(await screen.findByText("Signed in as Maria.")).toBeTruthy();
    expect(readRecentCrew(kiosk(CLOVIS))).toEqual(["maria"]);
    expect(localStorage.getItem(recentCrewKey(kiosk(CLOVIS)))).not.toContain("4821");
  });
});
