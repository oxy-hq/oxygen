// @vitest-environment jsdom
//
// `useOxyMode()` (2.31.0): the mode in force, read from <html> as the stylesheets see it (the
// shell's `data-oxy-mode`, a `.dark` class), and kept current when it changes, for what a
// stylesheet cannot restyle (Site Scout's and Supply Chain's map tiles, Command Center's).

import { act } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { readOxyMode, useOxyMode } from "./mode";
import { $, render } from "./phone/phone.harness";

afterEach(() => {
  document.body.innerHTML = "";
  document.documentElement.removeAttribute("data-oxy-mode");
  document.documentElement.classList.remove("dark");
});

function Tiles() {
  const mode = useOxyMode();
  return <span id='tiles' data-mode={mode} />;
}

const shown = () => $("#tiles")?.dataset.mode;
/** Let the MutationObserver's records reach React. */
const settle = () => act(async () => await Promise.resolve());

describe("useOxyMode", () => {
  it("reads light with nothing applied, and dark from data-oxy-mode or a .dark class", () => {
    expect(readOxyMode()).toBe("light");
    document.documentElement.setAttribute("data-oxy-mode", "dark");
    expect(readOxyMode()).toBe("dark");
    document.documentElement.setAttribute("data-oxy-mode", "light");
    expect(readOxyMode()).toBe("light");
    document.documentElement.classList.add("dark");
    expect(readOxyMode()).toBe("dark");
  });

  it("follows the mode as the shell changes it, both ways", async () => {
    await render(<Tiles />);
    expect(shown()).toBe("light");
    document.documentElement.setAttribute("data-oxy-mode", "dark");
    await settle();
    expect(shown()).toBe("dark");
    document.documentElement.setAttribute("data-oxy-mode", "light");
    await settle();
    expect(shown()).toBe("light");
    document.documentElement.classList.add("dark");
    await settle();
    expect(shown()).toBe("dark");
  });
});
