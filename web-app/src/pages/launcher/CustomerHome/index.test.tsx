// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import CustomerHome from "./index";

/** Which home the apps-first experience shows: v3 behind `home_v3`, else v2. Each is tested beside it. */

let homeV3 = false;
vi.mock("@/hooks/useHomeV3", () => ({ default: () => homeV3 }));
vi.mock("./HomeV2", () => ({ default: () => <div data-testid='home-v2' /> }));
vi.mock("./HomeV3", () => ({ default: () => <div data-testid='home-v3' /> }));

afterEach(() => {
  cleanup();
  homeV3 = false;
});

describe("the customer home", () => {
  it("is v2 with home_v3 off", () => {
    render(<CustomerHome />);
    expect(screen.getByTestId("home-v2")).toBeInTheDocument();
    expect(screen.queryByTestId("home-v3")).toBeNull();
  });

  it("is v3 with home_v3 on", () => {
    homeV3 = true;
    render(<CustomerHome />);
    expect(screen.getByTestId("home-v3")).toBeInTheDocument();
    expect(screen.queryByTestId("home-v2")).toBeNull();
  });
});
