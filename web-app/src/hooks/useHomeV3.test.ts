// @vitest-environment jsdom

import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import useHomeV3 from "./useHomeV3";

let appsFirst = false;
let homeV3: boolean | undefined;
vi.mock("./useAppsFirstExperience", () => ({ default: () => appsFirst }));
vi.mock("@/contexts/AuthContext", () => ({
  useOptionalAuth: () => ({ isLocalMode: false, authConfig: { home_v3: homeV3 } })
}));

afterEach(() => {
  appsFirst = false;
  homeV3 = undefined;
});

describe("useHomeV3", () => {
  it("is on only with both apps-first and home_v3", () => {
    appsFirst = true;
    homeV3 = true;
    expect(renderHook(() => useHomeV3()).result.current).toBe(true);
  });

  it("is off without apps-first, whatever home_v3 says", () => {
    homeV3 = true;
    expect(renderHook(() => useHomeV3()).result.current).toBe(false);
  });

  it("is off when an older server sends no home_v3", () => {
    appsFirst = true;
    expect(renderHook(() => useHomeV3()).result.current).toBe(false);
  });
});
