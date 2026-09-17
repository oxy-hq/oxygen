/**
 * The operating graph — the platform's model of the physical world an
 * operator runs: places, the positions people hold at them, and who holds
 * which position where. Industry-neutral by construction; see
 * `internal-docs/operating-graph.md`.
 *
 * Every route is `/orgs/{orgId}/...`; reads need an org member, writes an
 * org admin. Every error body is `{ error: string }`.
 */

export type LocationStatus = "pre_launch" | "launching" | "open" | "archived" | "terminated";

/** One row of `GET /orgs/{orgId}/locations`. */
export interface LocationRow {
  id: string;
  org_id: string;
  name: string;
  /** The level this place sits at, in the tenant's own words ("region", "store"). */
  kind: string | null;
  /** One self-reference is the whole hierarchy. */
  parent_id: string | null;
  status: LocationStatus;
  /** IANA zone. */
  timezone: string;
  /**
   * The tenant's own id for this place — what an app keys it by (Store Ops:
   * `santa-clara`). Unique within the org. Not the per-system `external_ids`.
   */
  external_id: string | null;
  /** `system` → id, e.g. `{ toast: "1234" }`. `system` is a lowercase token. */
  external_ids: Record<string, string>;
  created_at: string;
  updated_at: string;
}

export interface ListLocationsResponse {
  locations: LocationRow[];
}

export interface CreateLocationRequest {
  name: string;
  kind?: string;
  parent_id?: string;
  status?: LocationStatus;
  timezone?: string;
  external_id?: string;
}

/** `null` clears `kind` / `parent_id`; an omitted field is left alone. */
export interface UpdateLocationRequest {
  name?: string;
  kind?: string | null;
  parent_id?: string | null;
  status?: LocationStatus;
  timezone?: string;
  /** The tenant's own id; `null` clears it. 409 when another location of the org carries it. */
  external_id?: string | null;
}

/** `PUT /orgs/{orgId}/locations/{id}/external-ids/{system}`. */
export interface ExternalIdRow {
  location_id: string;
  system: string;
  external_id: string;
}

/** Where a position applies: at one place, or across the whole org. */
export type RoleScope = "location" | "franchisor";

/** One row of `GET /orgs/{orgId}/roles` — the org's position vocabulary. */
export interface RoleRow {
  id: string;
  org_id: string;
  name: string;
  scope: RoleScope;
  created_at: string;
  updated_at: string;
}

export interface CreateRoleRequest {
  name: string;
  scope: RoleScope;
}

/** An org member with an account, or a frontline (crew) worker with a PIN. */
export type PersonKind = "member" | "frontline";

/** One row of `GET /orgs/{orgId}/assignments`: a person holding a position, at a place or org-wide. */
export interface AssignmentRow {
  id: string;
  user_id: string;
  user_name: string;
  user_kind: PersonKind;
  role_id: string;
  role_name: string;
  role_scope: RoleScope;
  /** Null for an org-wide position, or when the place has since been removed. */
  location_id: string | null;
  location_name: string | null;
  supervisor_id: string | null;
  supervisor_name: string | null;
  created_at: string;
}

export interface ListAssignmentsResponse {
  assignments: AssignmentRow[];
}

/** Query filter for `GET assignments`; both optional, both narrow. */
export interface AssignmentsFilter {
  user_id?: string;
  location_id?: string;
}

/**
 * `POST /orgs/{orgId}/assignments`. Idempotent: 201 when created, 200 when the
 * same person already holds that position there. A `location` role needs a
 * `location_id`; a `franchisor` role refuses one.
 */
export interface CreateAssignmentRequest {
  user_id: string;
  role_id: string;
  location_id?: string | null;
  supervisor_id?: string | null;
}

/** An assignment as it rides on a frontline worker row — no names for the person, who is the row. */
export interface WorkerAssignment {
  id: string;
  role_id: string;
  role_name: string;
  role_scope: RoleScope;
  location_id: string | null;
  location_name: string | null;
  supervisor_id: string | null;
}

/** One "where they work" row sent with `POST frontline/workers`, before the worker exists. */
export interface AssignmentDraft {
  role_id: string;
  location_id?: string | null;
  supervisor_id?: string | null;
}
