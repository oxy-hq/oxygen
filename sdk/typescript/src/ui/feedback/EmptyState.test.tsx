// @vitest-environment jsdom
//
// The nothing: it says the read worked and found nothing and what makes something appear, with
// its actions; a decorative icon; and the no-match form, which quotes the search, is announced,
// clears it, and puts focus back in the search field (Clear leaves with the empty state).

import { useRef, useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { ui } from "../recipes";
import { $, click, render } from "../ui.harness";
import { EmptyState } from "./EmptyState";

afterEach(() => {
  document.body.innerHTML = "";
});

/** A filtered list whose search finds nothing at first. */
function List() {
  const [q, setQ] = useState("zzz");
  const field = useRef<HTMLInputElement>(null);
  const rows = ["Downtown", "Harbor"].filter((s) => s.toLowerCase().includes(q));
  return (
    <>
      <input ref={field} type='search' aria-label='Find a store' value={q} readOnly />
      {rows.length === 0 ? (
        <EmptyState query={q} onClear={() => setQ("")} searchRef={field} />
      ) : (
        <ul>
          {rows.map((r) => (
            <li key={r}>{r}</li>
          ))}
        </ul>
      )}
    </>
  );
}

describe("EmptyState", () => {
  it("says what is missing and what makes it appear, with its actions", async () => {
    await render(
      <EmptyState
        title='No reports yet'
        icon={<svg id='glyph' />}
        actions={<button type='button'>New report</button>}
      >
        The first one you submit shows up here.
      </EmptyState>
    );
    expect($(".oxy-ui-empty-state__title")?.textContent).toBe("No reports yet");
    const body = $(".oxy-ui-empty-state__body") as HTMLElement;
    expect(body.classList.contains(ui.note)).toBe(true);
    expect(body.textContent).toBe("The first one you submit shows up here.");
    expect($("#glyph")?.parentElement?.getAttribute("aria-hidden")).toBe("true");
    expect($(".oxy-ui-empty-state__actions")?.textContent).toBe("New report");
    expect($(".oxy-ui-empty-state")?.hasAttribute("role")).toBe(false);
  });

  it("for a search that found nothing: quotes it, is announced, clears it, focus to the field", async () => {
    await render(<List />);
    const empty = $(".oxy-ui-empty-state") as HTMLElement;
    expect(empty.getAttribute("role")).toBe("status");
    expect(empty.textContent).toContain("Nothing matches “zzz”");
    const clear = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Clear search"
    ) as HTMLElement;
    clear.focus();
    await click(clear);
    expect($(".oxy-ui-empty-state")).toBeNull();
    expect(document.activeElement).toBe($('[aria-label="Find a store"]'));
  });
});
