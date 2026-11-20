// Started from pokehouse-oxy `apps/store-ops/src/index.css:515` `.table-scroll` and its wrapper
// `src/components/ui/table.tsx` (INVENTORY section 5: one of five differing copies, and the only
// one that keeps the end fade off the focus ring, `.table-scroll:focus-visible::after`). Kept: the
// one-cell grid that holds the table and its fade in one cell, the sticky `::after` fade at the
// cut edge driven by the box's own scroll timeline, gone once the table is scrolled to its end,
// the end room past the last column only while the table overflows, no pinned column
// (decision 30), and the box as a Tab stop (inset ring) and a named region only while it scrolls
// (`useScrollFocusable`, ../focus). New: it wraps any <table> (Store Ops' was its kit Table), the
// ground the fade melts into is a property (`--oxy-ui-table-scroll-ground`, the card by default),
// and Store Ops' `[data-kiosk-device]` exception stays in Store Ops: the SDK has no kiosk notion,
// and an enrolled tablet's screens simply do not use this wrapper (decisions 1 and 2).
//
// A table wider than its box: it scrolls sideways and nothing is pinned for it. The cut edge
// fades while there is more past it. Browsers without scroll-driven animations draw no fade.

import { type ComponentPropsWithRef, type Ref, useCallback } from "react";
import { cx } from "../cx";
import { useScrollFocusable } from "../focus";
import { ui } from "../recipes";

export interface TableScrollProps extends ComponentPropsWithRef<"div"> {
  /** The box's name while it scrolls, when it becomes a Tab stop and a region: usually the
   *  table's own label. Without one it is a Tab stop with no landmark. */
  label?: string;
}

/** Set a ref of any shape. */
function assign<T>(ref: Ref<T> | undefined, value: T | null) {
  if (typeof ref === "function") ref(value);
  else if (ref) (ref as { current: T | null }).current = value;
}

export function TableScroll({ label, className, children, ref, ...rest }: TableScrollProps) {
  const box = useScrollFocusable<HTMLDivElement>();
  const own = box.ref;
  const setRef = useCallback(
    (el: HTMLDivElement | null) => {
      own.current = el;
      assign(ref, el);
    },
    [own, ref]
  );
  const region = box.scrolls && label ? { role: "region", "aria-label": label } : {};
  return (
    <div
      {...rest}
      {...region}
      ref={setRef}
      tabIndex={box.tabIndex}
      data-scrolls={box.scrolls ? "" : undefined}
      className={cx("oxy-ui-table-scroll", ui.focusRingInset, className)}
    >
      {children}
    </div>
  );
}
