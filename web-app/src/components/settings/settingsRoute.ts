import { CLOUD_NAV, LOCAL_NAV, RENAMED_SECTIONS, type SettingsTarget } from "./SettingsDialog/nav";

/**
 * A settings page's id. These are a contract, not a label: they appear in `?settings=` links
 * the backend puts in notification emails, in URLs published `oxyc` versions print, and in
 * people's bookmarks. Add one, never rename one — `RENAMED_SECTIONS` is where the four that
 * ever moved are redirected from.
 */
export type SettingsSection =
  | "organization.general"
  | "organization.members"
  | "organization.teams"
  | "organization.app_access"
  | "organization.crew"
  | "organization.locations"
  | "organization.positions"
  | "organization.api_tokens"
  | "organization.secrets"
  | "organization.billing"
  | "organization.integration"
  | "workspace.members"
  | "workspace.databases"
  | "workspace.previews"
  | "workspace.airhouse"
  | "workspace.oltp"
  | "workspace.connections"
  | "workspace.apps"
  | "workspace.activity_logs"
  | "workspace.oxy_access"
  | "account.notifications"
  | "preferences.appearance";

/** The query parameter the old dialog used, and that links already out there still carry. */
export const SETTINGS_PARAM = "settings";

/** Every section id in either nav, with the tabs a link may name for it. */
const KNOWN: ReadonlyMap<string, readonly string[]> = new Map(
  [...CLOUD_NAV, ...LOCAL_NAV].flatMap((group) =>
    group.items.map((item) => [item.value, item.tabs ?? []] as const)
  )
);

/**
 * Where a dotted settings id opens, or `null` for anything the surface does not have. Three
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
export function targetFromId(raw: string | null | undefined): SettingsTarget | null {
  if (!raw) return null;
  if (KNOWN.has(raw)) return { section: raw as SettingsSection };
  // `Object.hasOwn`, not a bare index: `RENAMED_SECTIONS` is an object literal, so
  // `?settings=constructor` (or `toString`, `__proto__`…) would otherwise return a truthy
  // member of `Object.prototype` and be handed on as if it were a target. The result is then
  // checked against the nav as well, so a redirect can only ever land on a real section.
  const renamed = Object.hasOwn(RENAMED_SECTIONS, raw) ? RENAMED_SECTIONS[raw] : undefined;
  if (renamed && KNOWN.has(renamed.section)) return renamed;
  const dot = raw.lastIndexOf(".");
  if (dot <= 0) return null;
  const section = raw.slice(0, dot);
  const tab = raw.slice(dot + 1);
  return KNOWN.get(section)?.includes(tab) ? { section: section as SettingsSection, tab } : null;
}

/**
 * Where a URL path under `/settings` opens.
 *
 * The path is the dotted id with its dots as slashes — `organization.api_tokens.personal`
 * becomes `organization/api_tokens/personal` — so the address and the contract are the same
 * string in two spellings, and one parser serves both. An empty path is the index, which is
 * the nav with nothing chosen.
 */
export function targetFromPath(splat: string | undefined): SettingsTarget | null {
  const trimmed = (splat ?? "").replace(/^\/+|\/+$/g, "");
  return trimmed ? targetFromId(trimmed.split("/").join(".")) : null;
}

/** The path under the org for a section, without a leading slash: `settings/organization/members`. */
export function settingsPathFor(section: SettingsSection, tab?: string | null): string {
  const id = tab ? `${section}.${tab}` : section;
  return `settings/${id.split(".").join("/")}`;
}

/** The whole address for a section, ready for `navigate()`. In local mode the org part is empty. */
export function settingsHref(
  orgSlug: string | undefined,
  section: SettingsSection,
  tab?: string | null
): string {
  const base = orgSlug ? `/${orgSlug}` : "";
  return `${base}/${settingsPathFor(section, tab)}`;
}

/** The settings index: the nav, with no section chosen. */
export function settingsRootHref(orgSlug: string | undefined): string {
  return orgSlug ? `/${orgSlug}/settings` : "/settings";
}
