import { SidebarSimple } from "@phosphor-icons/react";
import { Fragment, type ReactNode, useState } from "react";
import {
  DESIGN_REFERENCE,
  focusRing,
  iconControl,
  sectionLinkOff,
  sectionLinkOn,
  textXs
} from "@/components/ui/reference/tokens";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { cn } from "@/libs/shadcn/utils";
import { type RailItem, RailMark } from "./RailItem";

/**
 * The rail, the reference design's: it opens and closes from
 * a toggle at its head and narrows the page while open, rather than opening on
 * hover over it (`ShellRail`, which stays as it is with the flag off).
 *
 * - Closed, 48px: marks only. Each names itself in a hint to its right on
 *   hover, and to a screen reader always (the name is in the row, hidden).
 * - The customer's logo heads it. While the pointer is anywhere on the rail
 *   (or the toggle has keyboard focus) the logo turns into the toggle.
 * - Open, 224px: each row shows its name; the head shows the logo, the
 *   customer's name and the toggle at the far end; the apps get a title.
 * - Whether it is open is remembered in this browser.
 *
 * Transcribed from oxy-hq/reference-app `src/views/parts/HomeRail.tsx`
 * (branch design/home-match-oxygen) onto the shell's real entries.
 */

/** Where this browser remembers the rail open: the SDK rail's key and format
 *  (`"true"` / `"false"`, `@oxy-hq/sdk/shell` `RAIL_EXPANDED_KEY`), so an app
 *  served from this origin opens its rail as it was left here, and back. */
export const RAIL_EXPANDED_KEY = "oxy.shell-rail.expanded";

function readOpen(): boolean {
  try {
    return window.localStorage.getItem(RAIL_EXPANDED_KEY) === "true";
  } catch {
    return false;
  }
}

function writeOpen(open: boolean) {
  try {
    window.localStorage.setItem(RAIL_EXPANDED_KEY, String(open));
  } catch {
    // Private mode or full storage: the choice lasts for this page only.
  }
}

/** The rail's open state, remembered in this browser. */
export function useRailOpen(): [boolean, () => void] {
  const [open, setOpen] = useState(readOpen);
  const toggle = () =>
    setOpen((was) => {
      writeOpen(!was);
      return !was;
    });
  return [open, toggle];
}

/** A closed rail's hint, to the right of the control; open, the name is on the row, so none. */
function Hint({ label, show, children }: { label: string; show: boolean; children: ReactNode }) {
  if (!show) return children;
  return (
    <Tooltip delayDuration={300}>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side='right' sideOffset={6} className={DESIGN_REFERENCE}>
        {label}
      </TooltipContent>
    </Tooltip>
  );
}

/** One entry: a 32px square while closed, a row with its name once open. */
function Entry({ item, open }: { item: RailItem; open: boolean }) {
  const className = cn(
    "flex h-8 shrink-0 items-center text-left transition-colors",
    open ? "w-full justify-start gap-2 px-1.5" : "w-8 justify-center",
    item.active ? sectionLinkOn : sectionLinkOff,
    focusRing
  );
  const content = (
    <>
      <span className='flex size-5 shrink-0 items-center justify-center'>
        <RailMark item={item} />
      </span>
      <span className={open ? "min-w-0 flex-1 truncate text-sm" : "sr-only"}>{item.label}</span>
    </>
  );
  const common = {
    "data-testid": item.testId,
    "aria-current": item.active ? ("page" as const) : undefined,
    className
  };
  return (
    <Hint label={item.label} show={!open}>
      {item.href ? (
        <a href={item.href} {...common}>
          {content}
        </a>
      ) : (
        <button type='button' onClick={item.onSelect} {...common}>
          {content}
        </button>
      )}
    </Hint>
  );
}

