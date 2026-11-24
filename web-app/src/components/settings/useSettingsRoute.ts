import { useCallback, useMemo } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import useCurrentOrg from "@/stores/useCurrentOrg";
import {
  type SettingsSection,
  settingsHref,
  settingsRootHref,
  targetFromPath
} from "./settingsRoute";

export interface SettingsRoute {
  /** The section the address names, or `null` on the settings index. */
  section: SettingsSection | null;
  /** The tab the address names, or `null` for the section's first visible one. */
  tab: string | null;
  /** Go to a section (and optionally one of its tabs). */
  open: (section: SettingsSection, tab?: string | null) => void;
  /** Switch tabs inside the section that is already open. */
  setTab: (tab: string | null) => void;
  /** Leave settings. */
  close: () => void;
  /** Whether settings is the page currently on screen. */
  isOpen: boolean;
  /** The org the address belongs to; undefined in local mode, which has no org segment. */
  orgSlug: string | undefined;
}

/**
 * Settings as an address.
 *
 * **This replaced a Zustand store**, and the reason is worth keeping: the store and the URL
 * both owned "which section is open", and every bug this surface had came from the two of
 * them disagreeing on a single commit — Back became a no-op, a close turned into a reopen, a
 * `?settings=` link was wiped before it opened, and the address quietly stopped tracking the
 * dialog for the rest of the session. One owner, and none of them can happen: the URL is
 * read, and `open()` is a navigation.
 */
export function useSettingsRoute(): SettingsRoute {
  const navigate = useNavigate();
  const location = useLocation();
  const params = useParams();
  const storeSlug = useCurrentOrg((s) => s.org?.slug);
  // `useParams` only has the slug on a route that declares it; the store covers the rest.
  const orgSlug = params.orgSlug ?? storeSlug;

  // Anchored to this org's own `/settings`, not the first "/settings" anywhere in the path.
  // The loose search matched the *org segment* for a slug that starts with one —
  // `/settings-co/settings/organization/members` parsed as `-co.settings.…`, resolved to
  // nothing, and left every settings page in that org blank on desktop and sectionless on a
  // phone. Nothing reserves such a slug.
  //
  // Anchoring rather than reading the router's splat, because `params["*"]` is populated by
  // *any* matched splat — the org catch-all included — so it cannot answer "am I in
  // settings".
  const target = useMemo(() => {
    const root = `${orgSlug ? `/${orgSlug}` : ""}/settings`;
    const { pathname } = location;
    if (pathname !== root && !pathname.startsWith(`${root}/`)) {
      return { inSettings: false, found: null };
    }
    return { inSettings: true, found: targetFromPath(pathname.slice(root.length)) };
  }, [location, orgSlug]);

  const open = useCallback(
    (section: SettingsSection, tab?: string | null) => {
      navigate(settingsHref(orgSlug, section, tab));
    },
    [navigate, orgSlug]
  );

  const setTab = useCallback(
    (tab: string | null) => {
      if (!target.found) return;
      // Replace: moving between a section's tabs is not a separate place to come back to.
      navigate(settingsHref(orgSlug, target.found.section, tab), { replace: true });
    },
    [navigate, orgSlug, target.found]
  );

  const close = useCallback(() => {
    navigate(orgSlug ? `/${orgSlug}/home` : "/home");
  }, [navigate, orgSlug]);

  return {
    section: target.found?.section ?? null,
    tab: target.found?.tab ?? null,
    open,
    setTab,
    close,
    isOpen: target.inSettings,
    orgSlug
  };
}

/**
 * Just the "take me to settings" half, for the rail menu, the IDE and anywhere else outside
 * the surface. With no section it goes to the index, which lands on the first page the viewer
 * can actually open.
 */
export function useOpenSettings(): (
  section?: SettingsSection,
  tab?: string | null,
  options?: { replace?: boolean }
) => void {
  const navigate = useNavigate();
  const params = useParams();
  const storeSlug = useCurrentOrg((s) => s.org?.slug);
  const orgSlug = params.orgSlug ?? storeSlug;

  return useCallback(
    (section?: SettingsSection, tab?: string | null, options?: { replace?: boolean }) => {
      navigate(section ? settingsHref(orgSlug, section, tab) : settingsRootHref(orgSlug), {
        // `replace` is for arriving somewhere the person did not choose to go — a redirect
        // back from an OAuth provider, say, where the address they came in on should not
        // survive in the history behind them.
        replace: options?.replace ?? false
      });
    },
    [navigate, orgSlug]
  );
}
