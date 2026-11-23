// The icon rail — sole chrome of the workspace shell. Keep it looking and behaving
// like Oxygen's own rail (web-app/src/components/Shell/ToggleRail.tsx): the same
// widths, the same toggle, the same remembered state (`RAIL_EXPANDED_KEY`), the
// same square rows on the reference design's fills, and each app drawn as its grey
// mark through the same cutter (`railGlyph.ts`).

import { Fragment, type ReactNode, useState } from "react";
import { cx } from "./cx";
import { useNavSheetLayout } from "./navSheetQuery";
import { ShellPortalContext, useShellPortalContainer } from "./portal";
import { useRailGlyph } from "./railGlyph";
import { ShellTooltip } from "./Tooltip";

/** One rail entry. Provide `imageUrl` (custom app mark, falls back to
 *  `letter` on load error), `icon` (any SVG node), or `letter`.
 *  `href` renders a real anchor (full-page nav — custom apps live outside
 *  the SPA); `onSelect` renders a button (SPA nav, wired by the caller). */
export type RailItem = {
  key: string;
  label: string;
  /** Hover-tooltip text; defaults to `label`. Use for a richer description
   *  while keeping `label` short for the aria-label. */
  tooltip?: string;
  testId?: string;
  active?: boolean;
  icon?: ReactNode;
  letter?: string;
  imageUrl?: string;
  href?: string;
  onSelect?: () => void;
};

/**
 * An app's mark as Oxygen's rail draws it: no colour of its own, the row's text
 * colour (grey, the current one darker) through the mark cut out of its icon
 * (`railGlyph.ts`, the one cutter Oxygen's web-app copies). Nothing while the cut
 * is made, so the coloured icon never flashes; the app's letter, in the same
 * colour, when there is no mark to cut.
 */
function AppGlyph({ url, letter }: { url: string; letter?: string }) {
  const mask = useRailGlyph(url);
  if (mask === null) {
    return (
      <span data-glyph='letter' className='oxy-rail__item-letter'>
        {letter}
      </span>
    );
  }
  if (mask === undefined) return <span data-glyph='pending' className='oxy-rail__item-glyph' />;
  const image = `url("${mask}")`;
  return (
    <span
      data-glyph='mark'
      className='oxy-rail__item-glyph oxy-rail__item-glyph--ready'
      style={{ maskImage: image, WebkitMaskImage: image }}
    />
  );
}

/** An item's mark: an app as its grey mark (`AppGlyph`), else its icon, else its
 *  letter. Shared with `NavSheet`. */
export function RailItemMark({ item }: { item: RailItem }) {
  if (item.imageUrl) return <AppGlyph url={item.imageUrl} letter={item.letter} />;
  return item.icon ?? <span className='oxy-rail__item-letter'>{item.letter}</span>;
}

/** Where the rail remembers how it was left, per browser, as ChatGPT's and
 *  Gemini's sidebars do. The web-app's rail uses the same key, so where an app
 *  is served from Oxygen's own origin (`/customer-apps/…`) the rail stays as the
 *  person left it moving between Oxygen and its apps. */
export const RAIL_EXPANDED_KEY = "oxy.shell-rail.expanded";

function readExpanded(): boolean {
  try {
    return (
      typeof window !== "undefined" && window.localStorage.getItem(RAIL_EXPANDED_KEY) === "true"
    );
  } catch {
    // Storage blocked (a private window, a sandboxed frame): the rail starts closed.
    return false;
  }
}

function writeExpanded(expanded: boolean) {
  try {
    window.localStorage.setItem(RAIL_EXPANDED_KEY, String(expanded));
  } catch {
    // Storage blocked: the rail still opens and closes, it is just not remembered.
  }
}

/** The rail's open state, read once from the browser and written on every change, and
 *  whether the nav sheet's query holds (`narrow`). Narrow, the rail is never open: there it is
 *  drawn only for an app that keeps it (`hideTopBar`, or a `ShellRail` the app draws itself),
 *  and 14rem would leave a phone a sliver. The remembered choice is the desk's, so narrow
 *  leaves it as it is. */
