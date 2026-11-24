// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ADMIN_RAIL_EXPANDED_KEY, useAdminRailOpen } from "./sidebarPreference";

/**
 * The specification for "a preference you did not set, applied on a surface you were not
 * on".
 *
 * `SidebarProvider` writes one `sidebar_state` cookie, with a hard-coded name, at
 * `path=/`, from every instance in the app — the workspace layout, `OrgWorkspace`,
 * `PartnerLayout` and the admin console, each also binding ⌘B on `window`. Reading it
 * back in admin therefore answered "was *any* sidebar last left open?". Measured before
 * this: one ⌘B on `/local/home` set it to `false` and the next load of `/admin/apps`
 * opened the rail collapsed.
 *
 * So the rule under test is the narrow one — this reads its own key and nothing else —
 * and the second rule is the bug that preceded it: it has to read *something*, or a
 * collapse lasts only until the next reload.
 */

const setCookie = (value: string) => {
  document.cookie = `sidebar_state=${value}; path=/`;
};

beforeEach(() => {
  localStorage.clear();
  setCookie("");
});

afterEach(() => {
  localStorage.clear();
});

describe("useAdminRailOpen", () => {
  it("opens when this browser has no admin preference", () => {
    // Never collapsed here: show the names rather than a rail of unlabelled marks.
    expect(renderHook(() => useAdminRailOpen()).result.current[0]).toBe(true);
  });

  it("remembers a collapse, which is the whole point of collapsing", () => {
    localStorage.setItem(ADMIN_RAIL_EXPANDED_KEY, "false");
    expect(renderHook(() => useAdminRailOpen()).result.current[0]).toBe(false);
  });

  it("ignores the sidebar cookie every other surface writes", () => {
    // The regression. A ⌘B anywhere else in the app must not reach in here.
    setCookie("false");
    expect(renderHook(() => useAdminRailOpen()).result.current[0]).toBe(true);

    // ...and the admin preference wins over it in both directions.
    localStorage.setItem(ADMIN_RAIL_EXPANDED_KEY, "false");
    setCookie("true");
    expect(renderHook(() => useAdminRailOpen()).result.current[0]).toBe(false);
  });

  it("persists a choice under its own key", () => {
    const { result } = renderHook(() => useAdminRailOpen());
    act(() => result.current[1](false));
    expect(result.current[0]).toBe(false);
    expect(localStorage.getItem(ADMIN_RAIL_EXPANDED_KEY)).toBe("false");

    act(() => result.current[1](true));
    expect(localStorage.getItem(ADMIN_RAIL_EXPANDED_KEY)).toBe("true");
  });

  it("treats an unreadable store as open rather than failing to render", () => {
    // Private-mode Safari throws on the accessor itself, and the read runs in a
    // `useState` initialiser — where a throw takes the whole admin console with it.
    const getItem = Storage.prototype.getItem;
    Storage.prototype.getItem = () => {
      throw new Error("denied");
    };
    try {
      expect(renderHook(() => useAdminRailOpen()).result.current[0]).toBe(true);
    } finally {
      Storage.prototype.getItem = getItem;
    }
  });
});
