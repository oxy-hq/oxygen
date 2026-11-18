// PhoneSegmented's sideways scroll, for a row too wide for the screen: which edges
// shell.css fades (`data-oxy-fade`), and where the row scrolls so the selected
// option sits wholly in view, clear of those fades.

/** The width of the fade shell.css draws on an edge with more options beyond it. */
export const SEGMENT_FADE_PX = 24;

/** "start", "end", "start end" or "": the edges of `row` with more of it scrolled out beyond. */
export function edgesToFade(row: HTMLElement): string {
  const max = row.scrollWidth - row.clientWidth;
  // A pixel of slack: a row that fits can still report a fractional overflow.
  if (max <= 1) return "";
  const at = row.scrollLeft;
  return [at > 1 && "start", at < max - 1 && "end"].filter(Boolean).join(" ");
}

/** Where `row` must scroll for `option` to sit wholly in view and clear of the fades, or null if
 *  it is already there, or the row does not scroll. */
export function scrollToShow(row: HTMLElement, option: HTMLElement): number | null {
  const max = row.scrollWidth - row.clientWidth;
  if (max <= 1) return null;
  const box = row.getBoundingClientRect();
  const it = option.getBoundingClientRect();
  const before = it.left - box.left - SEGMENT_FADE_PX;
  const after = it.right - box.right + SEGMENT_FADE_PX;
  let left = row.scrollLeft;
  if (before < 0) left += before;
  else if (after > 0) left += after;
  else return null;
  // At either end there is no fade to clear, so the clamp never hides the option.
  const to = Math.min(max, Math.max(0, Math.round(left)));
  return to === Math.round(row.scrollLeft) ? null : to;
}
