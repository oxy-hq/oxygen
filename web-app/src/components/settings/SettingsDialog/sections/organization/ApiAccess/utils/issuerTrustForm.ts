import type {
  ClaimCondition,
  CreateIssuerTrustPolicyRequest,
  CreateOidcIssuerRequest,
  IssuerKeySource,
  IssuerTrustPolicy,
  OidcIssuer,
  ServiceAccountRole,
  UpdateIssuerTrustPolicyRequest
} from "@/types/orgApiAccess";
import {
  type AccessDraft,
  accessDraftError,
  buildGrantInputs,
  draftFromGrants,
  ORG_WIDE_ACCESS
} from "./grants";
import { serviceAccountHandle } from "./slug";

/**
 * The form's presets. Each fills in an issuer and a policy's conditions; the
 * server knows none of them and stores only what they filled in, so a new
 * kind of workload is a new entry here, not a server change.
 */
export type IssuerPreset = "aws" | "custom";

export const PRESETS: { value: IssuerPreset; label: string; description: string }[] = [
  {
    value: "aws",
    label: "AWS (ECS, Lambda, EC2)",
    description:
      "An AWS account's outbound identity federation issuer. Any IAM role in the account can ask STS for a signed token; a policy names which role may act."
  },
  {
    value: "custom",
    label: "Custom OIDC",
    description:
      "Any issuer of signed OIDC tokens. You name the issuer, its keys, and the claims a token must have."
  }
];

/** The preset an issuer was registered with, as the screen treats it. */
export function presetOf(issuer: Pick<OidcIssuer, "preset">): IssuerPreset {
  return issuer.preset === "aws" ? "aws" : "custom";
}

// ── Registering an issuer ───────────────────────────────────────────────────

export interface IssuerFormState {
  preset: IssuerPreset;
  name: string;
  issuer: string;
  keySource: IssuerKeySource;
  discoveryUrl: string;
  jwksUri: string;
  /** The pasted key set, as typed. */
  jwksText: string;
  jtiSingleUse: boolean;
}

/**
 * Both presets start at keys through discovery and single-use tokens. AWS
 * keeps them: STS publishes discovery under the issuer, and a workload asks
 * it for a fresh token for each exchange.
 */
export function emptyIssuerForm(preset: IssuerPreset): IssuerFormState {
  return {
    preset,
    name: "",
    issuer: "",
    keySource: "discovery",
    discoveryUrl: "",
    jwksUri: "",
    jwksText: "",
    jtiSingleUse: true
  };
}

export interface IssuerFormErrors {
  name?: string;
  issuer?: string;
  discoveryUrl?: string;
  jwksUri?: string;
  jwks?: string;
}

function parseUrl(raw: string): URL | null {
  try {
    return new URL(raw);
  } catch {
    return null;
  }
}

const isPlainHttps = (url: URL | null): boolean =>
  url !== null && url.protocol === "https:" && !url.search && !url.hash && !url.username;

/** The pasted key set, parsed, or why not. */
export function parseJwks(text: string): { jwks?: unknown; error?: string } {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return { error: "Paste the key set as JSON: an object with a 'keys' list." };
  }
  const keys = (value as { keys?: unknown })?.keys;
  if (!Array.isArray(keys) || keys.length === 0) {
    return { error: "The key set needs a non-empty 'keys' list." };
  }
  if (keys.some((key) => key && typeof key === "object" && "d" in key)) {
    return { error: "That holds a private key. Paste the public keys only." };
  }
  return { jwks: value };
}