function useRailExpanded(): { expanded: boolean; narrow: boolean; toggle: () => void } {
  const [open, setOpen] = useState(readExpanded);
  const narrow = useNavSheetLayout();
  const toggle = () =>
    setOpen((was) => {
      writeExpanded(!was);
      return !was;
    });
  return { expanded: open && !narrow, narrow, toggle };
}

/** Phosphor's SidebarSimple at the regular weight, the toggle Oxygen's rail and the
 *  reference design draw, the same open or closed (the SDK carries no icon library,
 *  so its path is inlined). */
function SidebarIcon(_props: { open: boolean }) {
  return (
    <svg width='16' height='16' viewBox='0 0 256 256' fill='currentColor' aria-hidden='true'>
      <path d='M216,40H40A16,16,0,0,0,24,56V200a16,16,0,0,0,16,16H216a16,16,0,0,0,16-16V56A16,16,0,0,0,216,40ZM40,56H80V200H40ZM216,200H96V56H216V200Z' />
    </svg>
  );
}

/** The expand / collapse control, in the head's 32px cell. Closed, the cell shows
 *  `mark` (the workspace's) and turns into the sidebar icon while the pointer is
 *  on the rail or the toggle has keyboard focus: the cue that the mark is the way
 *  in. The swap changes only the icon; nothing opens until the toggle is pressed.
 *  One element in both states, so keyboard focus stays on it as the rail opens. */
function RailToggle({
  expanded,
  onToggle,
  mark
}: {
  expanded: boolean;
  onToggle: () => void;
  mark?: ReactNode;
}) {
  const label = expanded ? "Collapse sidebar" : "Expand sidebar";
  return (
    <ShellTooltip content={label}>
      <button
        type='button'
        onClick={onToggle}
        aria-label={label}
        aria-expanded={expanded}
        data-testid='shell-rail-toggle'
        className={cx("oxy-rail__item oxy-rail__toggle", !!mark && "oxy-rail__toggle--mark")}
      >
        {mark ? (
          <>
            <span className='oxy-rail__toggle-mark'>{mark}</span>
            <span className='oxy-rail__toggle-icon'>
              <SidebarIcon open={expanded} />
            </span>
          </>
        ) : (
          <SidebarIcon open={expanded} />
        )}
      </button>
    </ShellTooltip>
  );
}

/** The head. Closed, one cell: the toggle carrying the mark. Open: the mark, `title` and the
 *  toggle, which keeps its place among the children (the `false` holes hold it), so it stays one
 *  element and keeps keyboard focus. Narrow, the rail is 2.32's: `top` is plain branding and
 *  there is no toggle, so a tap on the mark still does nothing on a phone. */
function RailHead({
  top,
  title,
  expanded,
  narrow,
  onToggle
}: {
  top?: ReactNode;
  title?: string;
  expanded: boolean;
  narrow: boolean;
  onToggle: () => void;
}) {
  if (narrow) return top ? <div className='oxy-rail__top'>{top}</div> : null;
  return (
    <div className='oxy-rail__top'>
      {expanded && top}
      {expanded && (
        <span className='oxy-rail__title' data-testid='shell-rail-title'>
          {title}
        </span>
      )}
      <RailToggle expanded={expanded} onToggle={onToggle} mark={expanded ? undefined : top} />
    </div>
  );
}

function RailEntry({ item, expanded }: { item: RailItem; expanded: boolean }) {
  // The mark keeps its 32px cell in both states, so it does not move as the rail
  // opens; the name beside it shows only while the rail is open.
  const content = (
    <>
      <span className='oxy-rail__item-mark'>
        <RailItemMark item={item} />
      </span>
      <span className='oxy-rail__item-label'>{item.label}</span>
    </>
  );
  const className = cx("oxy-rail__item", item.active && "oxy-rail__item--active");
  const ariaCurrent = item.active ? ("page" as const) : undefined;
  const entry = item.href ? (
    <a
      href={item.href}
      data-testid={item.testId}
      aria-label={item.label}
      aria-current={ariaCurrent}
      className={className}
    >
      {content}
    </a>
  ) : (
    <button
      type='button'
      onClick={item.onSelect}
      data-testid={item.testId}
      aria-label={item.label}
      aria-current={ariaCurrent}
      className={className}
    >
      {content}
    </button>
  );
  // Closed, the name is a hint; open, it is on the row, so only a richer `tooltip`
  // still earns one.
  const hint = expanded ? item.tooltip : (item.tooltip ?? item.label);
  return hint ? <ShellTooltip content={hint}>{entry}</ShellTooltip> : entry;
}

