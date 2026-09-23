// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import RefusalPanel from "./RefusalPanel";

/**
 * The refusal is the whole reason the cursor reset has a UI of its own.
 *
 * `/reset-cursors` answers `409` with prose the backend wrote carefully: it
 * names the table, says why re-pulling it would not converge, and where it
 * could not scope the reset it gives **both** readings rather than picking one.
 * An operator decides whether to override on exactly those sentences. Swallow
 * them into "Reset failed" and the override is made on nothing.
 *
 * ## Why the fixtures below are spelled out in full
 *
 * These three strings are not illustrative. They are the exact output of
 * `CursorResetRefusal`'s `Display` impl in
 * `crates/agentic/airway/src/reset/convergence.rs`, captured by running it —
 * not transcribed by eye — including the `→` in "resource→table", the em
 * dashes, and the backticks.
 *
 * Asserting them character-for-character (`textContent` + `toBe`, which unlike
 * `toHaveTextContent` does not normalise whitespace) is the point: it is what
 * fails if someone later truncates a long reason, reflows it, wraps it in a
 * summary line, or renders `reasons.length` instead of the reasons. A
 * `toContain("duplicate")` would survive all four and guard nothing.
 *
 * So: do not shorten these fixtures, and do not relax `toBe` to a substring
 * match. If the backend's wording changes, re-capture from the Rust and paste
 * the new string here — the mismatch is the signal, not the nuisance.
 */
const UNKNOWN_OWNERSHIP =
  "cannot scope this reset: the stored schema holds `order_checks`, which no resource root " +
  "claims, so the resource→table mapping is unknown (`Table::parent` is unpopulated and a " +
  "connector may rename nested children). Judging the whole schema instead — either " +
  "`order_checks` belongs to a resource you did not name, or it belongs to one you did and " +
  "the name was remapped. Pass `force` if you know this resource's tables converge";

const KEYLESS_MERGE =
  "`vendor_sales` is `merge` with no effective key, which appends — so re-pulling would " +
  "duplicate rows rather than converge. Use Reset schema if you intend to drop and rebuild";

const APPEND_ONLY =
  "`vendor_forecasting` appends with no merge key, so re-pulling would duplicate rows rather " +
  "than converge. Use Reset schema if you intend to drop and rebuild";

afterEach(cleanup);

const renderPanel = (props: Partial<React.ComponentProps<typeof RefusalPanel>> = {}) =>
  render(
    <RefusalPanel
      reasons={[UNKNOWN_OWNERSHIP]}
      scopeLabel='orders'
      overrideRevealed={false}
      onRevealOverride={() => {}}
      onForce={() => {}}
      forcing={false}
      {...props}
    />
  );

const reasonNodes = () =>
  Array.from(screen.getByTestId("airway-cursor-refusal-reasons").querySelectorAll("li"));

describe("RefusalPanel — the server's reasons, verbatim", () => {
  it("renders the unscopeable-reset refusal character-for-character", () => {
    renderPanel();

    const [item] = reasonNodes();
    // `textContent` + `toBe`, deliberately: `toHaveTextContent` collapses
    // whitespace, so it would pass on a reflowed or re-wrapped string.
    expect(item.textContent).toBe(UNKNOWN_OWNERSHIP);
  });

  it("keeps both readings the backend offers, not just the first", () => {
    renderPanel();

    const text = reasonNodes()[0].textContent ?? "";
    // The sentence exists to stop an operator overriding a conclusion they were
    // never shown the alternative to. Either half alone is a different claim.
    expect(text).toContain("belongs to a resource you did not name");
    expect(text).toContain("belongs to one you did and the name was remapped");
  });

  it("renders every reason, one per line, when several tables refuse", () => {
    renderPanel({ reasons: [UNKNOWN_OWNERSHIP, KEYLESS_MERGE, APPEND_ONLY] });

    const items = reasonNodes();
    expect(items).toHaveLength(3);
    // A refusal that names one table of three invites an override made on a
    // third of the facts — the executor's own words for why it returns them all.
    expect(items.map((li) => li.textContent)).toEqual([
      UNKNOWN_OWNERSHIP,
      KEYLESS_MERGE,
      APPEND_ONLY
    ]);
  });

  it("does not repeat the wire's `would_duplicate` label over a scoping refusal", () => {
    renderPanel();

    // The 409 body hardcodes `error: "would_duplicate"` even when every reason
    // is an unscopeable-reset one, which is not a duplication claim. Printing
    // that label here would put a wrong word above right ones.
    const panel = screen.getByTestId("airway-cursor-refusal");
    expect(panel.textContent).not.toContain("would_duplicate");
    expect(screen.getByText("The server refused this rewind.")).toBeInTheDocument();
  });

  it("says plainly that nothing was changed", () => {
    renderPanel();
    expect(screen.getByText(/Nothing was changed/)).toBeInTheDocument();
  });
});

describe("RefusalPanel — force is reachable only after the reasons are on screen", () => {
  it("offers no override control until it is disclosed", () => {
    renderPanel();

    expect(screen.queryByTestId("airway-cursor-override")).toBeNull();
    expect(screen.queryByRole("button", { name: "Rewind anyway" })).toBeNull();
    // What is offered is the disclosure, not the action.
    expect(screen.getByTestId("airway-cursor-reveal-override")).toBeInTheDocument();
  });

  it("reveals the override through a disclosure, not a checkbox beside the button", () => {
    const onRevealOverride = vi.fn();
    renderPanel({ onRevealOverride });

    // A checkbox sitting next to the confirm can be ticked by someone who never
    // read the reasons; a disclosure cannot be reached without passing them.
    expect(screen.queryByRole("checkbox")).toBeNull();
    fireEvent.click(screen.getByTestId("airway-cursor-reveal-override"));
    expect(onRevealOverride).toHaveBeenCalledOnce();
  });

  it("restates what is being overridden, counted from the reasons shown", () => {
    renderPanel({
      reasons: [UNKNOWN_OWNERSHIP, KEYLESS_MERGE],
      scopeLabel: "2 resources",
      overrideRevealed: true
    });

    const override = screen.getByTestId("airway-cursor-override");
    expect(override.textContent).toContain("all 2 reasons above");
    expect(override.textContent).toContain("2 resources");
    expect(override.textContent).toContain("A later run will not remove them.");
  });

  it("singularises the restatement for a lone reason", () => {
    renderPanel({ reasons: [APPEND_ONLY], overrideRevealed: true });
    expect(screen.getByTestId("airway-cursor-override").textContent).toContain(
      "all 1 reason above"
    );
  });

  it("forces only on the revealed destructive action", () => {
    const onForce = vi.fn();
    renderPanel({ overrideRevealed: true, onForce });

    fireEvent.click(screen.getByRole("button", { name: "Rewind anyway" }));
    expect(onForce).toHaveBeenCalledOnce();
  });

  it("keeps the reasons on screen while the override is open", () => {
    renderPanel({ overrideRevealed: true });
    // The restatement is a summary; it never replaces the sentences it summarises.
    expect(reasonNodes()[0].textContent).toBe(UNKNOWN_OWNERSHIP);
  });

  it("disables the override while a forced rewind is in flight", () => {
    renderPanel({ overrideRevealed: true, forcing: true });
    expect(screen.getByRole("button", { name: "Rewinding…" })).toBeDisabled();
  });
});
