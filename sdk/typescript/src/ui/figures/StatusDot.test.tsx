// @vitest-environment jsdom
//
// The status dot, decision 38b: round, and always with its word. The word is printed on screen
// (not only for a screen reader), the dot is decoration in its tone's `--dot-*` fill, the quiet
// step is a ring, and the word can describe a control by id. 2.31.0: `wordHidden`, the dot alone
// where the words beside it already say the state, the word kept for assistive tech and the hover.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { STATUS_TONES } from "../tone";
import { $, render } from "../ui.harness";
import { StatusDot } from "./StatusDot";

afterEach(() => {
  document.body.innerHTML = "";
});

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "figures.css"), "utf8");
const rule = (selector: string) => {
  const at = css.indexOf(`${selector} {`);
  return at < 0 ? "" : css.slice(at, css.indexOf("}", at));
};

describe("StatusDot", () => {
  it("prints its word beside a decorative dot, in every tone", async () => {
    for (const status of STATUS_TONES) {
      const page = await render(<StatusDot status={status} label='Within range' />);
      const root = $(".oxy-ui-status-dot") as HTMLElement;
      const dot = $(".oxy-ui-status-dot__dot") as HTMLElement;
      const word = $(".oxy-ui-status-dot__word") as HTMLElement;
      expect(root.textContent, status).toBe("Within range");
      expect(word.textContent).toBe("Within range");
      expect(dot.getAttribute("aria-hidden")).toBe("true");
      expect(dot.textContent).toBe("");
      expect(dot.classList).toContain(`oxy-ui-status-dot__dot--${status}`);
      expect(rule(`.oxy-ui-status-dot__dot--${status}`), status).toMatch(
        /background-color: var\(--/
      );
      await page.unmount();
    }
  });

  it("never hides the word unless asked: no other rule makes it screen-reader-only", () => {
    const word = rule(".oxy-ui-status-dot__word");
    expect(word).not.toMatch(/position: absolute|clip|width: 1px|display: none|visibility/);
    // The one rule that hides it is the opt-in `wordHidden` modifier's.
    expect(css).not.toMatch(
      /\.oxy-ui-status-dot__word(?!--hidden)[^{]*\{[^}]*(clip|display: none)/
    );
  });

  it("wordHidden: the dot alone, the word for assistive tech and as the hover title", async () => {
    const page = await render(<StatusDot status='warning' label='Still settling' wordHidden />);
    const root = $(".oxy-ui-status-dot") as HTMLElement;
    const word = $(".oxy-ui-status-dot__word") as HTMLElement;
    expect(word.textContent).toBe("Still settling");
    expect(word.classList).toContain("oxy-ui-status-dot__word--hidden");
    expect(root.classList).toContain("oxy-ui-status-dot--word-hidden");
    expect(root.title).toBe("Still settling");
    expect(rule(".oxy-ui-status-dot__word--hidden")).toMatch(/clip-path: inset\(50%\)/);
    // A caller's own title wins; without wordHidden there is no title and no hiding.
    await page.rerender(
      <StatusDot status='warning' label='Still settling' wordHidden title='Settles by 6 AM' />
    );
    expect(root.title).toBe("Settles by 6 AM");
    await page.rerender(<StatusDot status='warning' label='Still settling' />);
    expect(root.hasAttribute("title")).toBe(false);
    expect(word.classList.contains("oxy-ui-status-dot__word--hidden")).toBe(false);
  });

  it("is round, 7px, where every container is square", () => {
    const dot = rule(".oxy-ui-status-dot__dot");
    expect(dot).toMatch(/width: 7px;/);
    expect(dot).toMatch(/height: 7px;/);
    expect(dot).toMatch(/border-radius: var\(--radius-pill\)/);
  });

  it("draws the quiet step as a ring, and lends its word to a control by id", async () => {
    await render(
      <>
        <a href='/rail' aria-describedby='live-word'>
          Settings
        </a>
        <StatusDot status='warning' label='Notable' quiet wordId='live-word' />
      </>
    );
    expect($(".oxy-ui-status-dot__dot")?.classList).toContain("oxy-ui-status-dot__dot--quiet");
    expect(rule(".oxy-ui-status-dot__dot--quiet")).toMatch(/background-color: transparent/);
    const link = $("a") as HTMLAnchorElement;
    const id = link.getAttribute("aria-describedby") ?? "";
    expect(document.getElementById(id)?.textContent).toBe("Notable");
  });
});
