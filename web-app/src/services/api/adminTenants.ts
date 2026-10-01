import { isAxiosError } from "axios";
import type { PlatformRoleId } from "@/types/access";
import { apiClient } from "./axios";

// Shared types

export type WorkspaceStatusId = "ready" | "cloning" | "failed" | "not_oxy_project";
export type UserStatusId = "active" | "deleted";
export type OrgRoleId = "owner" | "admin" | "member";

// Organizations (admin meta surface)

/** The partner managing an org (at most one — `partner_orgs.org_id` is unique). */
interface OrgPartnerRef {
  id: string;
  name: string;
  slug: string;
}

export interface AdminOrgMeta {
  id: string;
  name: string;
  slug: string;
  created_at: string;
  member_count: number;
  workspace_count: number;
  owner_email: string | null;
  /** The partner that MANAGES this org. */
  partner: OrgPartnerRef | null;
  /**
   * This org IS a partner. Distinct from `partner` above (its manager) — the
   * Tenants directory lists the two populations separately, so conflating them
   * would file an agency under its own client list.
   */
  is_partner: boolean;
}

export interface AdminOrgDetail extends AdminOrgMeta {
  updated_at: string;
  owners: AdminOrgUserSummary[];
  workspaces: AdminOrgWorkspaceSummary[];
}

interface AdminOrgUserSummary {
  user_id: string;
  email: string;
  name: string;
  role: string;
}

interface AdminOrgWorkspaceSummary {
  id: string;
  name: string;
  status: WorkspaceStatusId;
  created_at: string;
}

export interface ListOrgsMetaQuery {
  search?: string;
  page?: number;
  page_size?: number;
}

export interface RenameOrgBody {
  name?: string;
  slug?: string;
}

export interface AdminCreateOrgBody {
  name: string;
  /** Optional; the server derives it from `name` when omitted. */
  slug?: string;
  owner_email: string;
}

/** `seeded` = the email was an existing user, added as Owner now. `invited` =
 *  an Owner-role invitation was created and emailed. */
type AdminOrgOwnerStatus = "seeded" | "invited";

export interface AdminCreateOrgResponse {
  org: AdminOrgMeta;
  owner_status: AdminOrgOwnerStatus;
  /** Echo of the normalized owner email, for the success message. */
  owner_email: string;
}

// Org bare subdomain (`<org-slug>.<zone>`) — Oxy-staff control.
interface AdminOrgSubdomainWorkspace {
  id: string;
  name: string;
  status: WorkspaceStatusId;
}

export interface AdminOrgSubdomainResponse {
  enabled: boolean;
  /** The org slug — this is the subdomain label (not editable). */
  subdomain: string;
  /** `https://<slug>.<zone>/`; null when the zone isn't derivable. */
  url: string | null;
  default_workspace_id: string | null;
  /** The org's workspaces, for the default-project dropdown. */
  workspaces: AdminOrgSubdomainWorkspace[];
  /** True when the slug collides with a reserved infra label (can't enable). */
  reserved: boolean;
}

export interface SetAdminOrgSubdomainBody {
  enabled: boolean;
  default_workspace_id?: string | null;
}