/** A hairline divider between conceptual rail groups. */
function RailDivider() {
  return <div className='oxy-rail__divider' />;
}

/** The icon rail. It opens and closes like ChatGPT's and Gemini's sidebars, and
 *  only from its toggle: closed it is 48px of marks, each named in a hint; open
 *  it is 224px with each entry's name beside its mark. It takes its width in the
 *  frame's flow, so the content beside it narrows as it opens rather than being
 *  covered, and hovering it opens nothing. The state is remembered per browser
 *  (`RAIL_EXPANDED_KEY`).
 *
 *  Head: closed, one cell, the `top` mark, which is the toggle; open, the mark,
 *  `title` (the workspace's name) and the toggle at the far end. A rail with no
 *  `top` still has the head, holding the toggle alone.
 *
 *  Under the nav sheet's query (below 768px, a phone on its side) the shell draws
 *  the sheet instead; a rail still drawn there (`hideTopBar`, or an app's own
 *  `ShellRail`) is the 2.32 rail: closed, no toggle, the mark plain branding.
 *
 *  `groups` are the conceptual sections (HQ · Apps · Intelligence), rendered
 *  top-down with a hairline divider between them. `footerItems` pin to the
 *  bottom of the scroll area as a distinct "system" zone (Oxygen Factory),
 *  above the `bottom` account slot (workspace switch · user menu — injected by
 *  the host app). */
export function ShellRail({
  top,
  title,
  groups,
  groupTitles,
  footerItems,
  bottom,
  className
}: {
  top?: ReactNode;
  /** The name beside `top` in the head once the rail is open. */
  title?: string;
  groups: RailItem[][];
  /** A title above each group, by index, shown while the rail is open (Oxygen's
   *  rail titles its apps "Apps"); none for `undefined`. */
  groupTitles?: (string | undefined)[];
  footerItems?: RailItem[];
  /** The account slot at the foot. A function is told whether the rail is open, so
   *  the slot can draw its open form (a row with its name) beside the entries. */
  bottom?: ReactNode | ((expanded: boolean) => ReactNode);
  className?: string;
}) {
  const { container, setContainer } = useShellPortalContainer();
  const { expanded, narrow, toggle } = useRailExpanded();
  // No foot at all when the slot has nothing to draw, in either form.
  const foot = typeof bottom === "function" ? bottom(expanded) : bottom;
  return (
    <ShellPortalContext.Provider value={container}>
      {/* A div with the navigation role, not a <nav>: an app's own `nav { … }` rule
          must not restyle the shell. */}
      <div
        ref={setContainer}
        role='navigation'
        aria-label='Oxygen'
        data-testid='shell-rail'
        data-expanded={expanded || undefined}
        className={cx("oxy-shell-scope oxy-rail", expanded && "oxy-rail--expanded", className)}
      >
        <RailHead top={top} title={title} expanded={expanded} narrow={narrow} onToggle={toggle} />
        <div className='oxy-rail__nav'>
          {groups.map((group, i) => (
            <Fragment key={group[0]?.key ?? i}>
              {i > 0 && <RailDivider />}
              {expanded && groupTitles?.[i] && (
                <div className='oxy-rail__group-title'>{groupTitles[i]}</div>
              )}
              {group.map((item) => (
                <RailEntry key={item.key} item={item} expanded={expanded} />
              ))}
            </Fragment>
          ))}
          {footerItems && footerItems.length > 0 && (
            <div className='oxy-rail__footer'>
              <RailDivider />
              {footerItems.map((item) => (
                <RailEntry key={item.key} item={item} expanded={expanded} />
              ))}
            </div>
          )}
        </div>
        {foot && <div className='oxy-rail__bottom'>{foot}</div>}
      </div>
    </ShellPortalContext.Provider>
  );
}