/** The same shape the server checks; it also reads the keys before saving, unless they are inline. */
export function validateIssuerForm(form: IssuerFormState): IssuerFormErrors {
  const errors: IssuerFormErrors = {};
  const name = form.name.trim();
  if (!name) errors.name = "Name it, as your team calls it.";
  else if (name.length > 64) errors.name = "At most 64 characters.";
  const issuer = form.issuer.trim();
  if (!issuer) {
    errors.issuer = "Paste the issuer URL, exactly as its tokens carry it in 'iss'.";
  } else if (!isPlainHttps(parseUrl(issuer))) {
    errors.issuer = "An https URL with no query or fragment.";
  }
  if (form.keySource === "discovery" && form.discoveryUrl.trim()) {
    if (!isPlainHttps(parseUrl(form.discoveryUrl.trim()))) {
      errors.discoveryUrl = "An https URL with no query or fragment, or leave it empty.";
    }
  }
  if (form.keySource === "jwks_uri" && !isPlainHttps(parseUrl(form.jwksUri.trim()))) {
    errors.jwksUri = "The https URL the issuer's key set is published at.";
  }
  if (form.keySource === "inline") {
    const parsed = parseJwks(form.jwksText);
    if (parsed.error) errors.jwks = parsed.error;
  }
  return errors;
}

export function buildCreateIssuerRequest(form: IssuerFormState): CreateOidcIssuerRequest {
  const request: CreateOidcIssuerRequest = {
    name: form.name.trim(),
    issuer: form.issuer.trim(),
    key_source: form.keySource,
    jti_single_use: form.jtiSingleUse,
    preset: form.preset
  };
  if (form.keySource === "discovery" && form.discoveryUrl.trim()) {
    request.discovery_url = form.discoveryUrl.trim();
  }
  if (form.keySource === "jwks_uri") request.jwks_uri = form.jwksUri.trim();
  if (form.keySource === "inline") request.jwks = parseJwks(form.jwksText).jwks;
  return request;
}

/** Where an issuer's keys come from, in words. */
export function describeKeySource(issuer: OidcIssuer): string {
  switch (issuer.key_source) {
    case "discovery":
      return `Keys from discovery: ${issuer.jwks_uri ?? ""}`;
    case "jwks_uri":
      return `Keys from ${issuer.jwks_uri ?? ""}`;
    case "inline":
      return `Keys pasted inline (${issuer.inline_key_ids.join(", ") || "no kid"}). Never fetched.`;
  }
}

// ── Claim conditions ────────────────────────────────────────────────────────

/** Claims no condition may name, as the server refuses them. */
export const NOT_CONDITIONS = ["iss", "exp", "iat", "nbf", "jti"];
export const MAX_CONDITIONS = 16;

/** An exact value, or a prefix ending in one `*`. */
export function conditionValueError(raw: string): string | null {
  const value = raw.trim();
  if (!value) return "Give the value the claim must have.";
  if (value.length > 512) return "At most 512 characters.";
  if (value === "*") return "A bare '*' matches anything. Give a prefix before it.";
  const stars = value.split("*").length - 1;
  if (stars > 1 || (stars === 1 && !value.endsWith("*"))) {
    return "One '*' at most, and only as the last character.";
  }
  return null;
}

export function claimError(raw: string): string | null {
  const claim = raw.trim();
  if (!claim) return "Name a top-level claim of the token.";
  if (claim.length > 128 || /\s/.test(claim)) return "A claim name, with no spaces.";
  if (claim === "iss") return "'iss' is the issuer's, checked already. Not a condition.";
  if (NOT_CONDITIONS.includes(claim)) return `'${claim}' differs on every token.`;
  return null;
}

/** The first thing wrong with a set of conditions, as the server would say it. */
export function conditionsError(conditions: ClaimCondition[]): string | null {
  if (conditions.length > MAX_CONDITIONS) return `At most ${MAX_CONDITIONS} conditions.`;
  const seen = new Set<string>();
  for (const condition of conditions) {
    const error = claimError(condition.claim) ?? conditionValueError(condition.value);
    if (error) return `${condition.claim.trim() || "A condition"}: ${error}`;
    const claim = condition.claim.trim();
    if (seen.has(claim)) return `'${claim}' is named twice. Give each claim one condition.`;
    seen.add(claim);
  }
  if (!conditions.some((c) => c.claim.trim() !== "aud")) {
    return "Add a condition on a claim other than 'aud'. Every token for Oxygen carries its audience.";
  }
  return null;
}

