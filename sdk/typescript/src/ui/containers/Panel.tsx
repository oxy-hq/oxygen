// Started from quadrogen-oxy `apps/command-center/src/components/Panel.tsx` (INVENTORY section 2:
// the closest of 21 copies to the reference's, diff 16, and the only one with a test, "Panel is a
// card with an eyebrow header over a rule"). Kept: the props (`title`, `right`, `className`) and
// the recipe it is made of. Changed: `title` takes any node, not only a string (a superset, so an
// app moves by an import swap), and the element is the SDK's recipes rather than Tailwind.
//
// A titled section, the app's one, for every screen that has one: `card` + `panel` with an
// `eyebrow` header over a rule. One component, one recipe: a panel on one screen is the same
// object as a panel on another.

import type { ReactNode } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";

export interface PanelProps {
  /** The section's label, drawn as a caps eyebrow. */
  title: ReactNode;
  /** Small meta text on the same line as the title, right-aligned. */
  right?: ReactNode;
  children: ReactNode;
  className?: string;
}

export function Panel({ title, right, children, className }: PanelProps) {
  return (
    <div className={cx(ui.panel, ui.card, "oxy-ui-panel--titled", className)}>
      <div className={cx(ui.panelHeader, ui.eyebrow)}>
        <span>{title}</span>
        {right ?? null}
      </div>
      <div className='oxy-ui-panel__body'>{children}</div>
    </div>
  );
}
