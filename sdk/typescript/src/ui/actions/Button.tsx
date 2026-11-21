// Started from oxy-hq/reference-app@3caad1f `src/components/Button.tsx` (INVENTORY section 2: the
// reference has the most sizes; its seven app copies are 31-76 lines with fewer sizes). Its
// matrix, its `inert` and `iconOnly` are kept; two things are new: a busy state (no copy has one)
// and the 44px touch floor (base.css, decision 16).
//
// The app's one button: three variants, two sizes, and nothing else.
//
// **variant**, what the action is worth:
//   primary     the action the surface exists to offer. One per surface.
//   secondary   a real action of lesser standing: back, save, add.
//   text        an action that ADDS to the thing above it ("add another container"), or an
//               icon-only action in a panel header (decision 47). No fill, no border.
//
// **size**, where it is pressed: `sm` 32px (a mouse and a desk), `lg` 44px (entry: a phone, a
// glove, a dock). On a touch screen every size reaches 44px.
//
// One weight, 500, at every variant and size: standing is the fill's job, not the type's.
//
// **busy**: the action is running. The button keeps focus (it is not `disabled`, which would
// drop focus to <body>), says so (`aria-busy`, `aria-disabled`), shows a turning CircleNotch in
// place of its icon and, given one, `busyLabel` in place of its label, and ignores presses
// (a submit included) until it is done.
//
// **inert** draws the same box as a non-interactive <span>: a screen showing an action nothing is
// behind yet. A real <button> that does nothing is a lie to every keyboard and screen-reader user,
// so the span is plain text to them (no role, no name, not focusable).

import type { ComponentPropsWithRef, MouseEvent, ReactNode } from "react";
import { cx } from "../cx";
import { CircleNotchIcon } from "../icons";
import { ui } from "../recipes";

export type ButtonVariant = "primary" | "secondary" | "text";
export type ButtonSize = "sm" | "lg";

/** The class recipe for one variant at one size: the whole matrix, in one place. */
export function buttonClass(variant: ButtonVariant, size: ButtonSize): string {
  if (variant === "text") return size === "lg" ? ui.btn.textLg : ui.btn.text;
  if (size === "lg") return variant === "primary" ? ui.btn.primaryLg : ui.btn.secondaryLg;
  return variant === "primary" ? ui.btn.primary : ui.btn.secondary;
}

export interface ButtonProps extends Omit<ComponentPropsWithRef<"button">, "children"> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** In front of the label, hidden from assistive tech. A ui icon (`PlusIcon`) sizes itself. */
  icon?: ReactNode;
  /** A square, icon-only button. Give it an `aria-label`: the icon is hidden, so without one the
   *  button has no name. */
  iconOnly?: boolean;
  /** Drawn, but nothing is behind it: renders a <span> in the same box. */
  inert?: boolean;
  /** The action is running: keeps focus, shows progress, ignores presses. */
  busy?: boolean;
  /** The label while busy ("Saving…"); the label stays when absent. */
  busyLabel?: ReactNode;
  children?: ReactNode;
}

export function Button({
  variant = "secondary",
  size = "sm",
  icon,
  iconOnly = false,
  inert = false,
  busy = false,
  busyLabel,
  className,
  children,
  type = "button",
  onClick,
  ...rest
}: ButtonProps) {
  const cls = cx(buttonClass(variant, size), iconOnly && ui.btn.iconOnly, className);
  const shown = busy ? <CircleNotchIcon className='oxy-ui-btn__spinner' /> : icon;
  const label = iconOnly ? null : busy && busyLabel !== undefined ? busyLabel : children;
  if (inert) {
    // Plain text to assistive tech: there is no action to name.
    return (
      <span className={cls} data-inert=''>
        {icon}
        {iconOnly ? null : children}
      </span>
    );
  }
  const press = (event: MouseEvent<HTMLButtonElement>) => {
    if (busy) {
      // A busy submit button must not submit the form a second time.
      event.preventDefault();
      return;
    }
    onClick?.(event);
  };
  return (
    <button
      {...rest}
      type={type}
      className={cls}
      // A caller's own `aria-busy` stands while `busy` is unset (Site Scout: the one button of a
      // running set that says so); `busy` adds the spinner and refuses presses on top.
      aria-busy={busy || rest["aria-busy"] || undefined}
      aria-disabled={busy || rest["aria-disabled"] || undefined}
      data-busy={busy ? "" : undefined}
      onClick={press}
    >
      {shown}
      {label}
    </button>
  );
}
