import { useEffect } from "react";
import { useSearchParams } from "react-router-dom";
import useSettingsDialog, { type SettingsSection } from "@/stores/useSettingsDialog";
import { CLOUD_NAV, LOCAL_NAV, RENAMED_SECTIONS, type SettingsTarget } from "./nav";

/** The query parameter a link uses to open the dialog at a section. */
export const SETTINGS_PARAM = "settings";

/** Every section id in either nav, with the tabs a link may name for it. */
const KNOWN: ReadonlyMap<string, readonly string[]> = new Map(
  [...CLOUD_NAV, ...LOCAL_NAV].flatMap((group) =>
    group.items.map((item) => [item.value, item.tabs ?? []] as const)
  )
);

/**
 * Where a `?settings=` value opens, or `null` for anything the dialog does not have. Three
 * shapes are accepted:
 *
 * - a section id from the nav (`organization.members`);
 * - a section id and one of its tabs (`organization.api_tokens.personal`);
 * - a section id that was renamed or merged (`account.tokens`), which lands where that page
 *   now lives (`RENAMED_SECTIONS`).
 *
 * Derived from the nav itself rather than a second list, so a section or tab added to the nav
 * is linkable the same day.
 */
export function targetFromParam(raw: string | null | undefined): SettingsTarget | null {
  if (!raw) return null;
  if (KNOWN.has(raw)) return { section: raw as SettingsSection };
  const renamed = RENAMED_SECTIONS[raw];
  if (renamed) return renamed;
  const dot = raw.lastIndexOf(".");
  if (dot <= 0) return null;
  const section = raw.slice(0, dot);
  const tab = raw.slice(dot + 1);
  return KNOWN.get(section)?.includes(tab) ? { section: section as SettingsSection, tab } : null;
}

/**
 * `?settings=<section>` opens the dialog at that section once and strips the
 * param, so a refresh does not reopen it.
 *
 * Mounted wherever the dialog is — the workspace layout and the org's
 * "being set up" screen — so the link the custom-app shell already emits
 * (`…/home?settings=organization.general`) lands, and so does a link into an
 * org that has no workspace yet (`/<org>?settings=organization.crew`).
 * An unknown value is dropped rather than opening the wrong section; the
 * dialog's own gates decide what the viewer may see once it is open — a
 * valid section the viewer cannot see (a workspace section on an org with
 * none, a cloud section in local mode) falls to the first visible one, which
 * is why this hook needs no mode guard of its own. A tab the viewer cannot see
 * falls to the section's first visible tab the same way.
 */
export function useSettingsDeepLink() {
  const [searchParams, setSearchParams] = useSearchParams();
  const open = useSettingsDialog((s) => s.open);
  useEffect(() => {
    const raw = searchParams.get(SETTINGS_PARAM);
    if (raw === null) return;
    const target = targetFromParam(raw);
    if (target) open(target.section, target.tab ?? null);
    // The functional form: another effect in the same commit (the Slack
    // return in the workspace layout) also strips a param, and two writes
    // from one snapshot would resurrect each other's deletion.
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete(SETTINGS_PARAM);
        return next;
      },
      { replace: true }
    );
  }, [searchParams, setSearchParams, open]);
}
