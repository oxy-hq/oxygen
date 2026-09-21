import type { AssignmentDraft, WorkerAssignment } from "./operatingGraph";

/**
 * Frontline (crew) sign-in — restaurant staff on a shared kiosk tablet who have
 * no email and no Oxygen account. An HttpOnly kiosk cookie binds the browser to
 * one org; the worker taps a name and enters a PIN.
 */

/** `GET /frontline/device` when this browser holds no kiosk cookie. */
export interface UnboundKioskDevice {
  bound: false;
}

/** `GET /frontline/device` for an enrolled kiosk. */
export interface BoundKioskDevice {
  bound: true;
  /** Org slug — the `org` every roster read and login is scoped to. */
  org: string;
  orgName: string;
  /** The enrolled device's display name, e.g. "Front counter". */
  device: string;
  /**
   * Where the tablet sits, when the admin said — one of the org's locations.
   * Absent on servers older than the operating graph.
   *
   * It also decides the roster: `GET /frontline/roster` lists only workers
   * assigned at this location. A kiosk with no location gets the org-wide list.
   */
  location?: { id: string; name: string } | null;
  /**
   * Seconds of inactivity after which a crew session should sign itself out.
   * The platform only carries the number — nothing server-side watches a clock,
   * so an app in crew mode has to arm its own timer with this. Always present
   * (the server resolves an unset column to its default, 1800 — 30 minutes);
   * optional here only for servers older than 2026-09-11.
   */
  idleTimeoutSeconds?: number;
  /**
   * The app this kiosk was enrolled for, or null. Still goes through the
   * return-to allowlist before the browser is sent there.
   */
  returnTo: string | null;
}

export type KioskDevice = UnboundKioskDevice | BoundKioskDevice;

export interface FrontlineStaff {
  identifier: string;
  name: string;
}

/**
 * `GET /frontline/roster?org=`. Empty — never an error — when the device isn't
 * bound to that org.
 */
export interface FrontlineRosterResponse {
  staff: FrontlineStaff[];
}

export interface FrontlineLoginRequest {
  org: string;
  identifier: string;
  pin: string;
}

/**
 * `POST /frontline/login`. The server sets the session cookie itself; the token
 * is informational on this page. A worker is not a platform user, so this
 * deliberately carries no `UserInfo` and must never feed `AuthContext.login`.
 */
export interface FrontlineLoginResponse {
  token: string;
  name: string;
  expires_in: number;
}

// ── Org admin: the workers and kiosks an org runs. Every route below is
// `/orgs/{orgId}/frontline/*` and needs an org-admin session.

export type FrontlineWorkerStatus = "active" | "suspended";

/** One row of `GET /orgs/{orgId}/frontline/workers`, sorted by name. */
export interface FrontlineWorker {
  user_id: string;
  name: string;
  /** What the worker is known by on the kiosk — an employee number, a short handle. */
  identifier: string;
  status: FrontlineWorkerStatus;
  created_at: string;
  /** Ids of this org's custom apps the worker holds a grant on. */
  apps: string[];
  /** Set while too many wrong PINs have locked sign-in; a PIN reset clears it. */
  locked_until: string | null;
  /** Where they work: the positions they hold, at which places. */
  assignments: WorkerAssignment[];
}

export interface ListWorkersResponse {
  workers: FrontlineWorker[];
}

/**
 * `POST /orgs/{orgId}/frontline/workers`. The PIN travels once, here, and is
 * never echoed back — the response deliberately has no `pin` field.
 */
export interface EnrolWorkerRequest {
  name: string;
  identifier: string;
  /** 4–8 digits. */
  pin: string;
  apps: string[];
  /** Validated before the worker exists: a bad row means no worker. */
  assignments?: AssignmentDraft[];
}

export interface EnrolledWorker {
  user_id: string;
  identifier: string;
  name: string;
  apps: string[];
}

/** `PATCH /orgs/{orgId}/frontline/workers/{userId}` with `{ active }`. */
export interface WorkerStandingResponse {
  user_id: string;
  active: boolean;
  /** False when the worker was already in that standing. */
  changed: boolean;
}

/** `PUT /orgs/{orgId}/frontline/workers/{userId}/apps` — a full replace. */
export interface WorkerAppsResponse {
  apps: string[];
}

/**
 * One row of `GET /orgs/{orgId}/frontline/devices`, newest first. The state is
 * derived, not stored: `revoked_at` set → revoked; `bound_at` set → a tablet
 * holds the cookie; otherwise the enrol link is live until `enrol_expires_at`.
 */
export interface KioskDeviceRow {
  id: string;
  name: string;
  /** Where the tablet lands after sign-in — an allowed absolute URL, or null for home. */
  return_to: string | null;
  created_at: string;
  bound_at: string | null;
  last_seen_at: string | null;
  revoked_at: string | null;
  enrol_expires_at: string | null;
  /** The place this tablet sits at, or null when it was enrolled without one. */
  location_id: string | null;
  location_name: string | null;
  /** Seconds of inactivity before the app signs the shift out; the default (1800) when unset. */
  idle_timeout_seconds: number;
}

export interface ListDevicesResponse {
  devices: KioskDeviceRow[];
}

export interface CreateKioskDeviceRequest {
  name: string;
  return_to?: string;
  /** The tablet's place — and, through it, whose names its crew picker shows. */
  location_id?: string | null;
  /**
   * Seconds of inactivity before the app closes the shift session. Omit to
   * leave it at the platform default (1800 — 30 minutes), which is also what
   * keeps the kiosk on the default if that ever changes again; 30 s to 12 h,
   * refused with a 400 outside that. A negative or fractional number never
   * reaches that check: the server's JSON extractor refuses it with a 422 and
   * a plain-text body, so there is no `error` field to show.
   */
  idle_timeout_seconds?: number;
}

/**
 * `PATCH /orgs/{orgId}/frontline/devices/{id}` — changing an enrolled kiosk
 * without revoking the tablet and walking a new link out to the counter.
 *
 * Every field is optional and **absent means "leave it alone"**, so a body
 * carrying one field cannot blank the others. Answers the updated
 * `KioskDeviceRow`; 400 for a blank name or a whole-second timeout outside
 * 30 s … 12 h, 404 for another org's device, 409 for a revoked one (its row is
 * the record of which tablet a shift was signed in on, so bringing it back is
 * enrolling it). A negative or fractional timeout is a 422 from the JSON
 * extractor, with a plain-text body — no `error` field — so a client that
 * shows `error` must not send one (`idleTimeoutFromMinutes` refuses anything
 * but whole minutes in range before a request is made).
 *
 * Deliberately narrow: `return_to` and `location_id` are not editable here —
 * both change what a bound tablet does next, which is a different act from
 * tuning a number and stays revoke-and-enrol.
 */
export interface UpdateKioskDeviceRequest {
  name?: string;
  /**
   * Seconds of inactivity before the app signs the shift out.
   *
   * **`null` is not the same as omitting this.** Omitted leaves the row alone;
   * `null` clears the column so the kiosk follows the platform default (1800 —
   * 30 minutes) again, including if that default ever moves. Sending 1800
   * instead would freeze today's number into the row.
   */
  idle_timeout_seconds?: number | null;
}

/**
 * `POST /orgs/{orgId}/frontline/devices`, and `…/devices/{id}/enrol-link` for a
 * replacement. `enrol_url` is shown once: the server keeps only a hash of the
 * token, so no later read can reproduce it — a lost link is replaced instead.
 */
export interface CreatedKioskDevice {
  id: string;
  name: string;
  enrol_url: string;
  bind_path: string;
  expires_at: string;
}