export const AdminOrgsService = {
  async list(query: ListOrgsMetaQuery = {}): Promise<AdminOrgMeta[]> {
    const res = await apiClient.get<AdminOrgMeta[]>("/admin/orgs-meta", { params: query });
    return res.data;
  },

  /** Create an org and onboard its owner in one step (POST /admin/orgs). */
  async create(body: AdminCreateOrgBody): Promise<AdminCreateOrgResponse> {
    const res = await apiClient.post<AdminCreateOrgResponse>("/admin/orgs", body);
    return res.data;
  },

  async detail(orgId: string): Promise<AdminOrgDetail> {
    const res = await apiClient.get<AdminOrgDetail>(`/admin/orgs/${orgId}/detail`);
    return res.data;
  },

  async rename(orgId: string, body: RenameOrgBody): Promise<AdminOrgMeta> {
    const res = await apiClient.patch<AdminOrgMeta>(`/admin/orgs/${orgId}`, body);
    return res.data;
  },

  async getSubdomain(orgId: string): Promise<AdminOrgSubdomainResponse> {
    const res = await apiClient.get<AdminOrgSubdomainResponse>(`/admin/orgs/${orgId}/subdomain`);
    return res.data;
  },

  async setSubdomain(
    orgId: string,
    body: SetAdminOrgSubdomainBody
  ): Promise<AdminOrgSubdomainResponse> {
    const res = await apiClient.put<AdminOrgSubdomainResponse>(
      `/admin/orgs/${orgId}/subdomain`,
      body
    );
    return res.data;
  },

  async remove(orgId: string): Promise<void> {
    await apiClient.delete(`/admin/orgs/${orgId}`);
  },

  /**
   * Fetch the org's uploaded logo bytes. Returns `null` when the org has no
   * logo (404). Fetched via the API client so the admin JWT rides along — no
   * public image endpoint — hence the blob-then-data-URL dance in the hook.
   */
  async getLogo(orgId: string): Promise<Blob | null> {
    try {
      const res = await apiClient.get(`/admin/orgs/${orgId}/logo`, { responseType: "blob" });
      return res.data instanceof Blob && res.data.size > 0 ? res.data : null;
    } catch (err) {
      if (isAxiosError(err) && err.response?.status === 404) return null;
      throw err;
    }
  },

  async uploadLogo(orgId: string, file: File): Promise<void> {
    await apiClient.put(`/admin/orgs/${orgId}/logo`, file, {
      headers: { "Content-Type": file.type }
    });
  },

  async deleteLogo(orgId: string): Promise<void> {
    await apiClient.delete(`/admin/orgs/${orgId}/logo`);
  },

  async transferOwnership(orgId: string, newOwnerUserId: string): Promise<void> {
    await apiClient.post(`/admin/orgs/${orgId}/transfer-ownership`, {
      new_owner_user_id: newOwnerUserId
    });
  }
};

// Users (admin meta surface)

/** A partner this user operates. `id` is the partner's ORG id. */
interface UserPartnerRef {
  id: string;
  name: string;
}

export interface AdminUserRow {
  id: string;
  email: string;
  name: string;
  status: UserStatusId;
  created_at: string;
  last_login_at: string;
  /** Holds a platform grant of ANY role — cannot distinguish them. Prefer `platform_role`. */
  is_app_admin: boolean;
  /** `global_admin` | `app_operator`, or null for a non-staff user. */
  platform_role: PlatformRoleId | null;
  /** True = the grant reaches every org. Meaningless when `platform_role` is null. */
  platform_scope_all: boolean;
  /** Orgs a bounded grant reaches; 0 when unbounded or non-staff. */
  platform_scope_org_count: number;
  org_count: number;
  /** Non-empty ⇒ Partner Admin — a delegated cross-org authority that org_count alone hides. */
  partners: UserPartnerRef[];
  /** Highest org role ("owner" | "admin" | "member") — tenant hierarchy, not just a count. */
  top_org_role: string | null;
}

export interface AdminUserDetail extends AdminUserRow {
  picture: string | null;
  email_verified: boolean;
  org_memberships: AdminUserOrgMembership[];
  workspace_memberships: AdminUserWorkspaceMembership[];
  /** Outstanding invitations addressed to this email — access offered, not taken up. */
  invitations: AdminUserInvitation[];
}

/**
 * Matched to the user by email, not user id: an invitation predates the account
 * and may never get one. Most people stuck behind a lapsed invite have no user
 * row at all — for those, the org's own detail page is the place they show up.
 */
interface AdminUserInvitation {
  id: string;
  org_id: string;
  org_slug: string;
  org_name: string;
  role: string;
  created_at: string;
  expires_at: string;
  /** Derived server-side from `expires_at` — a lapsed invite still reads as "pending". */
  is_expired: boolean;
}

