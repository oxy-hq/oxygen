import { type KeyboardEvent, useRef } from "react";
import { TYPE_AHEAD_MS, typeAheadTarget } from "../../appBrowserModel";

/** How a selection was made. Clicks push history; keys replace it. */
export type SelectHow = "pointer" | "key";

/**
 * A column browser's keyboard, which is where its speed actually comes from.
 *
 * Three columns side by side only save a click; arrow keys and type-ahead save the
 * trip. The bindings are a listbox's, plus the two a column browser adds:
 *
 * | Key | Does |
 * | --- | ---- |
 * | ↑ / ↓ | move within the column, committing as it goes |
 * | → / Enter | dive into the next column |
 * | ← | back to the previous one |
 * | Home / End | first / last row |
 * | a–z, 0–9, space, `.`, `-` | type-ahead (`typeAheadTarget`) |
 *
 * **Movement clamps rather than wrapping.** Finder clamps, and on a column of forty
 * apps a wrap means holding ↓ silently returns you to the top — you lose your place
 * without anything telling you that you did.
 *
 * A modifier key is left alone: ⌘↓ and ⌥← belong to the browser and the OS, and a
 * column that swallows them breaks text navigation for anyone driving by keyboard.
 */
export function useColumnKeys({
  labels,
  index,
  onPick,
  onLeaveColumn
}: {
  labels: readonly string[];
  /** The selected row, or `-1` when the column has no selection yet. */
  index: number;
  onPick: (index: number) => void;
  onLeaveColumn: (delta: -1 | 1) => void;
}): (event: KeyboardEvent<HTMLElement>) => void {
  const buffer = useRef({ text: "", at: 0 });

  const step = (delta: number) => {
    if (labels.length === 0) return;
    // With no selection, ↓ starts at the top and ↑ at the bottom.
    const from = index < 0 ? (delta > 0 ? -1 : labels.length) : index;
    const next = Math.min(labels.length - 1, Math.max(0, from + delta));
    if (next !== index) onPick(next);
  };

  const typeAhead = (key: string) => {
    const now = Date.now();
    const carried = now - buffer.current.at < TYPE_AHEAD_MS ? buffer.current.text : "";
    const text = carried + key;
    buffer.current = { text, at: now };
    const target = typeAheadTarget(labels, text, index);
    if (target !== null) onPick(target);
  };

  return (event: KeyboardEvent<HTMLElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    switch (event.key) {
      case "ArrowDown":
        step(1);
        break;
      case "ArrowUp":
        step(-1);
        break;
      case "Home":
        step(-labels.length);
        break;
      case "End":
        step(labels.length);
        break;
      case "ArrowRight":
      case "Enter":
        onLeaveColumn(1);
        break;
      case "ArrowLeft":
        onLeaveColumn(-1);
        break;
      default:
        // One printable character, and only the ones a slug or an app name is made of.
        // Space is in on purpose: "Store Ops" is unreachable without it.
        if (event.key.length !== 1 || !/[\w\s.-]/.test(event.key)) return;
        typeAhead(event.key);
    }
    event.preventDefault();
  };
}
