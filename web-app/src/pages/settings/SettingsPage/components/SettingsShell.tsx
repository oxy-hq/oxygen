import { SettingsNav } from "@/components/settings/SettingsDialog/components/SettingsNav";
import { VersionBadge } from "@/components/settings/SettingsDialog/components/VersionBadge";
import type { VisibleNavGroup } from "@/components/settings/SettingsDialog/nav";
import type { SettingsSection } from "@/components/settings/settingsRoute";
import { cn } from "@/libs/shadcn/utils";
import type { Organization, OrgRole } from "@/types/organization";
import type { Workspace } from "@/types/workspace";
import { SectionPane } from "./SectionPane";
import { SettingsPageHeader } from "./SettingsPageHeader";

export interface SettingsShellProps {
  groups: VisibleNavGroup[];
  /** The section on screen, or `null` on the index — which is the list, on a phone. */
  shownSection: SettingsSection | null;
  sectionTitle: string;
  sectionDescription?: string;
  isDesktop: boolean;
  hrefFor: (section: SettingsSection) => string;
  /** The settings index. Absolute, for the same reason `hrefFor` is — see `backHref`. */
  listHref: string;
  query: string;
  onQueryChange: (query: string) => void;
  scrollRef: React.RefObject<HTMLDivElement | null>;
  headingRef: React.RefObject<HTMLHeadingElement | null>;
  org: Organization | null;
  role: OrgRole | null;
  workspace: Workspace | null;
  close: () => void;
}

/**
 * The frame: a bar, the rail, and the section.
 *
 * On a wide screen both halves are on screen at once. On a phone they are separate pages —
 * the rail fills it until a section is chosen, and the bar grows a way back. That split is
 * the routing's, not this component's: it just renders whichever it was handed.
 */
export function SettingsShell({
  groups,
  shownSection,
  sectionTitle,
  sectionDescription,
  isDesktop,
  hrefFor,
  listHref,
  query,
  onQueryChange,
  scrollRef,
  headingRef,
  org,
  role,
  workspace,
  close
}: SettingsShellProps) {
  const inSectionOnPhone = !isDesktop && shownSection !== null;
  const showNav = isDesktop || shownSection === null;

  return (
    // `w-full flex-1 min-w-0`: the app root is a flex row, so without these the page sizes
    // to its content and leaves a band of empty background down the right — which on a phone
    // is most of the screen.
    <div className='flex h-full min-h-0 w-full min-w-0 flex-1 flex-col bg-background'>
      <SettingsPageHeader
        title={inSectionOnPhone ? sectionTitle : "Settings"}
        // Absolute, never `"."`: the leaf route is the splat `settings/*`, and React Router
        // resolves a splat route against the *full* pathname — so `"."` on
        // `/acme/settings/organization/members` was that same page, and Back re-opened the
        // section instead of showing the list. Same trap as the `../` that built
        // `/settings/organization/settings/…`.
        backHref={inSectionOnPhone ? listHref : undefined}
        onClose={close}
      />

      <div className='flex min-h-0 min-w-0 flex-1'>
        {showNav && (
          <SettingsNav
            groups={groups}
            activeSection={shownSection}
            hrefFor={hrefFor}
            query={query}
            onQueryChange={onQueryChange}
            footer={<VersionBadge />}
            className={cn(
              "min-w-0",
              isDesktop ? "w-64 shrink-0 border-border border-r bg-sidebar" : "flex-1"
            )}
          />
        )}

        {shownSection && (
          <SectionPane
            section={shownSection}
            title={sectionTitle}
            description={sectionDescription}
            titleVisuallyHidden={!isDesktop}
            scrollRef={scrollRef}
            headingRef={headingRef}
            org={org}
            role={role}
            workspace={workspace}
            close={close}
          />
        )}
      </div>
    </div>
  );
}
