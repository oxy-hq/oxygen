import { Search, X } from "lucide-react";
import { useId, useMemo, useRef } from "react";
import { Link } from "react-router-dom";
import { Input } from "@/components/ui/shadcn/input";
import { cn } from "@/libs/shadcn/utils";
import type { SettingsSection } from "../../settingsRoute";
import { filterNavGroups, type VisibleNavGroup } from "../nav";

export interface SettingsNavProps {
  groups: VisibleNavGroup[];
  /** `null` on the settings index, where nothing is chosen yet. */
  activeSection: SettingsSection | null;
  /** The address of a section. Supplied rather than derived so the rail needs no router
   *  context of its own, and so the org segment is resolved in exactly one place. */
  hrefFor: (section: SettingsSection) => string;
  query: string;
  onQueryChange: (query: string) => void;
  /** Rendered at the bottom of the rail on desktop. */
  footer?: React.ReactNode;
  className?: string;
}

/**
 * The settings rail: a filter, then the nav grouped by what the reader came to do.
 *
 * Shared by both layouts, so the phone's section list and the desktop rail cannot drift apart
 * — they used to be two separate renderings of the same data, one with tiled icons in cards
 * and one with bare icons.
 *
 * Accessibility, all of which the previous rail was missing:
 * - `aria-label` on the landmark, so a screen reader can name it among the dialog's regions.
 * - each list `aria-labelledby` its group heading, so items are heard inside their group;
 *   without it the reader heard 22 flat buttons and two of them said "Members".
 * - `aria-current="page"` on the active item, which was signalled by `data-active` only.
 * - a visible `focus-visible` ring. The rows previously set `outline-none` and replaced it
 *   with nothing, so a keyboard user could not see where they were (WCAG 2.4.7).
 * - arrow-key roving focus, so reaching the content does not mean 22 tab stops (WCAG 2.4.1).
 */
