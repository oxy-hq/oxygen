import type { ReactNode } from "react";
import { OxyMark } from "@/components/OxyMark";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { cn } from "@/libs/shadcn/utils";
import useAskDock from "@/stores/useAskDock";
import { Breadcrumb } from "./Breadcrumb";
import { AppHealthItem } from "./MenuBar/AppHealthItem";
import { ConnectionItem } from "./MenuBar/ConnectionItem";
import { NotificationsBell } from "./NotificationsBell";
import { SystemIndicator } from "./SystemIndicator";
import { WorkspaceClock } from "./WorkspaceClock";

/** Toggles the Ask dock. Renders the same OxyMark the removed floating pill
 *  used; ⌘K still toggles the dock too (App.tsx window listener). */
function AskOxygenButton() {
  const isOpen = useAskDock((s) => s.isOpen);
  const toggle = useAskDock((s) => s.toggle);
  return (
    <button
      type='button'
      data-testid='ask-oxygen-button'
      onClick={toggle}
      className={cn(
        "flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs transition-colors",
        isOpen
          ? "border-primary/40 bg-primary/10 text-foreground"
          : "text-muted-foreground hover:border-primary/40 hover:text-foreground"
      )}
    >
      <OxyMark className='size-3.5 text-primary' />
      <span className='hidden sm:inline'>Ask Oxygen</span>
      <kbd className='hidden rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] sm:inline'>
        ⌘K
      </kbd>
    </button>
  );
}

/**
 * The universal top bar: breadcrumb (left) + system status, clock, the inbox
 * bell, and the Ask Oxygen toggle (right). Spans the content column above `<main>` and the
 * Ask dock. Hidden inside the IDE and onboarding (gated with the rail in
 * WorkspaceShell). Mirrors the rail's `bg-sidebar-background` so the two read
 * as one frame. `leading` sits before the breadcrumb — the shell puts the
 * mobile nav drawer's menu button there. Follow-up: lift into `@oxy-hq/sdk`
 * for custom apps.
 *
 * With the apps-first experience on it is the home's menu bar, macOS-style: the
 * rail is the dock, and the right side carries status items — apps that need
 * attention, connection, Ask (the Spotlight), the bell (Notification Center) and
 * the day and time last.
 */
export function TopBar({ leading }: { leading?: ReactNode }) {
  const appsFirst = useAppsFirstExperience();
  return (
    <header
      data-testid='workspace-topbar'
      data-menubar={appsFirst || undefined}
      className='flex h-12 shrink-0 items-center gap-3 border-b bg-sidebar-background px-3'
    >
      {leading}
      <Breadcrumb />
      {appsFirst ? (
        <div className='ml-auto flex shrink-0 items-center gap-1.5' data-testid='menubar-status'>
          <AppHealthItem />
          <ConnectionItem />
          <AskOxygenButton />
          <NotificationsBell />
          <WorkspaceClock withDate />
        </div>
      ) : (
        <div className='ml-auto flex shrink-0 items-center gap-3'>
          <SystemIndicator />
          <WorkspaceClock />
          <NotificationsBell />
          <AskOxygenButton />
        </div>
      )}
    </header>
  );
}