interface AdminUserOrgMembership {
  org_id: string;
  org_slug: string;
  org_name: string;
  role: string;
  joined_at: string;
}

interface AdminUserWorkspaceMembership {
  workspace_id: string;
  workspace_name: string;
  role: string;
  joined_at: string;
}

/** Values the `role` filter accepts: a specific platform role, or any staff standing. */
export type UserRoleFilter = PlatformRoleId | "staff";

export interface ListUsersQuery {
  search?: string;
  status?: UserStatusId;
  role?: UserRoleFilter;
  page?: number;
  page_size?: number;
}

export const AdminUsersService = {
  async list(query: ListUsersQuery = {}): Promise<AdminUserRow[]> {
    const res = await apiClient.get<AdminUserRow[]>("/admin/users", { params: query });
    return res.data;
  },

  async detail(userId: string): Promise<AdminUserDetail> {
    const res = await apiClient.get<AdminUserDetail>(`/admin/users/${userId}`);
    return res.data;
  },

  async setStatus(userId: string, status: UserStatusId): Promise<void> {
    await apiClient.patch(`/admin/users/${userId}/status`, { status });
  },

  async addToOrg(userId: string, orgId: string, role: OrgRoleId): Promise<void> {
    await apiClient.post(`/admin/users/${userId}/org-memberships`, { org_id: orgId, role });
  },

  async updateRole(userId: string, orgId: string, role: OrgRoleId): Promise<void> {
    await apiClient.patch(`/admin/users/${userId}/org-memberships/${orgId}`, { role });
  },

  async removeFromOrg(userId: string, orgId: string): Promise<void> {
    await apiClient.delete(`/admin/users/${userId}/org-memberships/${orgId}`);
  },

  /**
   * Admin-scoped on purpose: staff don't pass the tenant's own `OrgAdmin` guard
   * without a live assume session, so the console needs its own route.
   */
  async revokeInvitation(userId: string, invitationId: string): Promise<void> {
    await apiClient.delete(`/admin/users/${userId}/invitations/${invitationId}`);
  }
};

// Workspaces (admin meta surface)

export interface AdminWorkspaceRow {
  id: string;
  name: string;
  status: WorkspaceStatusId;
  created_at: string;
  last_opened_at: string | null;
  org_id: string | null;
  org_slug: string | null;
  member_count: number;
}

export interface AdminWorkspaceDetail extends AdminWorkspaceRow {
  updated_at: string;
  org_name: string | null;
  path: string | null;
  git_remote_url: string | null;
  error: string | null;
  members: AdminWorkspaceMember[];
  current_revision_id: string | null;
}

interface AdminWorkspaceMember {
  user_id: string;
  email: string;
  name: string;
  role: string;
  joined_at: string;
}

export interface ListWorkspacesQuery {
  search?: string;
  status?: WorkspaceStatusId;
  org_id?: string;
  page?: number;
  page_size?: number;
}

export const AdminWorkspacesService = {
  async list(query: ListWorkspacesQuery = {}): Promise<AdminWorkspaceRow[]> {
    const res = await apiClient.get<AdminWorkspaceRow[]>("/admin/workspaces-meta", {
      params: query
    });
    return res.data;
  },

  async detail(workspaceId: string): Promise<AdminWorkspaceDetail> {
    const res = await apiClient.get<AdminWorkspaceDetail>(
      `/admin/workspaces/${workspaceId}/detail`
    );
    return res.data;
  },

  async rename(workspaceId: string, name: string): Promise<void> {
    await apiClient.patch(`/admin/workspaces/${workspaceId}`, { name });
  },

  async remove(workspaceId: string): Promise<void> {
    await apiClient.delete(`/admin/workspaces/${workspaceId}`);
  },

  async transferOrg(workspaceId: string, newOrgId: string): Promise<void> {
    await apiClient.post(`/admin/workspaces/${workspaceId}/transfer-org`, {
      new_org_id: newOrgId
    });
  }
};
