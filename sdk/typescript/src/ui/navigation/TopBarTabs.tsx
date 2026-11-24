// Started from pokehouse-oxy:apps/bookkeeping/src/components/ui/TopBarTabs.tsx (INVENTORY: the
// top-bar placement with a More menu, tested in `TopBarTabs.test.tsx`; decision 29's measured
// fit). Kept: the split from a hidden copy of every tab at its widest, the open screen never in
// the menu, the disclosure (not an ARIA menu) of real links, ↓/↑/Home/End/Escape, a press
// elsewhere or Tab leaving closes it, a hidden tab's count riding on More. Changed: the classes
// are the reference's `topBarTab` recipes, the strip shows from `topBarFrom` (the reference's
// breakpoint), and the More button's name is a prop.
//
// The screen strip in the shell's top bar, after the breadcrumb (`TopBarLeft`). The tabs that fit
// stay in the bar and the rest go behind a More button (Phosphor DotsThree) at the end of the
// run (`splitTabs`). The split follows the bar's width as the window changes. The menu floats
// over the page, so it takes the one elevation, the overlay's (`--shadow-overlay`).

import {
  type KeyboardEvent,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState
} from "react";
import { cx } from "../cx";
import { ui } from "../recipes";
import { DotsThreeIcon } from "./icons";
import {
  dataAttributes,
  isOpen,
  linkClick,
  type ParentTab,
  type ParentTabsProps
} from "./parentTab";
import { splitTabs, type TabSplit } from "./tabOverflow";

export type TopBarTabsProps<K extends string> = Omit<ParentTabsProps<K>, "placement">;

function sameSplit(a: TabSplit | null, b: TabSplit): boolean {
  return !!a && a.shown.join() === b.shown.join() && a.hidden.join() === b.hidden.join();
}

const countOf = <K extends string>(tabs: readonly ParentTab<K>[]) =>
  tabs.reduce((n, t) => n + (t.count ?? 0), 0);

/** The split, measured from the hidden copy and kept current as the bar's room changes. */
function useSplit(active: number) {
  const navRef = useRef<HTMLElement>(null);
  const rowRef = useRef<HTMLDivElement>(null);
  // Until measured, every tab: what a page with no layout draws too.
  const [split, setSplit] = useState<TabSplit | null>(null);
  const measure = useCallback(() => {
    const nav = navRef.current;
    const row = rowRef.current;
    if (!nav || !row) return;
    const copies = [...row.children] as HTMLElement[];
    const more = copies.pop();
    if (!more) return;
    const next = splitTabs(
      copies.map((el) => el.getBoundingClientRect().width),
      more.getBoundingClientRect().width,
      nav.clientWidth,
      active
    );
    setSplit((prev) => (sameSplit(prev, next) ? prev : next));
  }, [active]);
  useLayoutEffect(() => {
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    if (navRef.current) ro.observe(navRef.current);
    if (rowRef.current) ro.observe(rowRef.current);
    return () => ro.disconnect();
  }, [measure]);
  return { navRef, rowRef, split };
}

export function TopBarTabs<K extends string>({
  label,
  tabs,
  current,
  onNavigate,
  topBarFrom = "md",
  moreLabel = "More screens",
  className,
  ...rest
}: TopBarTabsProps<K>) {
  const active = tabs.findIndex((t) => isOpen(t, current));
  const { navRef, rowRef, split } = useSplit(active);
  const shown = split ? split.shown : tabs.map((_, i) => i);
  const hidden = split ? split.hidden : [];

  return (
    <nav
      {...dataAttributes(rest)}
      ref={navRef}
      aria-label={label}
      data-placement='topbar'
      className={cx(
        ui.topBarTabs,
        "oxy-ui-parent-tabs",
        `oxy-ui-parent-tabs--topbar-${topBarFrom}`,
        className
      )}
    >
      {shown.map((i) => {
        const tab = tabs[i];
        if (!tab) return null;
        const on = isOpen(tab, current);
        return (
          <a
            key={tab.id}
            href={tab.href}
            aria-current={on ? "page" : undefined}
            className={cx(
              ui.topBarTab,
              "oxy-ui-parent-tabs__tab",
              on ? ui.topBarTabOn : ui.topBarTabOff
            )}
            onClick={linkClick(tab, onNavigate)}
          >
            {tab.icon}
            {tab.label}
            {tab.trailing}
          </a>
        );
      })}
      {hidden.length > 0 && (
        <MoreMenu
          items={hidden.map((i) => tabs[i]).filter(Boolean)}
          onNavigate={onNavigate}
          label={moreLabel}
        />
      )}
      {/* Every tab at its widest (the selected weight), and the More button, out of sight and
          out of the accessibility tree: what the split is measured from. */}
      <div className='oxy-ui-parent-tabs__measure' aria-hidden='true' inert>
        <div ref={rowRef} className='oxy-ui-parent-tabs__measure-row'>
          {tabs.map((tab) => (
            <span
              key={tab.id}
              className={cx(ui.topBarTab, ui.topBarTabOn, "oxy-ui-parent-tabs__measure-tab")}
            >
              {tab.icon}
              {tab.label}
              {tab.trailing}
            </span>
          ))}
          <span className={cx(ui.topBarTab, "oxy-ui-parent-tabs__measure-more")}>
            <DotsThreeIcon className='oxy-ui-parent-tabs__more-icon' />
            {countOf(tabs) > 0 && (
              <span className={cx(ui.tabCount, ui.tabCountNeutral)}>{countOf(tabs)}</span>
            )}
          </span>
        </div>
      </div>
    </nav>
  );
}