export function SettingsNav({
  groups,
  activeSection,
  hrefFor,
  query,
  onQueryChange,
  footer,
  className
}: SettingsNavProps) {
  const filterId = useId();
  const groupIdPrefix = useId();
  const listRef = useRef<HTMLDivElement>(null);

  const filtered = useMemo(() => filterNavGroups(groups, query), [groups, query]);
  const matchCount = useMemo(() => filtered.reduce((n, g) => n + g.items.length, 0), [filtered]);

  /**
   * The rail's single tab stop, taken from what is **on screen** rather than from the active
   * section. A filter can hide the active section — you are on Notifications and type
   * "crew" — and keying the tab stop to it then left every rendered row at `tabIndex={-1}`,
   * so Tab skipped the whole result list and the arrow keys, which live on the rows, could
   * not be reached either. The filter's own results became unreachable by keyboard.
   */
  const tabStop = useMemo(() => {
    const onScreen = filtered.flatMap((g) => g.items);
    return onScreen.some((i) => i.value === activeSection) ? activeSection : onScreen[0]?.value;
  }, [filtered, activeSection]);

  const focusItem = (index: number) => {
    const buttons = Array.from(
      listRef.current?.querySelectorAll<HTMLAnchorElement>("a[data-nav-item]") ?? []
    );
    buttons.at(index)?.focus();
  };

  /**
   * Up / Down / Home / End across every item in the rail, regardless of group. One row is the
   * tab stop, so Tab enters the rail once and leaves it once; arrows move within it.
   */
  const onKeyDown = (event: React.KeyboardEvent<HTMLAnchorElement>) => {
    const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
    if (!keys.includes(event.key)) return;
    const buttons = Array.from(
      listRef.current?.querySelectorAll<HTMLAnchorElement>("a[data-nav-item]") ?? []
    );
    if (buttons.length === 0) return;
    const current = buttons.indexOf(document.activeElement as HTMLAnchorElement);
    let next: number;
    if (event.key === "Home") next = 0;
    else if (event.key === "End") next = buttons.length - 1;
    else if (event.key === "ArrowDown") next = current < 0 ? 0 : (current + 1) % buttons.length;
    else next = current <= 0 ? buttons.length - 1 : current - 1;
    event.preventDefault();
    focusItem(next);
  };

  return (
    <nav aria-label='Settings sections' className={cn("flex min-h-0 flex-col", className)}>
      <div className='shrink-0 p-3 pb-2'>
        <div className='relative'>
          <Search
            aria-hidden='true'
            className='pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground'
          />
          <Input
            id={filterId}
            // Not `type='search'`: that adds the browser's own clear button beside ours.
            type='text'
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            // Down from the field walks into the results, so the filter and the list it
            // produces are one keyboard path rather than two.
            onKeyDown={(e) => {
              if (e.key !== "ArrowDown") return;
              e.preventDefault();
              focusItem(0);
            }}
            placeholder='Find a setting'
            aria-label='Find a setting'
            data-testid='settings-nav-filter'
            className='h-9 bg-background pr-8 pl-8'
          />
          {query && (
            <button
              type='button'
              onClick={() => onQueryChange("")}
              aria-label='Clear filter'
              className='absolute top-1/2 right-1.5 flex size-6 -translate-y-1/2 items-center justify-center rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:outline-offset-2'
            >
              <X className='size-3.5' />
            </button>
          )}
        </div>
        {/* Announced, not just drawn, so a filter that matches nothing is not silence. */}
        <p aria-live='polite' className='sr-only'>
          {query ? (matchCount === 1 ? "1 setting matches" : `${matchCount} settings match`) : ""}
        </p>
      </div>

      <div ref={listRef} className='min-h-0 flex-1 overflow-y-auto px-3 pb-3'>
        {matchCount === 0 ? (
          <p className='px-2.5 py-6 text-muted-foreground text-sm' data-testid='settings-nav-empty'>
            Nothing matches “{query}”.
          </p>
        ) : (
          <div className='flex flex-col gap-5'>
            {filtered.map((group) => {
              const headingId = `${groupIdPrefix}-${group.label.replace(/\s+/g, "-")}`;
              return (
                <div key={group.label} className='flex flex-col gap-1'>
                  <div className='px-2.5 pb-1'>
                    {/* `t-label` is the design system's own 11px uppercase step
                        (styles/shadcn/typography.css) — the rail used to hand-roll it as
                        `text-xs uppercase tracking-wider`, a size the scale does not have. */}
                    <h3 id={headingId} className='t-label text-muted-foreground'>
                      {group.label}
                    </h3>
                    {group.subtitle && (
                      <p className='truncate text-muted-foreground text-xs'>{group.subtitle}</p>
                    )}
                  </div>
                  <ul aria-labelledby={headingId} className='flex flex-col gap-0.5'>
                    {group.items.map((item) => {
                      const Icon = item.icon;
                      const isActive = item.value === activeSection;
                      return (
                        <li key={item.value}>
                          <Link
                            to={hrefFor(item.value)}
                            data-nav-item
                            onKeyDown={onKeyDown}
                            aria-current={isActive ? "page" : undefined}
                            tabIndex={item.value === tabStop ? 0 : -1}
                            data-active={isActive}
                            data-testid={`settings-nav-${item.value}`}
                            className={cn(
                              "relative flex h-8 w-full items-center gap-2.5 rounded-md pr-2.5 pl-3 text-left text-sm transition-colors",
                              "text-muted-foreground hover:bg-accent hover:text-foreground",
                              // Not background alone: the old active fill was the same token
                              // as hover, so active and hovered looked identical (WCAG 1.4.1).
                              // Weight and the leading bar carry it too.
                              "data-[active=true]:bg-accent data-[active=true]:font-medium data-[active=true]:text-foreground",
                              "before:absolute before:top-1.5 before:bottom-1.5 before:left-0 before:w-0.5 before:rounded-full before:bg-primary before:opacity-0 before:transition-opacity",
                              "data-[active=true]:before:opacity-100",
                              "focus-visible:outline-2 focus-visible:outline-ring focus-visible:outline-offset-1"
                            )}
                          >
                            <Icon
                              className={cn(
                                "size-4 shrink-0",
                                isActive ? "text-foreground" : "text-muted-foreground"
                              )}
                            />
                            <span className='truncate'>{item.label}</span>
                          </Link>
                        </li>
                      );
                    })}
                  </ul>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {footer && <div className='shrink-0 border-border/60 border-t px-4 py-3'>{footer}</div>}
    </nav>
  );
}
