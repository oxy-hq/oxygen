// Started from oxy-hq/reference-app@3caad1f `src/components/ToggleChip.tsx` (INVENTORY section 2:
// the reference is the fullest of three copies, 98 lines against 51-62), with Store Ops' two
// additions (`apps/store-ops/src/chrome/ToggleChip.tsx`, exercised by `views/Compliance.test.tsx`):
// `disabled`, and a `className` on `ToggleChipGroup`. Kept: `pressed` + `onPressedChange`,
// `icon`, `sharesRow`, `aria-pressed`, the outlined look, the ON blue as `--link`.
//
// A filter chip: one option that is either ON or OFF, in a row of others.
//
// **Not a Chip.** A chip is a tag that qualifies the thing beside it; this is a CONTROL, and
// the two must not look alike or a reader starts pressing tags and ignoring filters. So this
// one is outlined where a Chip is filled, and set in caps where a Chip is not.
//
// **Not DefaultTabs.** Tabs pick ONE slice; these are independent, any number on at once.
//
// **24px, 10px caps**, a strip of labels over a dense sheet, not a second toolbar of 32px
// buttons. Unless it shares a row with them (`sharesRow`): then it takes `controlSm`, 32px and
// 12px of side padding, as DefaultTabs' prop of the same name does. On a touch screen, 44px.
//
// **ON is the blue outline, OFF is the field edge.** On is the state that needs finding, so it
// carries the colour; neither fills, so a row stays lighter than the tab strip above it.
// `aria-pressed` carries the state for a screen reader. The blue is `--link`, the app's blue for
// ink (`--brand` is a fill colour and measured 3.10:1 as ink on the dark card).

import type { ComponentPropsWithRef, ReactNode } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";

export interface ToggleChipProps
  extends Omit<ComponentPropsWithRef<"button">, "children" | "onClick" | "type" | "aria-pressed"> {
  pressed: boolean;
  onPressedChange: (pressed: boolean) => void;
  /** Decorative, in front of the label. */
  icon?: ReactNode;
  /** The chip shares a row with 32px controls, so it takes their height. */
  sharesRow?: boolean;
  children: ReactNode;
}

export function ToggleChip({
  pressed,
  onPressedChange,
  icon,
  sharesRow = false,
  className,
  children,
  ...rest
}: ToggleChipProps) {
  return (
    <button
      {...rest}
      type='button'
      aria-pressed={pressed}
      onClick={() => onPressedChange(!pressed)}
      className={cx(
        "oxy-ui-toggle-chip",
        sharesRow && cx(ui.controlSm, "oxy-ui-toggle-chip--shares-row"),
        ui.text3xs,
        ui.focusRing,
        className
      )}
    >
      {icon}
      {children}
    </button>
  );
}

export interface ToggleChipGroupProps {
  /** What the row filters: the caps label, and the group's accessible name. */
  label: string;
  children: ReactNode;
  className?: string;
}

/** A row of ToggleChips under one caps label, so a reader knows what "off" removes. */
export function ToggleChipGroup({ label, children, className }: ToggleChipGroupProps) {
  return (
    // Filters over a sheet, not a form's fieldset: a <fieldset> would bring a border and a
    // <legend> the caps label already is. The reference's markup.
    // biome-ignore lint/a11y/useSemanticElements: see above
    <div role='group' aria-label={label} className={cx("oxy-ui-toggle-chip-group", className)}>
      <span className={cx(ui.eyebrow, "oxy-ui-toggle-chip-group__label")} aria-hidden='true'>
        {label}
      </span>
      {children}
    </div>
  );
}
