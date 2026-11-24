import type { ReactNode } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { cn } from "@/libs/shadcn/utils";
import { Breadcrumb } from "./Breadcrumb";
import { AppHealthItem } from "./MenuBar/AppHealthItem";
import { ConnectionItem } from "./MenuBar/ConnectionItem";
import { DockBell } from "./NotificationsBell/DockBell";

/**
 * The universal top bar, the reference design's: the bar on the rail's white ground
 * with a hairline under it, the breadcrumb, then the status items (only while
 * something is wrong: an app down, the browser offline) and the bell that docks the
 * notifications panel. No Ask button (and no ⌘K: nothing in the chrome opens Ask), no
 * clock and no Today.
 * Spans the content column above `<main>` and the docks. Hidden inside the IDE and
 * the setup wizard (gated with the rail in WorkspaceShell). `leading` sits before the
 * breadcrumb: the shell puts the phone's nav sheet button there. `bellOnPage`: the
 * page draws its own bell on a phone (the home's, beside its greeting), so the
 * bar's hides there.
 */
export function TopBar({ leading, bellOnPage }: { leading?: ReactNode; bellOnPage?: boolean }) {
  return (
    <header
      data-testid='workspace-topbar'
      data-menubar
      className={cn(
        DESIGN_REFERENCE,
        "flex h-12 shrink-0 items-center gap-3 border-border-subtle border-b bg-sidebar px-3"
      )}
    >
      {leading}
      <Breadcrumb />
      <div className='ml-auto flex shrink-0 items-center gap-1.5' data-testid='menubar-status'>
        <AppHealthItem />
        <ConnectionItem />
        <span className={cn("flex", bellOnPage && "max-md:hidden")}>
          <DockBell />
        </span>
      </div>
    </header>
  );
}