/** The menu's links, in order. */
const linksIn = (list: HTMLUListElement | null) => [
  ...(list?.querySelectorAll<HTMLAnchorElement>("a") ?? [])
];

/** Keys on the open list: ↓/↑ move and wrap, Home/End jump, from the link that has focus. */
const LIST_KEYS: Record<string, (at: number, n: number) => number> = {
  ArrowDown: (at) => at + 1,
  ArrowUp: (at, n) => (at < 0 ? n - 1 : at - 1),
  Home: () => 0,
  End: (_at, n) => n - 1
};

function MoreMenu<K extends string>({
  items,
  onNavigate,
  label
}: {
  items: ParentTab<K>[];
  onNavigate: ParentTabsProps<K>["onNavigate"];
  label: string;
}) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const menuId = useId();
  // Where focus goes once the list is drawn: the first or last link, when the keyboard opened it.
  const focusOnOpen = useRef<"first" | "last" | null>(null);
  const links = () => linksIn(listRef.current);

  useLayoutEffect(() => {
    if (!open || !focusOnOpen.current) return;
    const all = linksIn(listRef.current);
    (focusOnOpen.current === "first" ? all[0] : all[all.length - 1])?.focus();
    focusOnOpen.current = null;
  }, [open]);

  // A press anywhere else closes it.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [open]);

  const close = () => {
    setOpen(false);
    buttonRef.current?.focus();
  };

  const onButtonKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const end = e.key === "ArrowDown" ? "first" : "last";
      if (!open) {
        focusOnOpen.current = end;
        setOpen(true);
        return;
      }
      const all = links();
      (end === "first" ? all[0] : all[all.length - 1])?.focus();
    } else if (e.key === "Escape" && open) {
      e.preventDefault();
      close();
    }
  };

  const onListKey = (e: KeyboardEvent<HTMLUListElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    const move = LIST_KEYS[e.key];
    if (!move) return;
    e.preventDefault();
    const all = links();
    const at = all.indexOf(document.activeElement as HTMLAnchorElement);
    all[(move(at, all.length) + all.length) % all.length]?.focus();
  };

  const count = countOf(items);
  const names = items.map((t) => t.label).join(", ");
  const counted = items
    .filter((t) => (t.count ?? 0) > 0)
    .map((t) => `${t.label}: ${t.countLabel ?? t.count}`);

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: blur bubbles from the button and the links inside; the wrapper takes no input of its own.
    <div
      ref={wrapRef}
      className='oxy-ui-parent-tabs__more-wrap'
      // Tabbing out of the menu closes it.
      onBlur={(e) => {
        if (open && !wrapRef.current?.contains(e.relatedTarget as Node | null)) setOpen(false);
      }}
    >
      <button
        ref={buttonRef}
        type='button'
        className={cx(ui.topBarTab, ui.topBarTabOff, "oxy-ui-parent-tabs__more")}
        aria-expanded={open}
        aria-controls={menuId}
        aria-label={count > 0 ? `${label}, ${counted.join(", ")}` : label}
        title={`${label}: ${names}`}
        onClick={(e) => {
          // A click from the keyboard (Enter, Space) carries no pointer detail: that opening puts
          // focus on the first screen, as ↓ does.
          if (!open && e.detail === 0) focusOnOpen.current = "first";
          setOpen((v) => !v);
        }}
        onKeyDown={onButtonKey}
      >
        <DotsThreeIcon className='oxy-ui-parent-tabs__more-icon' />
        {count > 0 && (
          <span className={cx(ui.tabCount, ui.tabCountNeutral)} aria-hidden='true'>
            {count}
          </span>
        )}
      </button>
      {open && (
        <ul
          id={menuId}
          ref={listRef}
          className='oxy-ui-parent-tabs__menu'
          aria-label={label}
          onKeyDown={onListKey}
        >
          {items.map((tab) => (
            <li key={tab.id} className='oxy-ui-parent-tabs__menu-entry'>
              <a
                href={tab.href}
                className={cx(ui.focusRingInset, "oxy-ui-parent-tabs__menu-item")}
                onClick={linkClick(tab, onNavigate, () => setOpen(false))}
              >
                {tab.icon}
                <span className='oxy-ui-parent-tabs__menu-label'>{tab.label}</span>
                {tab.trailing}
              </a>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
