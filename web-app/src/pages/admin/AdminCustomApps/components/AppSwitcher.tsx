import { cn } from "@/libs/shadcn/utils";

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
// The status → tone mapping lives beside the status model in `appStatus.ts` now, where
// the lifecycle verdict also reads it; re-exported here because every console surface
// already imports it from this module.
export { statusTone } from "../appStatus";

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
      "inline-flex h-7 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md border border-border/60 px-2 text-muted-foreground text-xs transition-colors hover:text-foreground",
      className
    )}
  >
    Switch app
  </button>
);