/** `claim = value` pairs, in a line. */
export function describeConditions(conditions: ClaimCondition[]): string {
  return conditions.map((c) => `${c.claim} = ${c.value}`).join(", ");
}

/** An IAM role's ARN, as STS writes it into `sub`: any partition, a 12-digit account, a role path. */
const ROLE_ARN = /^arn:aws[a-z-]*:iam::\d{12}:role\/[\w+=,.@/-]+$/;

export function roleArnError(raw: string): string | null {
  const arn = raw.trim();
  if (!arn) return "Paste the ARN of the IAM role the workload runs as.";
  if (!ROLE_ARN.test(arn)) {
    return "An IAM role ARN: arn:aws:iam::<12-digit account>:role/<name>.";
  }
  return null;
}

/**
 * The IAM role a policy names, when it is exactly what the AWS preset writes:
 * one exact `sub` condition holding a role ARN. Else `null`.
 */
export function awsRoleOf(conditions: ClaimCondition[]): string | null {
  if (conditions.length !== 1 || conditions[0].claim !== "sub") return null;
  return ROLE_ARN.test(conditions[0].value) ? conditions[0].value : null;
}

// ── A trust policy ──────────────────────────────────────────────────────────

/** One condition row of the form: a condition, and a key that stays with the row. */
export interface ConditionRow extends ClaimCondition {
  key: string;
}

let rowsMade = 0;

/** A form row for `condition`. */
export function conditionRow(condition: ClaimCondition = { claim: "", value: "" }): ConditionRow {
  rowsMade += 1;
  return { ...condition, key: `condition-${rowsMade}` };
}

/** A trust policy as its dialog holds it. */
export interface TrustPolicyFormState {
  preset: IssuerPreset;
  issuerId: string;
  /** The Oxygen service account it acts as. */
  accountId: string;
  /** AWS preset: the IAM role's ARN. */
  roleArn: string;
  /** Custom preset. */
  conditions: ConditionRow[];
  access: AccessDraft;
}

/** Starts at the whole organization: the one reach that names no workspace. */
export function emptyTrustPolicyForm(
  preset: IssuerPreset,
  issuerId: string,
  accountId = ""
): TrustPolicyFormState {
  return {
    preset,
    issuerId,
    accountId,
    roleArn: "",
    conditions: [conditionRow({ claim: "sub", value: "" })],
    access: ORG_WIDE_ACCESS
  };
}

export function formFromTrustPolicy(policy: IssuerTrustPolicy): TrustPolicyFormState {
  const role = awsRoleOf(policy.conditions);
  return {
    preset: role ? "aws" : "custom",
    issuerId: policy.issuer.id,
    accountId: policy.service_account.id,
    roleArn: role ?? "",
    conditions: policy.conditions.map((c) => conditionRow(c)),
    access: draftFromGrants(policy.grants)
  };
}

/** What the preset fills in: the conditions the server stores. */
export function conditionsOfForm(form: TrustPolicyFormState): ClaimCondition[] {
  if (form.preset === "aws") return [{ claim: "sub", value: form.roleArn.trim() }];
  return form.conditions
    .map((c) => ({ claim: c.claim.trim(), value: c.value.trim() }))
    .filter((c) => c.claim || c.value);
}

export interface TrustPolicyFormErrors {
  issuerId?: string;
  accountId?: string;
  roleArn?: string;
  conditions?: string;
  access?: string;
}

