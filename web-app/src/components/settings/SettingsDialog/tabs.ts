/**
 * The tabs of the settings sections that have them, in the order they show. A tab is addressable
 * by a link as a third segment of the section id (`?settings=organization.api_tokens.personal`),
 * so these ids are part of a URL people hold: add to them, never rename one.
 */

/** Organization → API tokens. */
export const API_TOKENS_TABS = ["personal", "service_accounts", "all", "policy"] as const;
export type ApiTokensTab = (typeof API_TOKENS_TABS)[number];

/** Organization → Secrets. */
export const SECRETS_TABS = ["shared", "app"] as const;
export type SecretsTab = (typeof SECRETS_TABS)[number];
