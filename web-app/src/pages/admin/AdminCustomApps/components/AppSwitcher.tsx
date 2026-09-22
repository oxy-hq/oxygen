import { cn } from "@/libs/shadcn/utils";
import type { AppStatus } from "../appStatus";

/**
 * Two small pieces the apps console and the admin palette share: the button that opens
 * the switcher, and the mapping from an app's status to a tone.
 *
 * **The palette itself is deliberately not here.** This file briefly owned a second ⌘K
 * dialog, and the admin console already binds that key — so one keypress opened two
 * stacked dialogs, each filtering its own half of the results. Custom apps are a group
 * in `AdminEntitySearch` now, and the button below asks that one palette to open.
 *
 * The button exists because the fleet is not the only way between apps. `/admin/apps`
 * does list every app, but hopping from one console straight to another should not cost
 * a round trip through it — three letters in the palette, with the console never
 * unmounting behind it. The fleet is for looking at everything; the palette is for going
 * somewhere.
 */
/**
 * A status's tone. `draft` and `quiet` are deliberately `muted` rather than `ok`: neither
 * is a verdict that the app is working, and a green pill beside an app nobody has
 * measured or visited is the kind of false comfort this whole surface is being rebuilt
 * to stop giving.
 */
export function statusTone(s: AppStatus) {
  switch (s) {
    case "down":
      return "danger" as const;
    case "degraded":
    case "not_measured":
      return "warn" as const;
    case "operational":
      return "ok" as const;
    default:
      return "muted" as const;
  }
}

/**
 * The trigger.
 *
 * It deliberately does **not** print `⌘K`. There is only one binding
 * (`AdminEntitySearch`), but the topbar's own Search button already advertises that key
 * on every admin page — so two controls printing the same shortcut read as a conflict
 * even though pressing it opens one palette. One advertisement, and this button says
 * what it does instead.
 */
export const AppSwitcherTrigger = ({
  onClick,
  className
}: {
  onClick: () => void;
  className?: string;
}) => (
  <button
    type='button'
    onClick={onClick}
    data-testid='apps-switcher-trigger'
    className={cn(
      "inline-flex h-7 items-center gap-1.5 rounded-md border border-border/60 px-2 text-muted-foreground text-xs transition-colors hover:text-foreground",
      className
    )}
  >
    Switch app
  </button>
);
