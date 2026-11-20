// Started from pokehouse-oxy:apps/bookkeeping/src/lib/tabOverflow.ts (tested,
// `tabOverflow.test.ts`; the More-menu variant of `stripFit` for the shell's top bar, decision
// 29's measured fit). Unchanged.
//
// Which screen tabs the top bar shows, and which go behind its More menu. The rule:
//   - Every tab fits: all of them, no More.
//   - Otherwise the leading run that fits beside the More button, in order, and the open
//     screen ALWAYS: in its own place when the run reaches it, else last, so the screen you are
//     on is never hidden in a menu.
//   - The rest go in the menu, in their order.
//
// Widths are measured (`ParentTabs` in the top bar), so this is plain arithmetic.

export interface TabSplit {
  /** Indexes of the tabs in the bar, in the order they draw. */
  shown: number[];
  /** Indexes of the tabs in the More menu, in order. */
  hidden: number[];
}

/** Half a pixel of slack, for sub-pixel widths that sum to just over. */
const SLACK = 0.5;

export function splitTabs(
  widths: readonly number[],
  moreWidth: number,
  available: number,
  active: number
): TabSplit {
  const all = widths.map((_, i) => i);
  const total = widths.reduce((sum, w) => sum + w, 0);
  if (total <= available + SLACK) return { shown: all, hidden: [] };

  const open = active >= 0 && active < widths.length ? active : -1;
  let budget = available - moreWidth - (open >= 0 ? widths[open] : 0);
  const run: number[] = [];
  let stoppedAt = widths.length;
  for (let i = 0; i < widths.length; i++) {
    if (i === open) continue;
    if (widths[i] > budget + SLACK) {
      stoppedAt = i;
      break;
    }
    budget -= widths[i];
    run.push(i);
  }

  let shown = run;
  if (open >= 0) {
    shown = open < stoppedAt ? [...run, open].sort((a, b) => a - b) : [...run, open];
  }
  const inBar = new Set(shown);
  return { shown, hidden: all.filter((i) => !inBar.has(i)) };
}
