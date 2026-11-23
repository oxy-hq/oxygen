import type { ReactNode } from "react";
import { AskDock } from "@/components/Ask/AskDock";
import { OpenFactoryDialog } from "@/components/admin/OpenFactoryDialog";
import { PreviewBar } from "@/components/PreviewBar";
import useOrgCustomApps from "@/hooks/api/customApps/useOrgCustomApps";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { NotificationsDock } from "./NotificationsDock";
import { RailUserMenu } from "./RailUserMenu";
import { RailWorkspaceTile } from "./RailWorkspaceTile";
import { ShellNavSheet } from "./ShellNavSheet";
import { ToggleRail, useRailOpen } from "./ToggleRail";
import { TopBar } from "./TopBar";
import { useFooterItems, useOpenFactoryEntry, useRailGroups, useShellPlace } from "./useShellNav";

/** The workspace chrome: icon rail + universal top bar + content column + Ask
 *  dock. Wraps every workspace route. The rail, top bar, and dock hide inside
 *  Oxygen Factory / the IDE (it has its own chrome) and on the setup wizard
 *  (`/<org>/setup`; local mode's old `/onboarding` too); the content column is identical everywhere so pages never re-layout
 *  between routes. The entries themselves come from `useShellNav`. */
export function WorkspaceShell({ children }: { children: ReactNode }) {
  const { workspace } = useCurrentWorkspace();
  const currentOrg = useCurrentOrg((s) => s.org);
  const { hideRail, isHome } = useShellPlace();
  const [railOpen, toggleRail] = useRailOpen();
  // The rail's app marks are the org's: every app the person can open across its
  // workspaces, the same list the org home's grid shows.
  const { apps } = useOrgCustomApps(workspace?.id ?? "");
  const groups = useRailGroups(apps);
  const factory = useOpenFactoryEntry();
  const footerItems = useFooterItems(factory);

  // The account menu: a row with the person's name in the open rail and the sheet (which
  // always shows names), the avatar alone in the closed rail.
  const bottom = (open: boolean) => (
    <>
      {/* No workspace switcher: the home lists every app across the org, and
          Oxy engineers move between workspaces and organizations in the
          Factory's top bar. */}
      <RailUserMenu open={open} />
    </>
  );

  const layout = (
    <div className='flex min-h-0 w-full flex-1'>
      {/* Below `md` the rail gives way to ShellNavSheet: opened, the rail's
          224px is most of a phone's width, and closed its 48px is an eighth of
          it. The rail sits in this row's flow, so opening it narrows the
          content column beside it rather than covering it. */}
      {!hideRail && (
        <div className='hidden h-full md:flex'>
          <ToggleRail
            open={railOpen}
            onToggle={toggleRail}
            logo={<RailWorkspaceTile className='size-7' />}
            name={currentOrg?.name ?? workspace?.name ?? ""}
            groups={groups}
            groupTitles={[undefined, "Apps"]}
            footerItems={footerItems}
            bottom={bottom(railOpen)}
          />
        </div>
      )}
      {/* Content column. The top bar sits to the RIGHT of the rail and is the
          same height as the rail's logo cell (h-12), so the logo anchors the
          top-left corner and the two bottom borders form one continuous line. */}
      <div className='flex h-full min-w-0 flex-1 flex-col'>
        {!hideRail && (
          <TopBar
            bellOnPage={isHome}
            leading={
              <div className='flex md:hidden'>
                <ShellNavSheet
                  title={currentOrg?.name ?? workspace?.name ?? ""}
                  top={<RailWorkspaceTile className='size-7' />}
                  groups={groups}
                  groupTitles={[undefined, "Apps"]}
                  footerItems={footerItems}
                  bottom={bottom(true)}
                />
              </div>
            }
          />
        )}
        {/* Follows the operator INTO the tenant — this is where an unnoticed
            impersonation would actually do damage. */}
        <div className='flex min-h-0 w-full flex-1'>
          <main className='relative flex h-full min-w-0 flex-1 flex-col bg-background'>
            <div className='w-full min-w-0 flex-1 overflow-hidden'>{children}</div>
          </main>
          {/* The Ask dock is a flex sibling — opening it compacts <main>
              (Cursor-style) rather than floating over it. A custom app is not
              hosted here: it opens in this tab and draws its own dock and Ask
              (`OxyShell`). */}
          {!hideRail && <AskDock />}
          {/* The notifications panel, on the same edge; one of the two at a time. */}
          {!hideRail && <NotificationsDock />}
        </div>
      </div>
    </div>
  );

  // The preview bar sits above everything — rail, top bar and the IDE's own
  // chrome alike: while a page is pinned to a preview it is the only thing
  // saying so. It renders nothing on a live page.
  return (
    <div className='flex h-full w-full flex-col'>
      <PreviewBar />
      {layout}
      {factory.staff && (
        <OpenFactoryDialog
          open={factory.open}
          onOpenChange={factory.setOpen}
          initialTarget={factory.target}
          initialScope={factory.scope}
        />
      )}
    </div>
  );
}
