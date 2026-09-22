// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { FrontlineStaff } from "@/types/frontline";
import CrewRosterPicker from "./CrewRosterPicker";

afterEach(cleanup);

const staff = (...names: string[]): FrontlineStaff[] =>
  names.map((name) => ({ identifier: name.toLowerCase(), name }));

/** `count` distinct names spread across the alphabet. */
const crewOf = (count: number) =>
  staff(
    ...Array.from({ length: count }, (_, i) =>
      String.fromCharCode(65 + (i % 26)).concat(`name${i}`)
    )
  );

const picker = (
  people: FrontlineStaff[],
  { recent = [], selected = "" }: { recent?: string[]; selected?: string } = {}
) =>
  render(
    <CrewRosterPicker
      staff={people}
      recent={recent}
      placeName='Clovis'
      selected={selected}
      onSelect={vi.fn()}
    />
  );

/** The tiles of the main list, in the order they are drawn. */
const listed = () =>
  within(screen.getByTestId("login-crew-roster"))
    .getAllByTestId(/^login-crew-staff-/)
    .map((tile) => tile.getAttribute("data-testid")?.replace("login-crew-staff-", ""));

describe("CrewRosterPicker", () => {
  it("runs every name on in one A–Z flow, with no block per letter", () => {
    picker(staff("Devon", "Ana", "Cy", "Abe"));
    expect(listed()).toEqual(["abe", "ana", "cy", "devon"]);
    // The letters live on the tiles and the rail now, never as headings that
    // break the grid into short rows.
    expect(screen.queryAllByRole("heading", { name: /^[A-Z#]$/ })).toHaveLength(0);
  });

  it("marks only the first name of each letter, and keeps the mark out of the name", () => {
    picker(staff("Devon", "Ana", "Cy", "Abe"));
    const mark = (id: string) =>
      screen.getByTestId(`login-crew-staff-${id}`).querySelector("[data-letter-mark]");
    expect(mark("abe")?.textContent).toBe("A");
    expect(mark("ana")).toBeNull();
    expect(mark("cy")?.textContent).toBe("C");
    expect(mark("devon")?.textContent).toBe("D");
    // A screen reader hears "Abe", not "A Abe".
    expect(mark("abe")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("leaves the rail off when there is nowhere to jump", () => {
    // Three names under one letter — a small store. A rail of one entry is
    // decoration on a screen that has no room for any.
    picker(staff("Ana", "Abe", "Amy"));
    expect(screen.queryByTestId("login-crew-letter-rail")).toBeNull();
    expect(screen.getByTestId("login-crew-staff-ana")).toBeTruthy();
  });

  it("gives every rail letter a name a screen reader can say", () => {
    picker(staff("Ana", "Devon", "2Pac"));
    expect(screen.getByRole("button", { name: "Jump to A" })).toBe(
      screen.getByTestId("login-crew-letter-A")
    );
    expect(screen.getByRole("button", { name: "Jump to D" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Jump to names that start with a number or symbol" })
    ).toBe(screen.getByTestId("login-crew-letter-#"));
  });

  it("carries a full alphabet at a store the size the row cap now allows", () => {
    picker(crewOf(200));
    expect(screen.getByTestId("login-crew-letter-rail")).toBeTruthy();
    expect(screen.getByTestId("login-crew-letter-A")).toBeTruthy();
    expect(screen.getByTestId("login-crew-letter-Z")).toBeTruthy();
    expect(listed()).toHaveLength(200);
  });

  it("scrolls the roster box, never the page", () => {
    // `scrollIntoView` would drag the whole kiosk screen up on a tablet, so the
    // rail moves the box by hand. jsdom implements neither, which is why both
    // are stubbed rather than spied.
    const scrollTo = vi.fn();
    Element.prototype.scrollTo = scrollTo;
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;

    picker(staff("Ana", "Devon"));
    fireEvent.click(screen.getByTestId("login-crew-letter-D"));

    expect(scrollTo).toHaveBeenCalledTimes(1);
    expect(scrollTo.mock.calls[0][0]).toMatchObject({ behavior: "smooth" });
    expect(scrollIntoView).not.toHaveBeenCalled();
  });

  it("lands the letter's first name at the top of the box, and puts focus on it", () => {
    // jsdom lays nothing out, so the geometry is stated: the box sits at y=100
    // and is scrolled to 300, and Devon — the first D — is drawn 40px above the
    // box's top edge.
    const scrollTo = vi.fn();
    Element.prototype.scrollTo = scrollTo;

    picker(staff("Ana", "Devon", "Dora"));
    const list = screen.getByTestId("login-crew-roster");
    const devon = screen.getByTestId("login-crew-staff-devon");
    const at = (top: number) => () => ({ top }) as DOMRect;
    Object.defineProperty(list, "scrollTop", { configurable: true, value: 300 });
    list.getBoundingClientRect = at(100);
    devon.getBoundingClientRect = at(60);

    fireEvent.click(screen.getByTestId("login-crew-letter-D"));

    expect(scrollTo).toHaveBeenCalledWith({ top: 260, behavior: "smooth" });
    // A keyboard user lands on the name, so the arrow keys carry on from there.
    expect(document.activeElement).toBe(devon);
  });

  it("keeps a picked name in view when the PIN step shrinks the list under it", () => {
    // On a phone the PIN sheet takes the bottom of the screen, so the list box
    // gets shorter in the same render that picks the name — and the tile just
    // tapped can end up below its new bottom edge. The box sits at y=100..300
    // unscrolled; Devon's tile is now drawn at y=320..376, 76px past the edge.
    const scrollTo = vi.fn();
    Element.prototype.scrollTo = scrollTo;
    const people = staff("Ana", "Devon");
    const { rerender } = picker(people);
    const list = screen.getByTestId("login-crew-roster");
    const devon = screen.getByTestId("login-crew-staff-devon");
    const box = (top: number, bottom: number) => () => ({ top, bottom }) as DOMRect;
    Object.defineProperty(list, "scrollTop", { configurable: true, value: 0 });
    list.getBoundingClientRect = box(100, 300);
    devon.getBoundingClientRect = box(320, 376);

    rerender(
      <CrewRosterPicker staff={people} placeName='Clovis' selected='devon' onSelect={vi.fn()} />
    );

    expect(scrollTo).toHaveBeenCalledWith({ top: 76, behavior: "smooth" });
  });

  it("leaves the list where it is when the picked name is already in view", () => {
    const scrollTo = vi.fn();
    Element.prototype.scrollTo = scrollTo;
    const people = staff("Ana", "Devon");
    const { rerender } = picker(people);
    const list = screen.getByTestId("login-crew-roster");
    const devon = screen.getByTestId("login-crew-staff-devon");
    const box = (top: number, bottom: number) => () => ({ top, bottom }) as DOMRect;
    list.getBoundingClientRect = box(100, 300);
    devon.getBoundingClientRect = box(150, 206);

    rerender(
      <CrewRosterPicker staff={people} placeName='Clovis' selected='devon' onSelect={vi.fn()} />
    );

    expect(scrollTo).not.toHaveBeenCalled();
  });

  describe("Find your name", () => {
    it("appears only past 30 people", () => {
      picker(crewOf(30));
      expect(screen.queryByRole("searchbox", { name: "Find your name" })).toBeNull();
      cleanup();

      picker(crewOf(31));
      expect(screen.getByRole("searchbox", { name: "Find your name" })).toBeTruthy();
    });

    it("narrows the list as you type, and says so when nobody matches", () => {
      picker([...crewOf(30), ...staff("Devon Hale")]);
      const search = screen.getByRole("searchbox", { name: "Find your name" });

      fireEvent.change(search, { target: { value: "devon" } });
      expect(listed()).toEqual(["devon hale"]);
      expect(screen.queryByText("No one here by that name.")).toBeNull();

      fireEvent.change(search, { target: { value: "nobody-called-this" } });
      expect(screen.queryAllByTestId(/^login-crew-staff-/)).toHaveLength(0);
      expect(screen.getByText("No one here by that name.")).toBeTruthy();

      fireEvent.change(search, { target: { value: "" } });
      expect(listed()).toHaveLength(31);
    });
  });

  describe("On this tablet recently", () => {
    const store = () => crewOf(20);
    const recentTiles = () =>
      screen
        .queryAllByTestId(/^login-crew-recent-/)
        .map((tile) => tile.getAttribute("data-testid")?.replace("login-crew-recent-", ""));

    it("puts the last people who signed in here first, four at most, and only those still here", () => {
      const [a, b, c, d, e] = store().map((m) => m.identifier);
      picker(store(), { recent: ["gone-last-week", c, a, e, b, d] });
      expect(screen.getByRole("heading", { name: "On this tablet recently" })).toBeTruthy();
      expect(recentTiles()).toEqual([c, a, e, b]);
      // Still in the full list too: the row is a shortcut, not a move.
      expect(screen.getByTestId(`login-crew-staff-${c}`)).toBeTruthy();
    });

    it("stays off at a small store, and while someone is searching", () => {
      const small = crewOf(11);
      picker(small, { recent: [small[0].identifier] });
      expect(recentTiles()).toEqual([]);
      cleanup();

      const big = crewOf(40);
      picker(big, { recent: [big[0].identifier] });
      expect(recentTiles()).toEqual([big[0].identifier]);
      fireEvent.change(screen.getByRole("searchbox", { name: "Find your name" }), {
        target: { value: "name" }
      });
      expect(recentTiles()).toEqual([]);
    });

    it("shows a picked name dark with light text in the recent row as well as the list", () => {
      // The recent row is tinted to set it apart. The tint must never win over
      // the picked state, or the worker cannot see who they just tapped.
      const [first] = store().map((m) => m.identifier);
      picker(store(), { recent: [first], selected: first });
      for (const tile of [
        screen.getByTestId(`login-crew-recent-${first}`),
        screen.getByTestId(`login-crew-staff-${first}`)
      ]) {
        expect(tile).toHaveAttribute("data-state", "on");
        expect(tile.className).toContain("data-[state=on]:bg-primary");
        expect(tile.className).toContain("data-[state=on]:text-primary-foreground");
      }
    });
  });

  it("says how many people work here", () => {
    picker(crewOf(20));
    expect(screen.getByRole("heading", { name: "Everyone at Clovis · 20" })).toBeTruthy();
  });
});
