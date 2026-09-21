// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { FrontlineStaff } from "@/types/frontline";
import CrewRosterPicker from "./CrewRosterPicker";

afterEach(cleanup);

const staff = (...names: string[]): FrontlineStaff[] =>
  names.map((name) => ({ identifier: name.toLowerCase(), name }));

const picker = (people: FrontlineStaff[]) =>
  render(<CrewRosterPicker staff={people} selected='' onSelect={vi.fn()} />);

describe("CrewRosterPicker", () => {
  it("still shows every name it was given, grouped under its letter", () => {
    picker(staff("Devon", "Ana", "Cy"));
    for (const name of ["Devon", "Ana", "Cy"]) {
      expect(screen.getByText(name)).toBeTruthy();
    }
    expect(screen.getByTestId("login-crew-letter-A")).toBeTruthy();
    expect(screen.getByTestId("login-crew-letter-C")).toBeTruthy();
    expect(screen.getByTestId("login-crew-letter-D")).toBeTruthy();
  });

  it("leaves the rail off when there is nowhere to jump", () => {
    // Three names under one letter — a small store. A rail of one entry is
    // decoration on a screen that has no room for any.
    picker(staff("Ana", "Abe", "Amy"));
    expect(screen.queryByTestId("login-crew-letter-rail")).toBeNull();
    expect(screen.getByTestId("login-crew-staff-ana")).toBeTruthy();
  });

  it("carries a full alphabet at a store the size the row cap now allows", () => {
    const many = Array.from({ length: 200 }, (_, i) =>
      String.fromCharCode(65 + (i % 26)).concat(`name${i}`)
    );
    picker(staff(...many));
    expect(screen.getByTestId("login-crew-letter-rail")).toBeTruthy();
    expect(screen.getByTestId("login-crew-letter-A")).toBeTruthy();
    expect(screen.getByTestId("login-crew-letter-Z")).toBeTruthy();
    expect(screen.getAllByTestId(/^login-crew-staff-/)).toHaveLength(200);
  });

  it("scrolls the roster box, never the page", () => {
    // `scrollIntoView` would drag the login card — and the PIN field with it —
    // up the screen on a tablet, so the rail moves the box by hand. jsdom
    // implements neither, which is why both are stubbed rather than spied.
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

  it("takes you back to the top of the letter you are already scrolled inside", () => {
    // The heading is sticky: partway down D it is pinned to the top of the box,
    // so measuring IT says "already there" and the tap does nothing. The group
    // it heads is not sticky, and its top is where "back to D" means. jsdom
    // lays nothing out, so the geometry is stated: the box sits at y=100 and is
    // scrolled to 300, D's group began 40px above the box's top, and D's heading
    // is pinned at the box's top.
    const scrollTo = vi.fn();
    Element.prototype.scrollTo = scrollTo;

    picker(staff("Ana", "Devon", "Dora"));
    const list = screen.getByTestId("login-crew-roster");
    const heading = screen.getByRole("heading", { name: "D" });
    const group = heading.parentElement as HTMLElement;
    const at = (top: number) => () => ({ top }) as DOMRect;
    Object.defineProperty(list, "scrollTop", { configurable: true, value: 300 });
    list.getBoundingClientRect = at(100);
    heading.getBoundingClientRect = at(100);
    group.getBoundingClientRect = at(60);

    fireEvent.click(screen.getByTestId("login-crew-letter-D"));

    expect(scrollTo).toHaveBeenCalledWith({ top: 260, behavior: "smooth" });
  });
});
