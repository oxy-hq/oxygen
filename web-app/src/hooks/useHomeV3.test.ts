// @vitest-environment jsdom

import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import useHomeV3 from "./useHomeV3";

let homeV3: boolean | undefined;
vi.mock("@/contexts/AuthContext", () => ({
  useOptionalAuth: () => ({ isLocalMode: false, authConfig: { home_v3: homeV3 } })
}));

afterEach(() => {
  homeV3 = undefined;
});

describe("useHomeV3", () => {
  it("is on with home_v3", () => {
    homeV3 = true;
    expect(renderHook(() => useHomeV3()).result.current).toBe(true);
  });

  it("is off when an older server sends no home_v3", () => {
    expect(renderHook(() => useHomeV3()).result.current).toBe(false);
  });
});
