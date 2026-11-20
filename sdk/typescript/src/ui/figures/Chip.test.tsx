// @vitest-environment jsdom
//
// The chip: every tone from `chipTone` on the `tag` recipe, an accent colour that is not a
// status, and the removable chip, whose × names the entry, removes once per press, locks when
// disabled and never submits the form it sits in.

import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { STATUS_TONES } from "../tone";
import { $, click, render } from "../ui.harness";
import { Chip } from "./Chip";

afterEach(() => {
  document.body.innerHTML = "";
});

const chip = () => $(".oxy-ui-chip") as HTMLSpanElement;

describe("Chip", () => {
  it("takes each tone from chipTone, on the tag recipe, and prints its word", async () => {
    for (const tone of STATUS_TONES) {
      const page = await render(<Chip tone={tone}>Over</Chip>);
      const classes = chip().classList;
      for (const c of `${ui.tag} ${ui.chipTone[tone]}`.split(" "))
        expect(classes, tone).toContain(c);
      for (const other of STATUS_TONES.filter((t) => t !== tone))
        expect(classes.contains(ui.chipTone[other]), `${tone} wears ${other}`).toBe(false);
      expect(chip().textContent).toBe("Over");
      await page.unmount();
    }
  });

  it("without a tone is the bare tag, and with a colour fills from that colour", async () => {
    const page = await render(<Chip>Zone</Chip>);
    expect(chip().className).not.toMatch(/oxy-ui-tone-/);
    expect(chip().getAttribute("style")).toBeNull();
    await page.rerender(<Chip color='rgb(10, 120, 200)'>Frozen</Chip>);
    expect(chip().style.color).toBe("rgb(10, 120, 200)");
    expect(chip().getAttribute("style")).toMatch(
      /background: color-mix\(in srgb, rgb\(10, 120, 200\) 15%, transparent\)/
    );
  });

  it("puts the icon before the label and spaces them", async () => {
    await render(
      <Chip tone='info' icon={<svg aria-hidden='true' data-glyph='' />}>
        Chilled
      </Chip>
    );
    expect(chip().firstElementChild?.hasAttribute("data-glyph")).toBe(true);
    expect(chip().classList).toContain("oxy-ui-chip--gap");
  });

  it("removes once per press, by a × named for the entry, and never submits its form", async () => {
    const onRemove = vi.fn();
    const onSubmit = vi.fn((e: Event) => e.preventDefault());
    const page = await render(
      <form onSubmit={(e) => onSubmit(e.nativeEvent)}>
        <Chip
          tone='neutral'
          onRemove={onRemove}
          removeLabel='Remove Matson from shipping companies'
        >
          Matson
        </Chip>
      </form>
    );
    const x = $("button") as HTMLButtonElement;
    expect(x.getAttribute("aria-label")).toBe("Remove Matson from shipping companies");
    expect(x.type).toBe("button");
    expect(x.classList).toContain(ui.focusRing);
    expect(x.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
    expect(chip().classList).toContain("oxy-ui-chip--removable");
    await click(x);
    expect(onRemove).toHaveBeenCalledTimes(1);
    expect(onSubmit).not.toHaveBeenCalled();
    await page.rerender(
      <form>
        <Chip onRemove={onRemove} removeLabel='Remove Matson' disabled>
          Matson
        </Chip>
      </form>
    );
    await click($("button"));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it("has no × at all when nothing can remove it", async () => {
    await render(<Chip tone='warning'>2 late</Chip>);
    expect($("button")).toBeNull();
    expect(chip().classList.contains("oxy-ui-chip--removable")).toBe(false);
  });
});
