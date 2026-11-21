// Started from quadrogen-oxy `apps/command-center/src/components/Panel.tsx` (INVENTORY section 2:
// the closest of 21 copies to the reference's, diff 16, and the only one with a test, "Panel is a
// card with an eyebrow header over a rule"). Kept: the props (`title`, `right`, `className`) and
// the recipe it is made of. Changed: `title` takes any node, not only a string (a superset, so an
// app moves by an import swap), and the element is the SDK's recipes rather than Tailwind.
//
// 2.31.0 (the gap round): what the apps' kept copies did, each opt-in, so a panel that passes none
// of it is drawn exactly as before.
//   - `header`, `as` + `label`, `open`, `bodyClassName`: pokehouse-oxy Command Center's
//     `ui/Panel.tsx` (a store panel's own header with its back button, the rails as `aside`
//     landmarks, Live events folded to its header, a rail's body that scrolls).
//   - `foldable`, `defaultOpen`, `right` as a function of the fold: Site Scout's
//     `chrome/Panel.tsx` (every panel of the map's left column folds behind a caret button).
//   - `headingLevel`: Supply Chain, Marketing and Amazon P&L title their panels with an `<h2>`,
//     so heading navigation lists them; with `as="section"` the heading names the region.
//   - `density="compact"`: the six staging demo apps' panel (12px in, 6px under the rule, a 12px
//     body in the secondary ink), whose eyebrow is the title's alone, so a chip or a button on the
//     right keeps its own case and ink and the header keeps the body's line.
//   - `title` optional: Client Management's panels, some with no header at all.
//
// A titled section, the app's one, for every screen that has one: `card` + `panel` with an
// `eyebrow` header over a rule. One component, one recipe: a panel on one screen is the same
// object as a panel on another.
//
// **The fold** (decision 47: an action in a panel header is a text button, no fill, no box): the
// title becomes the button, a caret in front of it, `aria-expanded` and `aria-controls` on it.
// Folded, the body is not drawn, the rule under the header goes, and the panel gives its height
// back to a column it shares.

import { type ReactNode, useId, useState } from "react";
import { cx } from "../cx";
import { CaretDownIcon, CaretRightIcon } from "../icons";
import { ui } from "../recipes";

export interface PanelProps {
  /** The section's label, drawn as a caps eyebrow. Optional: a panel may have no header. */
  title?: ReactNode;
  /** Small meta text on the same line as the title, right-aligned. A function gets whether the
   *  panel is open, for a meta that says what the folded body would. */
  right?: ReactNode | ((open: boolean) => ReactNode);
  /** A whole header in place of the eyebrow title (a record's name, its back button). Drawn on
   *  the header's line over the rule, in its own type; `right` still follows it. */
  header?: ReactNode;
  /** The title as a heading (`<h2>`...), so heading navigation lists the panel. */
  headingLevel?: 2 | 3 | 4;
  /** The element: `section` or `aside` make the panel a landmark. */
  as?: "div" | "section" | "aside";
  /** The landmark's accessible name. Without it, a `section` / `aside` whose title is a heading
   *  is named by that heading. */
  label?: string;
  /** The reader folds the body behind a caret button on the title. */
  foldable?: boolean;
  /** Whether the body shows, when the caller holds it (a fold of its own in `header`, or
   *  `foldable` with `onOpenChange`). Folded: the header alone, no rule under it. */
  open?: boolean;
  /** Where an uncontrolled `foldable` panel starts. Default open. */
  defaultOpen?: boolean;
  /** The fold button was pressed: the state it asks for. */
  onOpenChange?: (open: boolean) => void;
  /** `compact`: 12px in, 6px under the rule, a 12px body in the secondary ink; the eyebrow is the
   *  title's alone. */
  density?: "default" | "compact";
  children?: ReactNode;
  className?: string;
  /** On the body (a body that scrolls inside the panel's height, a layout of its own). */
  bodyClassName?: string;
}

export function Panel({
  title,
  right,
  header,
  headingLevel,
  as: Tag = "div",
  label,
  foldable = false,
  open: openProp,
  defaultOpen = true,
  onOpenChange,
  density = "default",
  children,
  className,
  bodyClassName
}: PanelProps) {
  const [openState, setOpenState] = useState(defaultOpen);
  const open = openProp ?? (foldable ? openState : true);
  const bodyId = useId();
  const titleId = useId();
  const compact = density === "compact";
  const meta = typeof right === "function" ? right(open) : right;
  const toggle = () => {
    const next = !open;
    if (openProp === undefined) setOpenState(next);
    onOpenChange?.(next);
  };

  const fold = foldable ? (
    <button
      type='button'
      aria-expanded={open}
      aria-controls={open ? bodyId : undefined}
      onClick={toggle}
      className={cx("oxy-ui-panel__fold", ui.focusRing, compact && !headingLevel && ui.eyebrow)}
    >
      {open ? (
        <CaretDownIcon className={ui.iconInline} />
      ) : (
        <CaretRightIcon className={ui.iconInline} />
      )}
      <span>{title}</span>
    </button>
  ) : null;
  const Heading = headingLevel ? (`h${headingLevel}` as const) : null;
  const titled = Heading ? (
    <Heading id={titleId} className={cx("oxy-ui-panel__title", compact && ui.eyebrow)}>
      {fold ?? title}
    </Heading>
  ) : (
    (fold ?? <span className={compact ? ui.eyebrow : undefined}>{title}</span>)
  );
  const named = label
    ? { "aria-label": label }
    : Tag !== "div" && Heading && header == null && title != null
      ? { "aria-labelledby": titleId }
      : {};
  const hasHeader = header != null || title != null || meta != null;

  return (
    <Tag
      {...named}
      className={cx(
        ui.panel,
        ui.card,
        "oxy-ui-panel--titled",
        compact && "oxy-ui-panel--compact",
        !open && "oxy-ui-panel--folded",
        className
      )}
    >
      {hasHeader && (
        <div className={cx(ui.panelHeader, header == null && !compact && ui.eyebrow)}>
          {header ?? titled}
          {meta ?? null}
        </div>
      )}
      {open && (
        <div id={bodyId} className={cx("oxy-ui-panel__body", bodyClassName)}>
          {children}
        </div>
      )}
    </Tag>
  );
}
