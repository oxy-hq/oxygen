import { toSlugInput } from "@/components/settings/SettingsDialog/sections/organization/ApiAccess/utils/slug";
import { ServiceAccountService } from "@/services/api/orgApiAccess";
import type { ExpiryInput } from "@/types/apiToken";
import type { ServiceAccount, ServiceAccountRole } from "@/types/orgApiAccess";

/** Which service account the new token signs in as. */
export type AccountChoice =
  | { kind: "new"; name: string; role: ServiceAccountRole }
  | { kind: "existing"; account: ServiceAccount };

export interface MintRequest {
  orgId: string;
  account: AccountChoice;
  /** The token's name, as API tokens → Service accounts lists it. */
  tokenName: string;
  expiry: ExpiryInput;
}

/**
 * A token created for a secret and not yet saved into it. The secret value lives only here and in
 * the form that saves it: it is never put on screen.
 */
export interface MintedApiToken {
  value: string;
  orgId: string;
  tokenId: string;
  expiresAt: string | null;
  account: {
    id: string;
    name: string;
    role: ServiceAccountRole;
    /** Made for this token, so taking the token back takes the account back too. */
    created: boolean;
  };
}

/** The server's cap on a token name (`crates/auth/src/token/access.rs` `NAME_MAX_CHARS`). */
const TOKEN_NAME_MAX = 100;

/**
 * What the token is called on the service account's page: the secret it was made for, so whoever
 * finds it there later knows where it lives and what revoking it breaks.
 */
export function tokenNameFor(secretName: string, appName?: string): string {
  const name = appName ? `Secret ${secretName} of ${appName}` : `Secret ${secretName}`;
  return name.slice(0, TOKEN_NAME_MAX);
}

/**
 * The account name to suggest: the app's own slug for an app's secret (one account per app reads
 * well in the list, and is what the Store Ops fix used), else the secret's name as a slug.
 */
export function suggestedAccountName(secretName: string, appSlug?: string): string {
  const base = toSlugInput(appSlug || secretName).replace(/^[^a-z]+/, "");
  return base.replace(/-+$/, "") || "app-token";
}

/**
 * Create the account if asked to, then mint a token on it. The token gets the account's
 * org-wide reach at the account's role: a secret's reader is an app or a job acting for the
 * whole org, and narrowing it further is done on the account's page.
 *
 * If minting fails after the account was created here, the account is deleted again, so a
 * failed attempt leaves nothing behind in the org's list.
 */
export async function mintApiToken(request: MintRequest): Promise<MintedApiToken> {
  const { orgId } = request;
  const created = request.account.kind === "new";
  const account =
    request.account.kind === "new"
      ? await ServiceAccountService.create(orgId, {
          name: request.account.name,
          org_role: request.account.role
        })
      : request.account.account;
  try {
    const { token, secret } = await ServiceAccountService.createToken(orgId, account.id, {
      name: request.tokenName,
      ...request.expiry
    });
    return {
      value: secret,
      orgId,
      tokenId: token.id,
      expiresAt: token.expires_at,
      account: { id: account.id, name: account.name, role: account.org_role, created }
    };
  } catch (error) {
    if (created) await ServiceAccountService.remove(orgId, account.id).catch(() => undefined);
    throw error;
  }
}

/**
 * Take back a token that was never saved: revoke it, and delete its account when the account was
 * made for it. A minted token that no secret holds would otherwise sit live on the account until
 * it expired.
 */
export async function discardMinted(minted: MintedApiToken): Promise<void> {
  await ServiceAccountService.revokeToken(minted.orgId, minted.account.id, minted.tokenId);
  if (minted.account.created) {
    await ServiceAccountService.remove(minted.orgId, minted.account.id);
  }
}
