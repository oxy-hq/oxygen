// Started from oxy-hq/reference-app@3caad1f `src/components/ParentTabs.tsx` for the semantics
// (INVENTORY: the reference's 315 lines carry the rule; its six app copies are 77-210), made
// router-free as pokehouse-oxy:apps/bookkeeping/src/components/ui/ParentTabs.tsx is (`href` +
// `onNavigate`); the top-bar placement is Bookkeeping's tested `TopBarTabs` (./TopBarTabs.tsx).
// Kept: `also`, the three placements, `topBarFrom`, `aria-current="page"`, real links. Dropped:
// the reference's own tab tables (`RECORDS_TABS`, `sectionTabsFor`), which are the app's.
//
// The app's PARENT tabs: which SCREEN of a section am I on. The pair to `DefaultTabs`. **Rule: if
// selecting it goes to another destination, it is this; if it switches content within the
// current screen, it is `DefaultTabs`; a query string alone does not make a choice navigation.**
//
// Real links, not `role="tab"` buttons: `role="tab"` announces "this control reveals a panel
// already in this document", and these load another screen. A `<nav>` of links marked
// `aria-current="page"` is the honest markup, and it keeps ⌘-click, middle-click, a copied URL
// and Back working. Each link is its own Tab stop, as in any navigation; arrow keys are not used.
// `onNavigate` takes a plain left click for the app's own router; anything else (⌘, Ctrl, Shift,
// Alt, a middle click) is left to the browser. Without `onNavigate` the links navigate natively.
//
// The underline shape, told apart from DefaultTabs' segments on purpose: two identical boxes
// give a reader no cue which one contains the other.
//
// Where it sits: the shell's top bar on the desk (`placement="topbar"`, after the breadcrumb:
// `TopBarLeft`), from `topBarFrom` up; below it the page's first row (`placement="page"`). Render
// both from one table and CSS shows exactly one at any width; the hidden copy is
// `display: none`, so it is out of the Tab order and the accessibility tree too. In the top bar,
// the tabs that do not fit go behind a More menu (decision 29). On a phone the app's phone tree
// draws the kit's `PhoneTabBar` instead (decision 15): this piece does not switch itself.

import { cx } from "../cx";
import { ui } from "../recipes";
import { isOpen, linkClick, type ParentTabsProps } from "./parentTab";
import { TopBarTabs } from "./TopBarTabs";

export type {
  ParentTab,
  ParentTabsPlacement,
  ParentTabsProps,
  TopBarFrom
} from "./parentTab";
export { isPlainClick } from "./parentTab";

export function ParentTabs<K extends string>({
  label,
  tabs,
  current,
  onNavigate,
  placement = "inline",
  topBarFrom = "md",
  moreLabel,
  className
}: ParentTabsProps<K>) {
  if (placement === "topbar")
    return (
      <TopBarTabs
        label={label}
        tabs={tabs}
        current={current}
        onNavigate={onNavigate}
        topBarFrom={topBarFrom}
        moreLabel={moreLabel}
        className={className}
      />
    );
  return (
    <nav
      aria-label={label}
      data-placement={placement}
      className={cx(
        ui.pageTabs,
        "oxy-ui-parent-tabs",
        placement === "page" && `oxy-ui-parent-tabs--page-${topBarFrom}`,
        className
      )}
    >
      {tabs.map((tab) => {
        const on = isOpen(tab, current);
        return (
          <a
            key={tab.id}
            href={tab.href}
            // `page`, not `true`: which of several PAGES is open, read as "current page".
            aria-current={on ? "page" : undefined}
            className={cx(ui.tab, "oxy-ui-parent-tabs__tab", on ? ui.tabOn : ui.tabOff)}
            onClick={linkClick(tab, onNavigate)}
          >
            {tab.icon}
            {tab.label}
            {tab.trailing}
          </a>
        );
      })}
    </nav>
  );
}
