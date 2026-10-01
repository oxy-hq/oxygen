import { apiClient } from "./axios";

/** postMessage type the OAuth success page sends back to the opener (popup
 *  mode). The opener must validate `event.origin === window.location.origin`
 *  before trusting it. */
export const QB_CONNECT_MESSAGE = "quickbooks-connected";

/** sessionStorage key the wizard uses to preserve its form across a
 *  full-page OAuth redirect (mobile / popup-blocked mode). */
export const QB_WIZARD_STASH_KEY = "qb-pipeline-wizard-stash";

export interface QuickBooksAuthorizeBody {
  client_id: string;
  /** Plaintext secret to store under `client_secret_var`; omit to reuse an
   *  existing secret of that name. */
  client_secret?: string;
  client_secret_var: string;
  refresh_token_var: string;
  mode: "popup" | "redirect";
  /** FE path to return to in redirect mode. */
  return_path?: string;
}

/** Authenticated XHR → Intuit consent URL. The caller hands the browser off
 *  to it (popup or full-page redirect). */
export async function fetchQuickBooksAuthorizeUrl(
  projectId: string,
  body: QuickBooksAuthorizeBody
): Promise<string> {
  const res = await apiClient.post<{ url: string }>(
    `/${projectId}/integrations/quickbooks/authorize`,
    body
  );
  return res.data.url;
}
