// Staff-facing partner provisioning DTOs (`/api/admin/partners`). Distinct from
// `types/partners.ts`, which describes the partner-console self-service surface.
//
// The model changed on 2026-07-14 (see the permission-model design): a partner is
// no longer its own entity — **a partner IS an org that holds a grant**. So there
// is no partner id/name/slug of its own; everything comes from the org.

/**
 * The **ceiling** — what Oxy permits this partner AT ALL. Every role the partner
 * hands out to its own people is intersected with this, so a permission missing
 * here is inert no matter who gets which role.
 */
export interface AdminPartnerCapabilities {
  manage_members: boolean;
  /** Publish / unpublish apps only — NOT data access. */
  manage_apps: boolean;
  /** The custom-app data plane (query / semantic-query / agent runs). Default OFF. */
  develop_apps: boolean;
  view_audit: boolean;
  manage_billing: boolean;
  manage_secrets: boolean;
  /** Onboard client orgs. Sensitive — it mints billable tenants. Default OFF. */
  create_orgs: boolean;
  manage_org_settings: boolean;
}

export interface AdminPartnerSummary {
  /** The partner IS this org. */
  org_id: string;
  name: string;
  slug: string;
  status: string; // "active" | "suspended"
  managed_count: number;
  created_at: string;
}

interface AdminPartnerOrgLink {
  org_id: string;
  org_name: string | null;
  org_slug: string | null;
  attached_at: string;
}

/** One member of the partner org. Staff see everyone, flagged by access. */
interface AdminPartnerPerson {
  /** Their membership in the partner org — the access row's key. */
  org_member_id: string;
  user_id: string;
  email: string;
  /** Their role in the partner org itself (owner/admin/member). */
  org_role: string;
  /** Whether they are a partner operator (reach every client, within the ceiling). */
  has_access: boolean;
}

export interface AdminPartnerDetail {
  org_id: string;
  name: string;
  slug: string;
  status: string;
  created_at: string;
  /** The ceiling. */
  capabilities: AdminPartnerCapabilities;
  managed_orgs: AdminPartnerOrgLink[];
  people: AdminPartnerPerson[];
}

/** Grant a partnership to an existing org. */
export interface GrantPartnershipInput {
  partner_org_id: string;
  capabilities?: AdminPartnerCapabilities;
  first_client_org_id?: string;
  /** Must already be a member of the partner org. */
  partner_admin_email?: string;
}

/**
 * Default ceiling for a new partnership. members / apps / audit / onboarding on;
 * data, billing, secrets off. `create_orgs` (onboard clients) is on because
 * that's the point of a reseller channel — staff can still toggle it off here
 * before granting. Mirrors the backend `sane_default`.
 */
export const DEFAULT_PARTNER_CEILING: AdminPartnerCapabilities = {
  manage_members: true,
  manage_apps: true,
  develop_apps: false,
  view_audit: true,
  manage_billing: false,
  manage_secrets: false,
  create_orgs: true,
  manage_org_settings: false
};

/** Only a Global Owner may grant these two — the server enforces it. */
export const OWNER_ONLY_CAPABILITIES: (keyof AdminPartnerCapabilities)[] = [
  "manage_billing",
  "manage_secrets"
];

export const CAPABILITY_LABELS: Record<keyof AdminPartnerCapabilities, string> = {
  manage_members: "Manage members",
  manage_apps: "Publish apps",
  develop_apps: "App data access",
  view_audit: "View audit log",
  manage_billing: "Manage billing",
  manage_secrets: "Manage secrets",
  create_orgs: "Onboard clients",
  manage_org_settings: "Change org settings"
};
