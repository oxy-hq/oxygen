import { ActiveSection } from "@/components/settings/SettingsDialog/components/ActiveSection";
import { SectionChrome } from "@/components/settings/SettingsDialog/components/SectionChrome";
import type { SettingsSection } from "@/components/settings/settingsRoute";
import type { Organization, OrgRole } from "@/types/organization";
import type { Workspace } from "@/types/workspace";

export interface SectionPaneProps {
  section: SettingsSection;
  title: string;
  description?: string;
  /** The bar already names the section on a phone, so the heading is not drawn twice. */
  titleVisuallyHidden: boolean;
  scrollRef: React.RefObject<HTMLDivElement | null>;
  headingRef: React.RefObject<HTMLHeadingElement | null>;
  org: Organization | null;
  role: OrgRole | null;
  workspace: Workspace | null;
  close: () => void;
}

/** The scrolling half of the page: the section's header, then the section. */
export function SectionPane({
  section,
  title,
  description,
  titleVisuallyHidden,
  scrollRef,
  headingRef,
  org,
  role,
  workspace,
  close
}: SectionPaneProps) {
  return (
    <main
      ref={scrollRef}
      aria-label={title}
      className='min-h-0 min-w-0 flex-1 overflow-y-auto px-4 py-5 sm:px-8 sm:py-6'
    >
      <SectionChrome
        title={title}
        description={description}
        headingRef={headingRef}
        titleVisuallyHidden={titleVisuallyHidden}
      >
        {/* Keyed so a section change remounts rather than reconciling one section's form
            state onto the next. */}
        <div
          key={section}
          className='transition-[opacity,translate] duration-(--duration-slow) starting:translate-y-1 starting:opacity-0 motion-reduce:starting:translate-y-0'
        >
          <ActiveSection
            activeSection={section}
            org={org}
            role={role}
            workspace={workspace}
            close={close}
          />
        </div>
      </SectionChrome>
    </main>
  );
}
