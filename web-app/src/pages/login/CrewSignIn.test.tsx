// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { BoundKioskDevice, FrontlineStaff } from "@/types/frontline";
import CrewSignIn from "./CrewSignIn";

afterEach(cleanup);

const CLOVIS = { id: "loc-clovis", name: "Clovis" };

const kiosk = (location: BoundKioskDevice["location"]): BoundKioskDevice => ({
  bound: true,
  org: "poke",
  orgName: "Poke",
  device: "Front counter",
  location,
  returnTo: null
});

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
      />
    </QueryClientProvider>
  );

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
  });

  it("keeps the ID box on a kiosk with no place, whose sign-in admits the whole org", () => {
    // `null`: enrolled without a place. `undefined`: a server older than the
    // operating graph, which never narrowed anything.
    for (const location of [null, undefined]) {
      signInPage(kiosk(location), []);
      expect(screen.getByTestId("login-crew-identifier")).toBeTruthy();
      expect(screen.getByTestId("login-crew-pin")).toBeTruthy();
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

  it("shows a store's crew as names to tap", () => {
    // Guard: the explanation is for an EMPTY roster, not every store's tablet.
    signInPage(kiosk(CLOVIS), [{ identifier: "maria", name: "Maria" }]);
    expect(screen.getByTestId("login-crew-staff-maria")).toBeTruthy();
    expect(screen.getByTestId("login-crew-pin")).toBeTruthy();
    expect(screen.queryByTestId("login-crew-nobody-here")).toBeNull();
    expect(screen.queryByTestId("login-crew-identifier")).toBeNull();
  });
});
