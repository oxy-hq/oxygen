import ROUTES from "@/libs/utils/routes";
import type { BoundKioskDevice } from "@/types/frontline";
import type { Organization } from "@/types/organization";

export const isOrgAdmin = (org: Organization | undefined): boolean =>
  org?.role === "owner" || org?.role === "admin";

/** "Front counter · Clovis · Poke House" — the place only when the kiosk has one. */
export function kioskSummary(device: BoundKioskDevice): string {
  return [device.device, device.location?.name, device.orgName].filter(Boolean).join(" · ");
}

/**
 * A name for the app a kiosk opens, read off its `returnTo` — the device probe
 * carries the URL, not the app. Either custom-app URL scheme counts: the
 * app-host path (`/customer-apps/<org>/<slug>/`) or the custom-app subdomain
 * (`<org>--<slug>.customer-apps.…`). `null` when the URL is neither, so the
 * caller can fall back to a generic label.
 */
export function kioskAppName(returnTo: string | null): string | null {
  if (!returnTo) return null;
  let url: URL;
  try {
    url = new URL(returnTo);
  } catch {
    return null;
  }
  const byPath = url.pathname.match(/^\/customer-apps\/[^/]+\/([^/]+)/)?.[1];
  // `[<env>--]<org>--<slug>`: the slug is the last `--` part of the first label.
  const hostParts = url.hostname.match(/^([^.]+)\.customer-apps\./)?.[1].split("--") ?? [];
  const byHost = hostParts.length >= 2 ? hostParts[hostParts.length - 1] : undefined;
  const slug = byPath ?? byHost;
  if (!slug) return null;
  return decodeURIComponent(slug)
    .split(/[-_]+/)
    .filter(Boolean)
    .map((word) => word[0].toUpperCase() + word.slice(1))
    .join(" ");
}

/**
 * The org whose Settings → Crew a "this browser isn't a kiosk" page points at:
 * the first the viewer administers, else the first they belong to (whose
 * settings will say what they may see), else none.
 */
export function crewSettingsOrg(orgs: Organization[] | undefined): Organization | undefined {
  return orgs?.find(isOrgAdmin) ?? orgs?.[0];
}

/** `/<org>?settings=organization.crew` — the org root forwards the section into its workspace. */
export const crewSettingsHref = (org: Organization): string =>
  `${ROUTES.ORG(org.slug).ROOT}?settings=organization.crew`;
