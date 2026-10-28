import { Menu } from "lucide-react";
import { Fragment, type ReactNode, useEffect, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Sheet,
  SheetClose,
  SheetContent,
  SheetTitle,
  SheetTrigger
} from "@/components/ui/shadcn/sheet";
import { cn } from "@/libs/shadcn/utils";
import { type RailItem, RailItemIcon } from "./ShellRail";

const rowClasses = (active?: boolean) =>
  cn(
    // h-11: a 44px touch target. Full opacity, unlike the rail's faded idle
    // tiles — the label carries the hierarchy here, not the fade.
    "flex h-11 w-full items-center gap-3 rounded-md px-3 text-left text-sm",
    active ? "border border-border-strong bg-muted font-medium" : "hover:bg-muted"
  );

function NavRow({ item }: { item: RailItem }) {
  const content = (
    <>
      {/* Decorative: the label names the row, and a letter mark would be read
          into it ("BBookkeeping"). */}
      <span aria-hidden='true' className='flex h-6 w-6 shrink-0 items-center justify-center'>
        <RailItemIcon item={item} />
      </span>
      <span className='truncate'>{item.label}</span>
    </>
  );
  // SheetClose closes the drawer on every pick; for an anchor it adds a click
  // handler and the browser still follows the link.
  return (
    <SheetClose asChild>
      {item.href ? (
        <a
          href={item.href}
          data-testid={`nav-sheet-${item.testId}`}
          aria-current={item.active ? "page" : undefined}
          className={rowClasses(item.active)}
        >
          {content}
        </a>
      ) : (
        <button
          type='button'
          onClick={item.onSelect}
          data-testid={`nav-sheet-${item.testId}`}
          aria-current={item.active ? "page" : undefined}
          className={rowClasses(item.active)}
        >
          {content}
        </button>
      )}
    </SheetClose>
  );
}

function Divider() {
  return <div className='mx-3 my-2 h-px bg-sidebar-border' />;
}

/** The rail's mobile form: a menu button (placed in the top bar) opening a
 *  left drawer of the same entries, each with its name. On a touch screen
 *  the rail's only labels are hover tooltips, which never show, so below `md`
 *  the shell swaps the rail for this. Same props as `ShellRail`. */
export function ShellNavSheet({
  title,
  top,
  groups,
  footerItems,
  bottom
}: {
  title: string;
  top?: ReactNode;
  groups: RailItem[][];
  footerItems?: RailItem[];
  bottom?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  // Widening past `md` brings the rail back; an open drawer would only cover it.
  useEffect(() => {
    if (!open) return;
    const wide = window.matchMedia?.("(min-width: 768px)");
    const onWide = () => {
      if (wide?.matches) setOpen(false);
    };
    wide?.addEventListener?.("change", onWide);
    return () => wide?.removeEventListener?.("change", onWide);
  }, [open]);
  return (
    <Sheet open={open} onOpenChange={setOpen}>
      {/* SheetTrigger, not a bare button: Radix returns focus on close only to
          its own trigger, so without it a keyboard user lands on <body>. */}
      <SheetTrigger asChild>
        <Button
          variant='ghost'
          size='icon'
          className='-ml-1 h-9 w-9 shrink-0'
          data-testid='nav-sheet-trigger'
          aria-label='Open navigation'
        >
          <Menu className='h-5 w-5' />
        </Button>
      </SheetTrigger>
      <SheetContent
        side='left'
        // No description: the title names the drawer and the rows are labeled.
        aria-describedby={undefined}
        data-testid='nav-sheet'
        className='w-3/4 max-w-72 gap-0 bg-sidebar-background p-0'
      >
        <div className='flex h-12 shrink-0 items-center gap-3 border-b px-3 pr-10'>
          {top}
          <SheetTitle className='truncate text-sm'>{title}</SheetTitle>
        </div>
        <nav className='flex min-h-0 flex-1 flex-col overflow-y-auto p-2'>
          {groups.map((group, i) => (
            <Fragment key={group[0]?.key ?? i}>
              {i > 0 && <Divider />}
              {group.map((item) => (
                <NavRow key={item.key} item={item} />
              ))}
            </Fragment>
          ))}
          {footerItems && footerItems.length > 0 && (
            <div className='mt-auto pt-2'>
              <Divider />
              {footerItems.map((item) => (
                <NavRow key={item.key} item={item} />
              ))}
            </div>
          )}
        </nav>
        {bottom && (
          <div className='flex shrink-0 items-center gap-2 border-t px-3 py-2'>{bottom}</div>
        )}
      </SheetContent>
    </Sheet>
  );
}
