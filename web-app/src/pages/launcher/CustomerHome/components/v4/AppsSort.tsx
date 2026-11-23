import { CaretDown, Check } from "@phosphor-icons/react";
import { useEffect, useRef, useState } from "react";
import { focusRing, iconInline, note, textMd } from "@/components/ui/reference/tokens";
import { cn } from "@/libs/shadcn/utils";
import type { CustomAppSummary } from "@/types/apps";

export type AppSort = "recent" | "alpha";

const LABEL: Record<AppSort, string> = { recent: "Recently used", alpha: "Alphabetical" };

/**
 * The apps in the chosen order. Recently used: the ones this person last opened
 * here, newest first, then the rest in the order the server lists them.
 * Alphabetical: by name, case and accents aside.
 */
export function sortApps(
  apps: readonly CustomAppSummary[],
  recent: readonly CustomAppSummary[],
  sort: AppSort
): CustomAppSummary[] {
  if (sort === "alpha") {
    return [...apps].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" })
    );
  }
  const opened = new Set(recent.map((a) => a.id));
  return [...recent, ...apps.filter((a) => !opened.has(a.id))];
}

/**
 * The apps' sort, at the title line's right end: the current order in quiet text
 * with a caret, opening a menu of the two orders with a check on the current one.
 * Closes on a choice, a press outside it, or Escape.
 *
 * Transcribed from oxy-hq/reference-app `src/views/parts/HomeApps.tsx`
 * (`AppsSort`, branch design/home-match-oxygen).
 */
export function AppsSort({ sort, onSort }: { sort: AppSort; onSort: (sort: AppSort) => void }) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [open]);
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: a delegate for Escape from the menu's own buttons
    <div
      ref={box}
      className='relative'
      onKeyDown={(e) => {
        if (e.key === "Escape") setOpen(false);
      }}
    >
      <button
        type='button'
        aria-haspopup='menu'
        aria-expanded={open}
        aria-label={`Sort apps: ${LABEL[sort]}`}
        onClick={() => setOpen((o) => !o)}
        data-testid='home-apps-sort'
        className={cn(
          "group -my-1 -mr-2 flex items-center gap-1.5 px-2 py-1 hover:text-foreground",
          open && "text-foreground",
          note,
          focusRing
        )}
      >
        <span className='whitespace-nowrap'>{LABEL[sort]}</span>
        <CaretDown
          aria-hidden
          className={cn(
            iconInline,
            "transition-transform motion-reduce:transition-none",
            open && "rotate-180"
          )}
        />
      </button>
      {open && (
        <div
          role='menu'
          className='absolute top-full right-0 z-20 mt-1 min-w-40 border border-border-subtle bg-card py-1 shadow-lg'
        >
          {(Object.keys(LABEL) as AppSort[]).map((key) => (
            <button
              key={key}
              type='button'
              role='menuitemradio'
              aria-checked={sort === key}
              onClick={() => {
                onSort(key);
                setOpen(false);
              }}
              data-testid={`home-apps-sort-${key}`}
              className={cn(
                "flex w-full items-center gap-2 px-3 py-1.5 text-left text-foreground hover:bg-row-hover focus-visible:bg-row-hover focus-visible:outline-none",
                textMd
              )}
            >
              <span className='flex size-3.5 shrink-0 items-center justify-center'>
                {sort === key && <Check className={iconInline} aria-hidden />}
              </span>
              {LABEL[key]}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
