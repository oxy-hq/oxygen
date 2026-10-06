/**
 * A service account's name is a slug: it is the second half of the handle
 * (`<org_slug>/<name>`) a workflow passes to `oxyc`, so it has to survive a
 * YAML file and a shell without quoting.
 *
 * The server is the authority on the exact rule. This mirrors the rule the
 * rest of the product uses for slugs (apps, org subdomains) so the common
 * mistakes are caught before a round trip; anything it lets through that the
 * server refuses still surfaces as the server's own message.
 */
export const SERVICE_ACCOUNT_NAME_MIN = 2;
export const SERVICE_ACCOUNT_NAME_MAX = 40;

const SLUG = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/;

/**
 * What typing produces: lowercase, with spaces and underscores turned into
 * hyphens and everything else dropped. A trailing hyphen is kept, because the
 * person is usually about to type the next word.
 */
export function toSlugInput(raw: string): string {
  return raw
    .toLowerCase()
    .replace(/[\s_]+/g, "-")
    .replace(/[^a-z0-9-]/g, "")
    .replace(/-{2,}/g, "-")
    .slice(0, SERVICE_ACCOUNT_NAME_MAX);
}

/** Why `name` can't be a service account name, or `null` when it can. */
export function serviceAccountNameError(name: string, taken: string[] = []): string | null {
  if (name.length === 0) return "Give the account a name.";
  if (name.length < SERVICE_ACCOUNT_NAME_MIN) {
    return `Use at least ${SERVICE_ACCOUNT_NAME_MIN} characters.`;
  }
  if (name.length > SERVICE_ACCOUNT_NAME_MAX) {
    return `Use at most ${SERVICE_ACCOUNT_NAME_MAX} characters.`;
  }
  if (!/^[a-z]/.test(name)) return "Start with a lowercase letter.";
  if (name.endsWith("-")) return "End with a letter or a digit, not a hyphen.";
  if (!SLUG.test(name)) return "Use lowercase letters, digits and single hyphens only.";
  if (taken.some((other) => other.toLowerCase() === name)) {
    return "A service account with that name already exists here.";
  }
  return null;
}

/**
 * `acme/deployer`: how a person reads the account, and what a CI log shows it
 * signed in as. NOT how a workflow names it — that is the account's id, which
 * cannot be taken over when an org's slug changes hands.
 */
export function serviceAccountHandle(orgSlug: string, name: string): string {
  return `${orgSlug}/${name}`;
}
