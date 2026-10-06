import type {
  BlockedOrg,
  Grant,
  RoleCeiling,
  Token,
  TokenKind,
  TokenSummary
} from "@/types/apiToken";

/** The ceiling names people see. `owner` is "no cap", so it reads as Full. */
export const CEILING_LABELS: Record<RoleCeiling, string> = {
  viewer: "Read",
  member: "Write",
  admin: "Admin",
  owner: "Full"
};

/**
 * What each kind of credential is called, wherever a list names it. A legacy API key is not a
 * token; it is named here because an org's inventory lists both, apart.
 */
export const TOKEN_KIND_LABELS: Record<TokenKind, string> = {
  personal: "Personal",
  legacy_key: "Legacy API key",
  service_account: "Service account",
  // Minted from a GitHub Actions run through a trusted-access policy.
  ci: "Trusted access"
};

/** Lowest to highest, the order the ceiling control lists them in. */
export const CEILINGS: readonly RoleCeiling[] = ["viewer", "member", "admin", "owner"];

/** `oxy_pat_Ab3x…wxyz`. The server may already end the prefix with an ellipsis; show one. */
export const maskedToken = (token: Pick<Token, "display_prefix" | "last_four">): string => {
  const prefix = token.display_prefix.replace(/(…|\.{3})$/, "");
  return token.last_four ? `${prefix}…${token.last_four}` : `${prefix}…`;
};

/** A `Token` in the shape the shared status badge, Extend popover and Activity drawer read. */
export const toTokenSummary = (
  token: Pick<Token, "id" | "name" | "expires_at" | "status" | "kind" | "display_prefix"> & {
    last_four?: string;
  }
): TokenSummary => ({
  id: token.id,
  name: token.name,
  expires_at: token.expires_at,
  is_active: token.status !== "revoked",
  masked_key: maskedToken({
    display_prefix: token.display_prefix,
    last_four: token.last_four ?? ""
  }),
  kind: token.kind
});

export type Standing = "platform" | "partner";

export interface AccessSummary {
  /** "All access", "3 workspaces in 2 orgs". */
  label: string;
  /** Standing the token carries, shown as badges. */
  standing: Standing[];
  /** Orgs whose policy refuses this token: the warning chip. */
  blocked: BlockedOrg[];
  /** One line per grant, for the hover detail. */
  lines: string[];
}

const plural = (count: number, noun: string): string => `${count} ${noun}${count === 1 ? "" : "s"}`;

const live = (grant: Grant): boolean => !grant.revoked_at;

const distinctOrgs = (grants: Grant[]): string[] => [...new Set(grants.map((g) => g.org_id))];

const grantLine = (grant: Grant): string => {
  if (grant.kind === "app_publish") {
    return `${grant.org_name}: publish ${grant.app_name ?? "an app"}`;
  }
  const where =
    grant.workspace_id === null ? "every workspace" : (grant.workspace_name ?? "a workspace");
  const ceiling = grant.role_ceiling ? CEILING_LABELS[grant.role_ceiling] : CEILING_LABELS.owner;
  const line = `${grant.org_name}: ${where} (${ceiling})`;
  return live(grant) ? line : `${line}, removed by the organization`;
};

/** "3 workspaces in 2 orgs", "All workspaces in Acme", or both, plus any app-publish grants. */
const grantsLabel = (grants: Grant[]): string => {
  const active = grants.filter(live);
  const wide = active.filter((g) => g.kind === "workspace" && g.workspace_id === null);
  const narrow = active.filter((g) => g.kind === "workspace" && g.workspace_id !== null);
  const apps = active.filter((g) => g.kind === "app_publish");

  const parts: string[] = [];
  if (narrow.length > 0) {
    parts.push(
      `${plural(narrow.length, "workspace")} in ${plural(distinctOrgs(narrow).length, "org")}`
    );
  }
  if (wide.length > 0) {
    const orgs = distinctOrgs(wide);
    const scope = orgs.length === 1 ? wide[0].org_name : plural(orgs.length, "org");
    parts.push(narrow.length > 0 ? `all of ${scope}` : `All workspaces in ${scope}`);
  }
  if (apps.length > 0) parts.push(`publish ${plural(apps.length, "app")}`);
  // Every grant was taken away by its org: the token still exists and reaches nothing.
  return parts.length > 0 ? parts.join(", ") : "No access";
};

/**
 * What a personal access token can reach, in the words of the list's Access column. Legacy API
 * keys never reach this: `/user/tokens` does not return them.
 */
export const summarizeAccess = (
  token: Pick<Token, "all_access" | "platform" | "partner" | "grants" | "blocked_orgs">
): AccessSummary => {
  const blocked = token.blocked_orgs ?? [];
  const standing: Standing[] = [
    ...(token.platform ? (["platform"] as const) : []),
    ...(token.partner ? (["partner"] as const) : [])
  ];
  const grants = token.grants ?? [];
  if (token.all_access) {
    // The server lists an all-access token's grants only where an org ended its reach: history
    // worth showing, since "all access" no longer includes that org.
    const removed = grants.filter((grant) => !live(grant));
    return {
      label: "All access",
      standing,
      blocked,
      lines: [
        "Every organization and workspace you can reach, now and later.",
        ...removed.map(grantLine)
      ]
    };
  }
  return { label: grantsLabel(grants), standing, blocked, lines: grants.map(grantLine) };
};
