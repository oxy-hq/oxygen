import type { Grant, Token, TokenOwner } from "@/types/apiToken";

// Fixtures for this page's tests: the tokens `GET /admin/standing-tokens` lists.

const HOUR = 60 * 60 * 1000;
export const inHours = (hours: number) => new Date(Date.now() + hours * HOUR).toISOString();
export const inDays = (days: number) => inHours(days * 24 + 12);

export const workspaceGrant = (org: string, workspace: string | null): Grant => ({
  id: `g-${org}-${workspace ?? "all"}`,
  kind: "workspace",
  org_id: `o-${org}`,
  org_name: org,
  workspace_id: workspace ? `w-${workspace}` : null,
  workspace_name: workspace,
  role_ceiling: "admin",
  app_id: null,
  app_name: null,
  revoked_at: null
});

export const ADA: TokenOwner = { type: "user", id: "u-ada", label: "ada@oxy.tech" };
export const LIN: TokenOwner = { type: "user", id: "u-lin", label: "lin@oxy.tech" };
export const JO: TokenOwner = { type: "user", id: "u-jo", label: "jo@harbor.example" };

/** What `oxyc login` makes for a staff member: all-access, staff standing, 90 days. */
export const token = (over: Partial<Token> = {}): Token => ({
  id: "t1",
  name: "oxyc on ada-studio",
  kind: "personal",
  display_prefix: "oxy_pat_Ab3x",
  last_four: "wxyz",
  all_access: true,
  platform: true,
  partner: false,
  grants: [],
  // Half a day past the day, so the countdown does not tick over mid-test.
  expires_at: inDays(88),
  last_used_at: inHours(-0.2),
  created_at: inHours(-30),
  revoked_at: null,
  status: "active",
  source: "oxyc",
  owner: ADA,
  blocked_orgs: [],
  ...over
});

/** A partner's token made in Settings and scoped to one client org. */
export const partnerToken = (over: Partial<Token> = {}): Token =>
  token({
    id: "t-partner",
    name: "client onboarding",
    platform: false,
    partner: true,
    all_access: false,
    grants: [workspaceGrant("Rivermark", null)],
    source: "ui",
    owner: JO,
    ...over
  });

export const expired = (over: Partial<Token> = {}): Token =>
  token({
    id: "t-expired",
    name: "oxyc on old-laptop",
    status: "expired",
    expires_at: inHours(-50),
    ...over
  });

export const revoked = (over: Partial<Token> = {}): Token =>
  token({
    id: "t-revoked",
    name: "oxyc on lost-laptop",
    status: "revoked",
    revoked_at: inHours(-3.1),
    ...over
  });
