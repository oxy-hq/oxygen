import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import OxyLogo from "@/components/OxyLogo";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Sidebar as ShadcnSidebar } from "@/components/ui/shadcn/sidebar";

/**
 * The rail both operations consoles are navigated by — the staff console's and the
 * partner console's.
 *
 * They are the same *kind* of surface (`PartnerLayout`: "what differs is reach, not
 * visual language"), so they are one rail: the same frame, the same 52px head with the
 * logo and a scope chip, the same scroller, the same collapse. What differs is the chip's
 * word, where the logo points, and what is in it. Before this they had drifted — two
 * border tokens, two chip inks, two letter-spacings, one with a footer and one without,
 * and one collapsing to a strip while the other vanished.
 *
 * **The scope goes inside `<Sidebar>`, not on it.** Below 768px shadcn's `Sidebar`
 * renders its children into a portalled `<SheetContent>` whose `className` it fixes and
 * whose portal leaves the console's `design-reference` subtree. A scope on the rail —
 * or on the layout above it — therefore does not reach a phone, and the reference-only
 * roles the rows are written in (`row-selected`, `row-hover`, `subtle-foreground`,
 * `border-subtle`) would have no value there: the active row would lose its highlight
 * entirely and the hairlines would fall back to `currentColor`. Custom properties
 * inherit through the DOM, so scoping the children covers the desktop rail and the
 * phone sheet alike.
 */
export function ConsoleRail({
  badge,
  logoTo,
  children,
  footer
}: {
  /** The one word under the logo saying which console this is: `Admin`, `Partner`. */
  badge: string;
  /** Where the logo goes: each console's own home. */
  logoTo: string;
  /** The rail's groups. Scrolls. */
  children: ReactNode;
  /** Pinned to the foot, below the scroller. */
  footer?: ReactNode;
}) {
  return (
    <ShadcnSidebar
      // **`icon`, not the default `offcanvas`** — the same bargain the product's rail
      // strikes (`components/Shell/ToggleRail`): closed it is a 3rem strip of marks, not
      // nothing. Off-canvas bought 256px by taking the navigation away with it, so the
      // toggle was a choice between a wide rail and no rail; now it is a choice between
      // names and no names, and ⌘B is worth pressing on a wide screen.
      collapsible='icon'
      className='border-border-subtle border-r bg-sidebar-background'
    >
      <div className={`${DESIGN_REFERENCE} flex h-full min-h-0 flex-col`}>
        <div className='flex h-[52px] shrink-0 items-center gap-2 border-border-subtle border-b px-3 group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0'>
          <Link to={logoTo} className='flex shrink-0 items-center'>
            <OxyLogo />
          </Link>
          {/* The chip is the one thing with no room at 3rem, and the logo already says
              where you are. 10px to stay on the console's type scale, not the
              reference's 12px eyebrow (`pages/admin/CLAUDE.md`). */}
          <span className='border border-border-subtle px-1.5 py-0.5 font-mono text-[10px] text-subtle-foreground uppercase tracking-eyebrow group-data-[collapsible=icon]:hidden'>
            {badge}
          </span>
        </div>

        <div className='min-h-0 flex-1 overflow-auto'>{children}</div>

        {footer}
      </div>
    </ShadcnSidebar>
  );
}

/**
 * The one row treatment both consoles' rails use, in the reference's roles: dim ink
 * going to full, a neutral fill when selected rather than a tint, no border.
 *
 * Spelled as `data-[active=true]` variants so they outrank `SidebarMenuButton`'s own
 * `data-[active=true]:bg-sidebar-accent` through tailwind-merge.
 */
export const consoleNavRow =
  "gap-2.5 text-[13px] text-subtle-foreground hover:bg-row-hover hover:text-foreground data-[active=true]:bg-row-selected data-[active=true]:font-medium data-[active=true]:text-foreground [&>svg]:size-3.5";