/** The open / close control, in the 32px cell an entry takes. */
function Toggle({
  open,
  onToggle,
  children
}: {
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  const label = open ? "Collapse sidebar" : "Expand sidebar";
  return (
    <Hint label={label} show>
      <button
        type='button'
        onClick={onToggle}
        aria-label={label}
        aria-expanded={open}
        data-testid='shell-rail-toggle'
        className={cn(
          "group/toggle flex size-8 shrink-0 items-center justify-center text-subtle-foreground transition-colors hover:bg-row-hover hover:text-foreground",
          focusRing
        )}
      >
        {children}
      </button>
    </Hint>
  );
}

/** A hairline between the rail's groups. */
function Divider({ open }: { open: boolean }) {
  return (
    <span
      aria-hidden
      className={cn("my-1 h-px shrink-0 bg-border-subtle", open ? "w-full" : "w-5")}
    />
  );
}

/** The head: the logo, which turns into the toggle on hover; open, the name and the toggle beside it. */
function Head({
  open,
  onToggle,
  logo,
  name
}: {
  open: boolean;
  onToggle: () => void;
  logo: ReactNode;
  name: string;
}) {
  const icon = <SidebarSimple className={iconControl} aria-hidden />;
  if (open) {
    return (
      <div className='flex h-8 w-full shrink-0 items-center gap-2 pl-1'>
        <span className='flex size-7 shrink-0 items-center justify-center'>{logo}</span>
        <span
          className='min-w-0 flex-1 truncate font-medium text-foreground text-sm'
          data-testid='shell-rail-name'
        >
          {name}
        </span>
        <Toggle open onToggle={onToggle}>
          {icon}
        </Toggle>
      </div>
    );
  }
  return (
    <Toggle open={false} onToggle={onToggle}>
      <span className='flex group-hover/rail:hidden group-focus-visible/toggle:hidden'>{logo}</span>
      <span className='hidden group-hover/rail:flex group-focus-visible/toggle:flex'>{icon}</span>
    </Toggle>
  );
}

export function ToggleRail({
  open,
  onToggle,
  logo,
  name,
  groups,
  groupTitles,
  footerItems,
  bottom
}: {
  open: boolean;
  onToggle: () => void;
  /** The customer's logo, 28px: a customer's logo is usually a badge with its own margin,
   *  so at the design mark's 20px it read much smaller than the mark (Mars, 23 Nov). */
  logo: ReactNode;
  /** The customer's name, beside the logo while open. */
  name: string;
  /** The rail's groups, top down, a hairline between them. The last one scrolls. */
  groups: RailItem[][];
  /** A title per group, by index, shown above it while open. */
  groupTitles?: (string | undefined)[];
  /** Pinned to the foot: the system zone. */
  footerItems?: RailItem[];
  /** Under the system zone: the account menu. */
  bottom?: ReactNode;
}) {
  const align = open ? "items-stretch" : "items-center";
  const shown = groups.filter((g) => g.length > 0);
  return (
    <nav
      aria-label='Oxygen'
      data-testid='shell-rail'
      data-open={open || undefined}
      className={cn(
        DESIGN_REFERENCE,
        // `overflow-x-clip`, not `hidden`: the rows are cut while the width eases, and the
        // column does not become a scroll container of its own.
        "group/rail h-full shrink-0 overflow-x-clip border-border-subtle border-r bg-sidebar transition-[width] duration-200 ease-out motion-reduce:transition-none",
        open ? "w-56" : "w-12"
      )}
    >
      <div className={cn("flex h-full flex-col gap-1 py-2", open ? "px-2" : "px-1", align)}>
        <Head open={open} onToggle={onToggle} logo={logo} name={name} />
        {shown.map((group, i) => {
          const title = groupTitles?.[groups.indexOf(group)];
          const last = i === shown.length - 1;
          return (
            <Fragment key={group[0]?.key ?? i}>
              {i > 0 && <Divider open={open} />}
              {open && title && (
                <div
                  className={cn(
                    textXs,
                    "shrink-0 truncate px-1.5 pt-2 pb-1 text-disabled-foreground"
                  )}
                >
                  {title}
                </div>
              )}
              <div
                role='group'
                aria-label={title}
                className={cn(
                  "flex flex-col gap-1",
                  // The apps can be many: the last group scrolls, the head and foot stay.
                  last && "min-h-0 overflow-y-auto overflow-x-hidden",
                  align
                )}
              >
                {group.map((item) => (
                  <Entry key={item.key} item={item} open={open} />
                ))}
              </div>
            </Fragment>
          );
        })}
        <div className={cn("mt-auto flex shrink-0 flex-col gap-1 pt-2", align)}>
          {footerItems && footerItems.length > 0 && (
            <>
              <Divider open={open} />
              {footerItems.map((item) => (
                <Entry key={item.key} item={item} open={open} />
              ))}
            </>
          )}
          {bottom}
        </div>
      </div>
    </nav>
  );
}
