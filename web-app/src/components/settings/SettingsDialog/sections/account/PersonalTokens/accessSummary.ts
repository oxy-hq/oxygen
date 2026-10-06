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
  ci: "Trusted access",
  // Held by an AI agent building custom apps: it reaches its apps' sandboxes and nothing else.
  sandbox_agent: "Sandbox agent"
};

/**
 * A sandbox agent token is fixed once minted: no rename, no new access, no extend, no
 * regenerate. Any of them answers 409 `sandbox_token_fixed`, so no row offers one.
 */
export const isFixedToken = (token: { kind?: TokenKind }): boolean =>
  token.kind === "sandbox_agent";

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

const removedByOrg = (line: string, grant: Grant): string =>
  live(grant) ? line : `${line}, removed by the organization`;

const grantLine = (grant: Grant): string => {
  if (grant.kind === "app_publish") {
    return `${grant.org_name}: publish ${grant.app_name ?? "an app"}`;
  }
  // Checked before the workspace reading below: an app grant's `workspace_id` is `null` too,
  // and there it must never be read as "every workspace".
  if (grant.kind === "app_sandbox") {
    const line = `${grant.org_name}: sandboxes of ${grant.app_name ?? "an app"}`;
    // Who ended it isn't on the grant, so the line doesn't say.
    return live(grant) ? line : `${line}, no longer covered`;
  }
  const where =
    grant.workspace_id === null ? "every workspace" : (grant.workspace_name ?? "a workspace");
  const ceiling = grant.role_ceiling ? CEILING_LABELS[grant.role_ceiling] : CEILING_LABELS.owner;
  return removedByOrg(`${grant.org_name}: ${where} (${ceiling})`, grant);
};

/** "Sandboxes of Store Ops", "Sandboxes of Store Ops and Refunds", "Sandboxes of 3 apps". */
const sandboxLabel = (grants: Grant[]): string => {
  const apps = grants.filter((grant) => grant.kind === "app_sandbox" && live(grant));
  if (apps.length === 0) return "No access";
  const names = apps.map((grant) => grant.app_name ?? "an app");
  return `Sandboxes of ${names.length <= 2 ? names.join(" and ") : plural(names.length, "app")}`;
};

export interface SandboxGrantApp {
  id: string | null;
  name: string;
  /** `acme/store-ops`, or `null` where the grant names no slug for its org or its app. */
  ref: string | null;
}

/**
 * The `<org>/<app>` reference a grant carries. `null` from a server that predates the slugs, and
 * for an org or app that is gone: the server sends that slug empty.
 */
const grantAppRef = (grant: Grant): string | null =>
  grant.org_slug && grant.app_slug ? `${grant.org_slug}/${grant.app_slug}` : null;

/** The apps a sandbox agent token still reaches, for a cell that names each by its reference. */
export const sandboxGrantApps = (grants: Grant[]): SandboxGrantApp[] =>
  grants
    .filter((grant) => grant.kind === "app_sandbox" && live(grant))
    .map((grant) => ({
      id: grant.app_id,
      name: grant.app_name || "an app",
      ref: grantAppRef(grant)
    }));

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
 * What a personal access token or a sandbox agent token can reach, in the words of the list's
 * Access column. Legacy API keys never reach this: `/user/tokens` does not return them.
 */
export const summarizeAccess = (
  token: Pick<Token, "all_access" | "platform" | "partner" | "grants" | "blocked_orgs"> & {
    kind?: TokenKind;
  }
): AccessSummary => {
  const blocked = token.blocked_orgs ?? [];
  const grants = token.grants ?? [];
  if (token.kind === "sandbox_agent") {
    // Stored with `platform: true`, which is how it reaches the sandbox routes and not standing
    // the token carries: it gets no Staff badge, since it can do nothing else staff can.
    return { label: sandboxLabel(grants), standing: [], blocked, lines: grants.map(grantLine) };
  }
  const standing: Standing[] = [
    ...(token.platform ? (["platform"] as const) : []),
    ...(token.partner ? (["partner"] as const) : [])
  ];
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
