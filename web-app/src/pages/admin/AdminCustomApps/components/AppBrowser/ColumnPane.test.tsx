// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ColumnPane, type ColumnRow } from "./ColumnPane";

/**
 * The specification for the column browser's keyboard, which is where its speed comes
 * from — three columns side by side only save a click.
 *
 * These assert behaviour an operator can feel: that ↓ commits as it moves, that one key
 * pressed twice cycles rather than sticking, that → does *not* select, that exactly one
 * row per column is a tab stop, and that ⌘-click is still left to the browser. The last
 * two are the ones that silently rot — a roving tabindex with two tab stops still looks
 * right, and an intercepted ⌘-click still navigates, just in the wrong window.
 */

const ORGS = ["acme", "local", "mars", "mars-eu"];

const rows = (labels: readonly string[]): ColumnRow[] =>
  labels.map((label) => ({
    id: label,
    label,
    href: `/admin/apps?org=${label}`,
    children: <span>{label}</span>
  }));

const renderPane = ({
  labels = ORGS,
  selectedId = null
}: {
  labels?: readonly string[];
  selectedId?: string | null;
} = {}) => {
  const onSelect = vi.fn();
  const onLeaveColumn = vi.fn();
  render(
    <ColumnPane
      id='orgs'
      label='Organizations'
      depth={0}
      width='w-52'
      rows={rows(labels)}
      selectedId={selectedId}
      empty='No org has a custom app yet.'
      onSelect={onSelect}
      onLeaveColumn={onLeaveColumn}
    />
  );
  return { onSelect, onLeaveColumn };
};

const option = (label: string) => screen.getByTestId(`admin-apps-browser-row-orgs-${label}`);
const listbox = () => screen.getByRole("listbox", { name: "Organizations" });

const press = (key: string, init: KeyboardEventInit = {}) => {
  // On the listbox, which is where the handler lives: a real keypress reaches it by
  // bubbling from the focused row.
  listbox().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, ...init }));
};

afterEach(cleanup);

describe("ColumnPane keyboard", () => {
  it("commits as it moves, rather than waiting for Enter", () => {
    const { onSelect } = renderPane({ selectedId: "acme" });
    press("ArrowDown");
    expect(onSelect).toHaveBeenCalledWith("local", "key");
  });

  it("starts at the top on ↓ and at the bottom on ↑ when nothing is selected", () => {
    const first = renderPane();
    press("ArrowDown");
    expect(first.onSelect).toHaveBeenCalledWith("acme", "key");
    cleanup();
    const second = renderPane();
    press("ArrowUp");
    expect(second.onSelect).toHaveBeenCalledWith("mars-eu", "key");
  });

  it("clamps at the ends instead of wrapping", () => {
    // Holding ↓ must not silently return you to the top: you would lose your place with
    // nothing on screen saying so.
    const { onSelect } = renderPane({ selectedId: "mars-eu" });
    press("ArrowDown");
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("jumps to the first and last row", () => {
    const { onSelect } = renderPane({ selectedId: "local" });
    press("End");
    expect(onSelect).toHaveBeenLastCalledWith("mars-eu", "key");
    press("Home");
    expect(onSelect).toHaveBeenLastCalledWith("acme", "key");
  });

  it("jumps by prefix, and cycles when the same key is pressed again", () => {
    const { onSelect } = renderPane({ selectedId: "mars" });
    press("m");
    // From `mars`, one `m` moves on to the next match rather than staying put.
    expect(onSelect).toHaveBeenLastCalledWith("mars-eu", "key");
  });

  it("refines onto the row it is already on as the buffer grows", () => {
    const { onSelect } = renderPane({ selectedId: "acme" });
    press("m");
    expect(onSelect).toHaveBeenLastCalledWith("mars", "key");
    press("a");
    press("r");
    press("s");
    // m-a-r-s lands on `mars`, not past it onto `mars-eu`.
    expect(onSelect).toHaveBeenLastCalledWith("mars", "key");
  });

  it("moves between tiers without selecting anything", () => {
    // → and ← are navigation. Selecting on → would load an app's console just because
    // someone looked at the next tier.
    const { onSelect, onLeaveColumn } = renderPane({ selectedId: "acme" });
    press("ArrowRight");
    expect(onLeaveColumn).toHaveBeenCalledWith(1);
    press("ArrowLeft");
    expect(onLeaveColumn).toHaveBeenCalledWith(-1);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("leaves modified keys to the browser and the OS", () => {
    const { onSelect, onLeaveColumn } = renderPane({ selectedId: "acme" });
    press("ArrowDown", { metaKey: true });
    press("ArrowRight", { altKey: true });
    press("a", { ctrlKey: true });
    expect(onSelect).not.toHaveBeenCalled();
    expect(onLeaveColumn).not.toHaveBeenCalled();
  });

  it("ignores a key that is not a row prefix", () => {
    const { onSelect } = renderPane({ selectedId: "acme" });
    press("z");
    press("Escape");
    expect(onSelect).not.toHaveBeenCalled();
  });
});

describe("ColumnPane rows", () => {
  it("keeps exactly one tab stop — the selection", () => {
    renderPane({ selectedId: "local" });
    const stops = ORGS.filter((l) => option(l).getAttribute("tabindex") === "0");
    expect(stops).toEqual(["local"]);
    expect(option("local")).toHaveAttribute("aria-selected", "true");
  });

  it("makes the first row the tab stop when the column has no selection", () => {
    // Otherwise a tier nobody has visited cannot be reached with Tab at all.
    renderPane();
    const stops = ORGS.filter((l) => option(l).getAttribute("tabindex") === "0");
    expect(stops).toEqual(["acme"]);
  });

  it("selects on a plain click and keeps the row a real link", () => {
    const { onSelect } = renderPane({ selectedId: "acme" });
    expect(option("mars")).toHaveAttribute("href", "/admin/apps?org=mars");
    const plain = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    option("mars").dispatchEvent(plain);
    expect(onSelect).toHaveBeenCalledWith("mars", "pointer");
    expect(plain.defaultPrevented).toBe(true);
  });

  it("leaves a ⌘-click to the browser, so an app still opens in a new tab", () => {
    const { onSelect } = renderPane({ selectedId: "acme" });
    const meta = new MouseEvent("click", {
      bubbles: true,
      cancelable: true,
      button: 0,
      metaKey: true
    });
    option("mars").dispatchEvent(meta);
    expect(onSelect).not.toHaveBeenCalled();
    expect(meta.defaultPrevented).toBe(false);
  });

  it("says a tier is empty instead of rendering an empty listbox", () => {
    renderPane({ labels: [] });
    expect(screen.getByText("No org has a custom app yet.")).toBeInTheDocument();
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("ColumnPane testids", () => {
  it("spells an app's `org/slug` id selector-safely", () => {
    // An unquoted CSS attribute selector cannot hold a `/`, and every agentic flow and
    // hand-written selector on this surface is unquoted. The real id stays reachable on
    // `data-row-id`.
    render(
      <ColumnPane
        id='apps'
        label='Apps'
        depth={1}
        width='w-60'
        rows={[
          {
            id: "local/oxy-starter",
            label: "Oxy Starter",
            href: "/admin/apps/local/oxy-starter",
            children: <span>Oxy Starter</span>
          }
        ]}
        selectedId={null}
        empty='No apps.'
        onSelect={vi.fn()}
        onLeaveColumn={vi.fn()}
      />
    );
    const row = screen.getByTestId("admin-apps-browser-row-apps-local-oxy-starter");
    expect(row).toHaveAttribute("data-row-id", "local/oxy-starter");
  });
});
