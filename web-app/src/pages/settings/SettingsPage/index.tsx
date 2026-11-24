import { useRef, useState } from "react";
import { Navigate } from "react-router-dom";
import { flattenNavGroups, navItemFor } from "@/components/settings/SettingsDialog/nav";
import { useVisibleNavGroups } from "@/components/settings/SettingsDialog/useVisibleNavGroups";
import { settingsHref, settingsRootHref } from "@/components/settings/settingsRoute";
import { useSettingsRoute } from "@/components/settings/useSettingsRoute";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { SettingsShell } from "./components/SettingsShell";
import { settingsDestination } from "./destination";
import { useArriveAtSection } from "./useArriveAtSection";

/** Below this the rail and the section are separate pages, as on a phone. */
const DESKTOP = "(min-width: 768px)";

/**
 * Settings, at `/<org>/settings/<area>/<page>[/<tab>]`.
 *
 * **It was a dialog.** The move to a route is not cosmetic: the dialog kept "which section is
 * open" in a store *and* mirrored it into `?settings=`, and every defect this surface had came
 * from those two owners disagreeing on one commit. As a route the address is the only owner,
 * so Back, Forward, refresh, bookmarking and sharing are the router's job and not this
 * component's. `?settings=` links keep working through `LegacySettingsParamRedirect`.
 *
 * On a phone the two halves are two pages rather than one pane with a flag: `/settings` is the
 * list, `/settings/<area>/<page>` is the section, and Back between them is the browser's. The
 * dialog faked this with internal state, which is why its deep links landed phone users on the
 * list instead of the page they asked for.
 */
export interface SettingsPageProps {
  /**
   * Whether the org, role and workspace the nav gates on have all answered. Until they have,
   * a section missing from the nav is missing because nothing has loaded, not because the
   * viewer may not open it — and redirecting then threw away the address they just followed.
   */
  contextReady?: boolean;
}

export default function SettingsPage({ contextReady = true }: SettingsPageProps) {
  const { section, close, orgSlug } = useSettingsRoute();
  const { org } = useCurrentOrg();
  const role = useCurrentOrg((s) => s.role);
  const { workspace } = useCurrentWorkspace();

  const isDesktop = useMediaQuery(DESKTOP);
  const [query, setQuery] = useState("");

  const scrollRef = useRef<HTMLDivElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);

  const visibleGroups = useVisibleNavGroups();
  const allItems = flattenNavGroups(visibleGroups);
  // From the definitions, not the visible nav: see `navItemFor`.
  const activeItem = section ? navItemFor(section) : undefined;

  useArriveAtSection(section, { scrollRef, headingRef });

  if (visibleGroups.length === 0) return null;

  const where = settingsDestination({
    section,
    visible: allItems.map((i) => i.value),
    isDesktop,
    contextReady
  });

  if (where.kind === "nothing") return null;
  // Absolute, never relative: `../` resolved against the current path, so arriving deeper
  // (`/settings/organization/members`) built `/settings/organization/settings/...`.
  if (where.kind === "redirect") {
    return <Navigate to={settingsHref(orgSlug, where.section)} replace />;
  }

  const shownSection = where.kind === "section" ? where.section : null;

  return (
    <SettingsShell
      groups={visibleGroups}
      shownSection={shownSection}
      sectionTitle={activeItem?.label ?? "Settings"}
      sectionDescription={activeItem?.description}
      isDesktop={isDesktop}
      hrefFor={(target) => settingsHref(orgSlug, target)}
      listHref={settingsRootHref(orgSlug)}
      query={query}
      onQueryChange={setQuery}
      scrollRef={scrollRef}
      headingRef={headingRef}
      org={org}
      role={role}
      workspace={workspace}
      close={close}
    />
  );
}