/** An edit changes only the grants: what a policy names is fixed. */
export function validateTrustPolicyForm(
  form: TrustPolicyFormState,
  { mode }: { mode: "create" | "edit" }
): TrustPolicyFormErrors {
  const errors: TrustPolicyFormErrors = {};
  if (mode === "create") {
    if (!form.issuerId) errors.issuerId = "Pick the issuer.";
    if (!form.accountId) errors.accountId = "Pick the service account it acts as.";
    if (form.preset === "aws") {
      const role = roleArnError(form.roleArn);
      if (role) errors.roleArn = role;
    } else {
      const conditions = conditionsError(conditionsOfForm(form));
      if (conditions) errors.conditions = conditions;
    }
  }
  // Org-wide, or apps only: a policy of this kind never names one workspace.
  const apps = form.access.scope === "selected" ? { ...form.access, workspaces: [] } : form.access;
  if (accessDraftError(apps, true))
    errors.access = "Pick the whole organization or at least one app.";
  return errors;
}

/** The whole organization at the account's role, and the picked apps. Never a workspace. */
function grantsOf(form: TrustPolicyFormState, accountRole: ServiceAccountRole) {
  const access = { ...form.access, workspaces: [] };
  return buildGrantInputs(access, accountRole, { explicitOrgWide: true }) ?? [];
}

export function buildCreateTrustPolicyRequest(
  form: TrustPolicyFormState,
  accountRole: ServiceAccountRole
): CreateIssuerTrustPolicyRequest {
  return {
    issuer_id: form.issuerId,
    conditions: conditionsOfForm(form),
    // A policy always states its grants; there is no server default to lean on.
    grants: grantsOf(form, accountRole)
  };
}

export function buildUpdateTrustPolicyRequest(
  form: TrustPolicyFormState,
  accountRole: ServiceAccountRole
): UpdateIssuerTrustPolicyRequest {
  return { grants: grantsOf(form, accountRole) };
}

// ── The workload's side, for the AWS preset ─────────────────────────────────

export interface ClientSnippetInput {
  /** What the workload names the account BY: its id, never `<org>/<name>`. */
  accountId: string;
  orgSlug: string;
  accountName: string;
  /**
   * The deployment's own address, `https://app.oxygen-hq.com` — `authOrigin()`, never an org
   * subdomain the browser is on: the exchange accepts only the audience of that address.
   */
  origin: string;
}

/** `oxy:<host>`: the audience the deployment at `origin` requires. */
export function audienceFor(origin: string): string {
  return `oxy:${new URL(origin).host}`;
}

/**
 * What a workload on AWS runs to act as the account: ask STS for a token for
 * this deployment's audience (RS256, the one Oxygen verifies; a regional
 * endpoint, the only kind that serves it), then trade it for a 15-minute
 * Oxygen token — directly, or by letting oxyc read it from a file.
 */
export function buildClientSnippet(input: ClientSnippetInput): string {
  const handle = serviceAccountHandle(input.orgSlug, input.accountName);
  const audience = audienceFor(input.origin);
  return [
    "# The workload's IAM role needs sts:GetWebIdentityToken for this audience.",
    "TOKEN=$(aws sts get-web-identity-token \\",
    '  --region "$AWS_REGION" --endpoint-url "https://sts.$AWS_REGION.amazonaws.com" \\',
    `  --audience '${audience}' --signing-algorithm RS256 --duration-seconds 300 \\`,
    "  --query WebIdentityToken --output text)",
    "",
    `# Trade it for a 15-minute Oxygen token acting as ${handle}.`,
    `curl -sS ${input.origin}/api/auth/oidc/exchange \\`,
    "  -H 'content-type: application/json' \\",
    `  -d "{\\"token\\": \\"$TOKEN\\", \\"service_account\\": \\"${input.accountId}\\"}"`,
    "",
    "# Or let oxyc make the exchange, reading the token from a file.",
    'printf %s "$TOKEN" > /tmp/oxy-token',
    `OXY_SERVICE_ACCOUNT=${input.accountId} OXY_OIDC_TOKEN_FILE=/tmp/oxy-token \\`,
    `  oxyc api /api/... --target ${input.origin}`
  ].join("\n");
}
