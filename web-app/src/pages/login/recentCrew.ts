import type { BoundKioskDevice } from "@/types/frontline";

/**
 * Who signed in on this kiosk lately — the list behind the name board's
 * "On this tablet recently" row.
 *
 * Kept in this browser's `localStorage` and nowhere else: the browser IS the
 * tablet, the row is a convenience for the people standing at it, and the
 * server has no business keeping a sign-in history for it. Identifiers only —
 * never a name (the roster supplies the current one) and never a PIN. The
 * picker shows only identifiers that are still on today's roster, so someone
 * who left the store does not linger here (`recentRow`).
 *
 * Keyed by the org, the store and the kiosk's name, so a tablet re-enrolled to
 * another store starts its row afresh instead of showing strangers.
 */

const PREFIX = "oxy:crew_recent:";

/**
 * How many to keep. The row shows four; the spares are there so that when one
 * of the four leaves the roster, the next most recent still fills the row.
 */
const KEEP = 8;

export function recentCrewKey(device: Pick<BoundKioskDevice, "org" | "location" | "device">) {
  return `${PREFIX}${device.org}:${device.location?.id ?? "-"}:${device.device}`;
}

/** Newest first. Anything unreadable — or a browser that refuses storage — is nobody. */
export function readRecentCrew(device: Pick<BoundKioskDevice, "org" | "location" | "device">) {
  try {
    const raw = localStorage.getItem(recentCrewKey(device));
    if (!raw) {
      return [];
    }
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) && parsed.every((id) => typeof id === "string") ? parsed : [];
  } catch (error: unknown) {
    console.warn("Could not read this kiosk's recent sign-ins; showing none", error);
    return [];
  }
}

/**
 * Call on a SUCCESSFUL sign-in only: a wrong PIN must not promote a name to
 * the top of the board. Moves `identifier` to the front, once.
 */
export function rememberCrewSignIn(
  device: Pick<BoundKioskDevice, "org" | "location" | "device">,
  identifier: string
) {
  const next = [identifier, ...readRecentCrew(device).filter((id) => id !== identifier)].slice(
    0,
    KEEP
  );
  try {
    localStorage.setItem(recentCrewKey(device), JSON.stringify(next));
  } catch (error: unknown) {
    // Full or disabled storage costs the "recently" row, never the sign-in.
    console.warn("Could not remember this sign-in on the kiosk", error);
  }
}
