// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { readTokenLoginLink, stripFragment } from "./tokenLoginLink";

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

describe("readTokenLoginLink", () => {
  it("reads the ticket and both destinations out of the fragment", () => {
    const hash = `#ticket=tkt-1&next=%2Fide%3Ftab%3Druns&return_to=${encodeURIComponent(
      "https://acme--store.customer-apps.example.com/"
    )}`;
    expect(readTokenLoginLink(hash)).toEqual({
      ticket: "tkt-1",
      next: "/ide?tab=runs",
      returnTo: "https://acme--store.customer-apps.example.com/"
    });
  });

  it("reads a link that carries only a ticket", () => {
    expect(readTokenLoginLink("#ticket=tkt-1")).toEqual({
      ticket: "tkt-1",
      next: null,
      returnTo: undefined
    });
  });

  it("has no ticket when the fragment is empty, or names none", () => {
    expect(readTokenLoginLink("").ticket).toBeNull();
    expect(readTokenLoginLink("#").ticket).toBeNull();
    expect(readTokenLoginLink("#next=%2Fide").ticket).toBeNull();
    // An empty value is no ticket: there is nothing to redeem.
    expect(readTokenLoginLink("#ticket=").ticket).toBeNull();
  });
});

describe("stripFragment", () => {
  it("drops the fragment and keeps the path and query", () => {
    window.history.replaceState(null, "", "/token-login?from=agent#ticket=tkt-1&next=%2Fide");
    stripFragment();
    expect(window.location.pathname).toBe("/token-login");
    expect(window.location.search).toBe("?from=agent");
    expect(window.location.hash).toBe("");
  });

  it("replaces the entry rather than adding one, and keeps the router's history state", () => {
    const routerState = { idx: 3, key: "abc", usr: null };
    window.history.replaceState(routerState, "", "/token-login#ticket=tkt-1");
    const entries = window.history.length;

    stripFragment();

    // Back must not lead to a URL that still has the ticket in it.
    expect(window.history.length).toBe(entries);
    expect(window.history.state).toEqual(routerState);
  });
});
