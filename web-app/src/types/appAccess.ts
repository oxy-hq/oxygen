/**
 * Teams and custom-app access.
 *
 * A grant is modelled as a tagged union over its GRANTEE, not as two parallel
 * lists — the panel shows one access list, and a team and a person read as the
 * same kind of row with different metadata.
 */

/** What a grant buys. `admin` also unlocks the app's own privileged surface. */
export type GrantRole = "admin" | "member";

/** `org` — any org member can open the app. `members` — only grantees. */
export type AppVisibility = "org" | "members";

export interface Team {
  id: string;
  name: string;
  description: string | null;
  member_count: number;
  created_at: string;
}

interface TeamMember {
  user_id: string;
  email: string;
  name: string;
  /** The person's ORG role. Team membership never changes it. */
  org_role: string;
  added_at: string;
}

export interface TeamDetail extends Team {
  members: TeamMember[];
}

/** One row in the org's "who can open what" list. */
export interface AppAccessSummary {
  id: string;
  name: string;
  slug: string;
  visibility: AppVisibility;
  /** Grants of both kinds. Zero on a restricted app means officers only. */
  grant_count: number;
  published: boolean;
}

interface Grant {
  kind: "user" | "team";
  id: string;
  name: string;
  /** Users only. */
  email: string | null;
  role: GrantRole;
  /** Teams only — how many people the grant actually reaches. */
  member_count: number | null;
}

export interface AppAccess {
  app_id: string;
  visibility: AppVisibility;
  grants: Grant[];
}

/**
 * Someone who could be granted an app. Normalized across the three sources that
 * can supply it (org members, the admin app-members route, the partner console),
 * so one picker component consumes any of them.
 */
export interface GrantablePerson {
  user_id: string;
  email: string;
  name: string;
  /** Their ORG role, shown for context. Granting an app never changes it. */
  role: string;
}

/** The write shape — mirrors {@link Grant} with only what the server needs. */
export interface GranteeRef {
  kind: "user" | "team";
  id: string;
  role: GrantRole;
}

export interface SetAppAccessRequest {
  visibility: AppVisibility;
  grants: GranteeRef[];
}
