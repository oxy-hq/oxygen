import type { KioskDeviceRow, UpdateKioskDeviceRequest } from "@/types/frontline";

/**
 * What a kiosk row is right now. Derived from its timestamps — the server
 * stores no state column — and shared by the Crew section (badge, count) and
 * the devices query (whether the list is worth polling).
 */
export type KioskState = "waiting" | "bound" | "expired" | "revoked";

export function kioskState(device: KioskDeviceRow, now = Date.now()): KioskState {
  if (device.revoked_at) return "revoked";
  if (device.bound_at) return "bound";
  if (device.enrol_expires_at && new Date(device.enrol_expires_at).getTime() > now) {
    return "waiting";
  }
  return "expired";
}

/**
 * True while some kiosk's enrol link is live and unspent — the window in
 * which a tablet may bind at any moment. The bind happens on that other
 * device, so nothing in the admin's browser would otherwise refetch.
 */
export function awaitingTablet(devices: KioskDeviceRow[], now = Date.now()): boolean {
  return devices.some((device) => kioskState(device, now) === "waiting");
}

/**
 * How long a kiosk may sit untouched before the crew app signs the shift out,
 * when the kiosk names no number of its own.
 *
 * The only client-side copy of `frontline_devices::DEFAULT_IDLE_TIMEOUT_SECONDS`,
 * and it exists to *label* a value the server already resolved — every
 * `idle_timeout_seconds` on the wire is effective, never null — not to decide
 * one. Leaving the field empty sends nothing, so a kiosk enrolled today follows
 * the default if it changes again rather than freezing 30 minutes into its row.
 */
export const DEFAULT_IDLE_TIMEOUT_SECONDS = 30 * 60;

/**
 * The window the server accepts, in the minutes the dialog asks for. Its own
 * floor is 30 seconds; a kiosk is not worth offering half a minute, and one
 * whole minute is the smallest value that cannot land under the floor.
 */
export const IDLE_TIMEOUT_MIN_MINUTES = 1;
export const IDLE_TIMEOUT_MAX_MINUTES = 12 * 60;

const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"}`;

/**
 * An idle timeout as a sentence — "30 minutes (default)", "45 seconds",
 * "1 hour 30 minutes".
 *
 * The default is marked because a kiosk that never named a number and one that
 * asked for 1800 s are indistinguishable on the wire (the server resolves the
 * column before it serializes), and the useful thing to tell an admin reading
 * the list is which tablets are simply following the platform.
 */
export function idleTimeoutLabel(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) {
    return "—";
  }
  const whole = Math.round(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const secs = whole % 60;
  const parts: string[] = [];
  if (hours > 0) {
    parts.push(plural(hours, "hour"));
  }
  if (minutes > 0) {
    parts.push(plural(minutes, "minute"));
  }
  if (secs > 0) {
    parts.push(plural(secs, "second"));
  }
  const spelled = parts.join(" ");
  return whole === DEFAULT_IDLE_TIMEOUT_SECONDS ? `${spelled} (default)` : spelled;
}

/** What the enrol dialog's minutes box means. */
export type IdleTimeoutChoice =
  /** Send nothing: the kiosk follows the platform default, now and later. */
  { kind: "default" } | { kind: "seconds"; seconds: number } | { kind: "invalid"; message: string };

/**
 * Read the minutes box.
 *
 * The server refuses anything outside its own bounds with a 400, so this is
 * not the gate — it is the sentence an admin gets before they wait for one.
 */
export function idleTimeoutFromMinutes(raw: string): IdleTimeoutChoice {
  const trimmed = raw.trim();
  if (trimmed === "") {
    return { kind: "default" };
  }
  const minutes = Number(trimmed);
  const outOfRange =
    !Number.isInteger(minutes) ||
    minutes < IDLE_TIMEOUT_MIN_MINUTES ||
    minutes > IDLE_TIMEOUT_MAX_MINUTES;
  if (outOfRange) {
    return {
      kind: "invalid",
      message: `Whole minutes, ${IDLE_TIMEOUT_MIN_MINUTES} to ${IDLE_TIMEOUT_MAX_MINUTES} — or empty for ${idleTimeoutLabel(DEFAULT_IDLE_TIMEOUT_SECONDS)}.`
    };
  }
  return { kind: "seconds", seconds: minutes * 60 };
}

/**
 * The minutes box for a kiosk that already exists — the editable half of the
 * **Signs out** column.
 *
 * Empty when the kiosk is on the platform default, because that is the same
 * empty box `New kiosk` uses to mean the default, and because the wire cannot
 * tell a NULL column from an explicit 1800: `idle_timeout_seconds` is always
 * the *effective* number. Saving an untouched box therefore sends `null`, which
 * puts a kiosk that had frozen today's default back onto the default itself —
 * the safe direction of that ambiguity.
 *
 * A kiosk set through the API to a number of seconds that is not a whole
 * number of minutes (the server's floor is 30 s; this box's is one minute)
 * shows the nearest minute. Nothing is written unless the admin saves, and the
 * field says "minutes", so the rounding is on screen rather than silent.
 */
export function idleTimeoutMinutesField(seconds: number): string {
  if (seconds === DEFAULT_IDLE_TIMEOUT_SECONDS) {
    return "";
  }
  return String(Math.max(IDLE_TIMEOUT_MIN_MINUTES, Math.round(seconds / 60)));
}

/**
 * The minutes box as the body of `PATCH …/frontline/devices/{id}`, or `null`
 * when there is nothing to send because the box is one the server would refuse.
 *
 * The empty box becomes `idle_timeout_seconds: null` and NOT an omitted field:
 * on a PATCH, absent means "leave the row alone", so omitting it would make
 * "put this tablet back on the default" unsendable — which is the hole that
 * made changing a kiosk mean re-enrolling it. Sending 1800 instead would freeze
 * today's default into the row and leave the tablet behind the next time it
 * moves.
 */
export function idleTimeoutPatch(choice: IdleTimeoutChoice): UpdateKioskDeviceRequest | null {
  switch (choice.kind) {
    case "default":
      return { idle_timeout_seconds: null };
    case "seconds":
      return { idle_timeout_seconds: choice.seconds };
    case "invalid":
      return null;
  }
}
