export type OrgRole = "owner" | "admin" | "member";
export type WorkspaceRole = "owner" | "admin" | "member" | "viewer";

export interface Organization {
  id: string;
  name: string;
  slug: string;
  role: OrgRole;
  created_at?: string;
  /** Bumped on any org update (incl. logo upload/remove); used to
   *  cache-bust the workspace logo <img>. */
  updated_at?: string;
  workspace_count?: number;
  member_count?: number;
  /** The server's answer to "may this person create a workspace here": an
   *  org owner or admin who is also Oxy staff with `operate_platform` over this
   *  org. Absent means unknown, which reads as no. */
  may_provision_workspace?: boolean;
}

export interface OrgMember {
  id: string;
  user_id: string;
  email: string;
  name: string;
  role: OrgRole;
  created_at: string;
}

export interface WorkspaceMember {
  user_id: string;
  email: string;
  name: string;
  org_role: OrgRole;
  workspace_role: WorkspaceRole;
  is_override: boolean;
}

export interface OrgInvitation {
  id: string;
  email: string;
  role: OrgRole;
  token: string;
  status: "pending" | "accepted" | "expired";
  expires_at: string;
  created_at: string;
  /**
   * Past `expires_at`: the link no longer works and the row is waiting to be
   * revoked or superseded by a fresh invite. Don't infer this from `status` —
   * nothing transitions a row to `"expired"`, so a lapsed invite reports
   * `"pending"` forever. The server derives this from `expires_at`.
   */
  is_expired: boolean;
}

export interface MyInvitation {
  id: string;
  token: string;
  role: OrgRole;
  expires_at: string;
  created_at: string;
  org_id: string;
  org_name: string;
  org_slug: string;
  invited_by_name?: string;
}
