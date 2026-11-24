import { createContext, useContext, useId, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/libs/shadcn/utils";

/**
 * The header every settings section gets, drawn by the shell rather than by the section.
 *
 * **Why the shell owns it.** Each section used to render its own `SectionHeader`, and the
 * consequences were all visible on screen: General, Billing and Integration rendered none at
 * all, so those pages opened with no heading; `workspace/Members` repeated the same call in
 * its loading, error and loaded branches, so the title appeared only after the fetch and the
 * page jumped; and the title, the nav label and the mobile header could disagree, which they
 * did ("Oxy access" in the nav, "Oxy staff access" on the page). Reading the title and the
 * description off the nav item makes all three the same string by construction.
 *
 * A section that needs buttons in the header renders them through {@link SectionActions},
 * which portals into the slot below. Four sections do.
 */

/**
 * `undefined` means no `SectionChrome` above us at all — a section rendered on its own, which
 * is how the section tests mount them. Then {@link SectionActions} renders in place rather
 * than portalling into a header that does not exist. A provider supplies `{ slot }`, where a
 * null slot is only the first commit before the ref attaches.
 */
const ActionsSlotContext = createContext<{ slot: HTMLElement | null } | undefined>(undefined);
const HeadingIdContext = createContext<string | undefined>(undefined);

/** The id of the heading the content region is labelled by, for `aria-labelledby`. */
export function useSectionHeadingId(): string | undefined {
  return useContext(HeadingIdContext);
}

export interface SectionChromeProps {
  title: string;
  description?: string;
  children: React.ReactNode;
  /**
   * Focused when the section changes so a keyboard or screen-reader user is moved to the new
   * page instead of being left on the nav button they just pressed, with nothing announced.
   */
  headingRef?: React.Ref<HTMLHeadingElement>;
  /**
   * Keep the heading for screen readers but take it off the screen. Set on a phone, where
   * the bar at the top already names the section and showing it twice wastes a third of the
   * first screenful. The description stays visible.
   */
  titleVisuallyHidden?: boolean;
}

/**
 * Title, description, an actions slot, then the section's own content.
 *
 * `h2` under the dialog's `h1`, so the headings inside a section (`h3`) sit at the right depth.
 * The old header was an `h3` with no `h2` anywhere above it.
 */
export function SectionChrome({
  title,
  description,
  children,
  headingRef,
  titleVisuallyHidden = false
}: SectionChromeProps) {
  const [actionsSlot, setActionsSlot] = useState<HTMLElement | null>(null);
  const headingId = useId();
  // A stable object identity per slot, so consumers re-render when the ref attaches and
  // not on every render of this component.
  const slotValue = useMemo(() => ({ slot: actionsSlot }), [actionsSlot]);

  return (
    <HeadingIdContext.Provider value={headingId}>
      <div className='flex min-w-0 flex-col gap-6'>
        <div className='flex min-w-0 flex-col gap-3 sm:flex-row sm:items-start sm:justify-between sm:gap-6'>
          <div className='flex min-w-0 flex-col gap-1.5'>
            {/* `tabIndex={-1}` so the shell can move focus here without putting the
                heading in the tab order. `outline-none` is safe on a programmatic
                focus target that is not interactive. */}
            <h2
              id={headingId}
              ref={headingRef}
              tabIndex={-1}
              className={cn(
                "min-w-0 break-words font-medium text-foreground text-xl tracking-tight outline-none",
                titleVisuallyHidden && "sr-only"
              )}
            >
              {title}
            </h2>
            {description && (
              <p className='max-w-prose text-muted-foreground text-sm'>{description}</p>
            )}
          </div>
          <div
            ref={setActionsSlot}
            className='flex flex-wrap items-center gap-2 empty:hidden sm:shrink-0'
          />
        </div>
        <ActionsSlotContext.Provider value={slotValue}>{children}</ActionsSlotContext.Provider>
      </div>
    </HeadingIdContext.Provider>
  );
}

/**
 * Renders its children into the section header's actions slot, from anywhere inside the
 * section — so a section's "New team" button sits beside the title the shell drew, without
 * the section having to own a header.
 *
 * With no `SectionChrome` above it, the children render where they stand. That keeps a
 * section usable on its own, which is how its tests mount it, and means losing the shell
 * degrades to a button in the body rather than a button that is gone.
 */
export function SectionActions({ children }: { children: React.ReactNode }) {
  const ctx = useContext(ActionsSlotContext);
  if (!ctx) return <>{children}</>;
  return ctx.slot ? createPortal(children, ctx.slot) : null;
}
