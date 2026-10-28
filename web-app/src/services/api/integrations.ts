import { apiClient } from "./axios";

interface LookerExplore {
  model: string;
  name: string;
  description: string | null;
}

export interface LookerIntegrationInfo {
  name: string;
  explores: LookerExplore[];
}

export class IntegrationService {
  static async listLookerIntegrations(
    projectId: string,
    branchName: string
  ): Promise<LookerIntegrationInfo[]> {
    const response = await apiClient.get(`/${projectId}/integrations/looker`, {
      params: { branch: branchName }
    });
    return response.data;
  }
}

/**
 * OAuth providers connectable from Settings → Integrations.
 *
 * Surfaced in **Settings → Connections**.
 *
 * Hand-maintained mirror of the `slug`s in
 * `crates/app/src/integrations/oauth_provider.rs`, with no drift guard in
 * either direction — a slug typo'd here 404s at authorize rather than failing a
 * build. QuickBooks is deliberately absent: it has its own connect flow inside
 * the Airway pipeline wizard, where the realm id the callback returns is
 * actually used.
 */
export interface OauthProvider {
  slug: string;
  label: string;
  /** What the user is consenting to, in their words rather than the scope string. */
  grants: string;
  /** Default secret names the tokens land under. */
  clientSecretVar: string;
  refreshTokenVar: string;
}

export const OAUTH_PROVIDERS: OauthProvider[] = [
  {
    slug: "google-drive",
    label: "Google Drive",
    grants:
      "Read and write only files this app creates in your Drive. It cannot see anything already there.",
    clientSecretVar: "GOOGLE_CLIENT_SECRET",
    refreshTokenVar: "GOOGLE_REFRESH_TOKEN"
  }
];

export interface OauthAuthorizeBody {
  client_id: string;
  /** Plaintext secret stored under `client_secret_var`; omit to reuse a stored one. */
  client_secret?: string;
  client_secret_var: string;
  refresh_token_var: string;
  mode: "popup" | "redirect";
  return_path?: string;
}

/** Authenticated XHR → the provider's consent URL, which the caller opens. */
export async function fetchOauthAuthorizeUrl(
  projectId: string,
  providerSlug: string,
  body: OauthAuthorizeBody
): Promise<string> {
  const res = await apiClient.post<{ url: string }>(
    `/${projectId}/integrations/oauth/${providerSlug}/authorize`,
    body
  );
  return res.data.url;
}
