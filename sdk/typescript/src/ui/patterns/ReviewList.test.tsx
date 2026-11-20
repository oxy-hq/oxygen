// @vitest-environment jsdom
//
// ReviewList: a loading line (announced) while it reads; nothing at all for an empty list unless
// the screen gives words for it; otherwise the title, the caveat ABOVE the cards, and one card per
// item on two fixed lines (the rating line with its picture, figure and flag; the meta line) over
// the text, verbatim; the grid's floor is the caller's.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { $, $$, render } from "../ui.harness";
import { ReviewList, type ReviewListItem } from "./ReviewList";

afterEach(() => {
  document.body.innerHTML = "";
});

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "patterns.css"), "utf8");

const ITEMS: ReviewListItem[] = [
  {
    key: "a",
    rating: <span role='img' aria-label='1 out of 5 stars' data-stars='' />,
    score: "1 star",
    flag: "No reply",
    meta: "Web · 2026-11-02",
    message: "Cold food.\nLong wait."
  },
  { key: "b", score: "2 stars", meta: "App · 2026-11-01", message: "Small portion." }
];

describe("ReviewList", () => {
  it("says it is reading, and shows no list, while it loads", async () => {
    const page = await render(<ReviewList title='What they said' items={ITEMS} loading />);
    const line = $("p[role='status']") as HTMLElement;
    expect(line.textContent).toBe("Loading…");
    expect($("ul")).toBeNull();
    await page.rerender(
      <ReviewList title='What they said' items={[]} loading loadingText='Reading…' />
    );
    expect($("p[role='status']")?.textContent).toBe("Reading…");
  });

  it("renders nothing for an empty list, unless it is given words for one", async () => {
    const page = await render(<ReviewList title='What they said' items={[]} />);
    expect(page.container.innerHTML).toBe("");
    await page.rerender(
      <ReviewList title='What they said' items={[]} empty='Nobody has written in yet.' />
    );
    expect($("h3")?.textContent).toBe("What they said");
    expect($(".oxy-ui-review-list__empty")?.textContent).toBe("Nobody has written in yet.");
    expect($("ul")).toBeNull();
  });

  it("puts the caveat above the cards, one card per item on two fixed lines", async () => {
    await render(
      <ReviewList title='What they said' note='Only reviews with text.' items={ITEMS} min='20rem' />
    );
    const root = $(".oxy-ui-review-list") as HTMLElement;
    expect([...root.children].map((c) => c.tagName)).toEqual(["H3", "P", "UL"]);
    expect(root.children[1].textContent).toBe("Only reviews with text.");
    const grid = $("ul") as HTMLElement;
    expect(grid.style.gridTemplateColumns).toBe("repeat(auto-fit, minmax(20rem, 1fr))");

    const cards = $$("li.oxy-ui-review-list__item");
    expect(cards).toHaveLength(2);
    expect(cards[0].classList).toContain("oxy-ui-card");
    const first = cards[0];
    const rating = first.querySelector(".oxy-ui-review-list__rating") as HTMLElement;
    expect(rating.querySelector("[data-stars]")?.getAttribute("aria-label")).toBe(
      "1 out of 5 stars"
    );
    expect(rating.querySelector(".oxy-ui-review-list__score")?.textContent).toBe("1 star");
    const flag = rating.querySelector(".oxy-ui-review-list__flag") as HTMLElement;
    expect(flag.textContent).toBe("No reply");
    expect(flag.classList).toContain("oxy-ui-ink-warning");
    expect(first.querySelector(".oxy-ui-review-list__meta")?.textContent).toBe("Web · 2026-11-02");
    // The text verbatim, its line breaks kept on screen.
    expect(first.querySelector(".oxy-ui-review-list__message")?.textContent).toBe(
      "Cold food.\nLong wait."
    );
    expect(css).toMatch(/\.oxy-ui-review-list__message \{[^}]*white-space: pre-line;/);
    // An item with no flag and no picture draws neither.
    expect(cards[1].querySelector(".oxy-ui-review-list__flag")).toBeNull();
    expect(cards[1].querySelector("[data-stars]")).toBeNull();
  });

  it("draws the flag in the tone it is given", async () => {
    await render(<ReviewList title='T' items={ITEMS} flagTone='error' />);
    expect($(".oxy-ui-review-list__flag")?.classList).toContain("oxy-ui-ink-error");
  });
});
