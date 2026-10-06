import type { TokenPolicy } from "@/types/orgApiAccess";

/** What the server answers for an org that has never saved a policy. */
export const DEFAULT_TOKEN_POLICY: TokenPolicy = {
  max_lifetime_days: null,
  allow_all_access_tokens: true,
  require_environment_on_trust_policies: true
};

/** The longest lifetime the form accepts. Ten years; past that, "no limit" says it better. */
export const MAX_LIFETIME_LIMIT = 3650;

/**
 * The policy as the form holds it. The lifetime is a switch plus a text field
 * so that turning the limit off doesn't throw away the number someone typed.
 */
export interface PolicyFormState {
  limitLifetime: boolean;
  maxLifetimeDays: string;
  allowAllAccessTokens: boolean;
  requireEnvironment: boolean;
}

export function policyToForm(policy: TokenPolicy): PolicyFormState {
  return {
    limitLifetime: policy.max_lifetime_days !== null,
    maxLifetimeDays: policy.max_lifetime_days === null ? "90" : String(policy.max_lifetime_days),
    allowAllAccessTokens: policy.allow_all_access_tokens,
    requireEnvironment: policy.require_environment_on_trust_policies
  };
}

/** Why the lifetime field can't be saved, or `null` when it can (or doesn't apply). */
export function maxLifetimeError(form: PolicyFormState): string | null {
  if (!form.limitLifetime) return null;
  const raw = form.maxLifetimeDays.trim();
  if (raw === "") return "Enter a number of days.";
  if (!/^\d+$/.test(raw)) return "Use a whole number of days.";
  const days = Number(raw);
  if (days < 1) return "Use at least 1 day.";
  if (days > MAX_LIFETIME_LIMIT) {
    return `Use at most ${MAX_LIFETIME_LIMIT} days, or turn the limit off.`;
  }
  return null;
}

/** The body to `PUT`, or `null` while the form is invalid. */
export function formToPolicy(form: PolicyFormState): TokenPolicy | null {
  if (maxLifetimeError(form)) return null;
  return {
    max_lifetime_days: form.limitLifetime ? Number(form.maxLifetimeDays.trim()) : null,
    allow_all_access_tokens: form.allowAllAccessTokens,
    require_environment_on_trust_policies: form.requireEnvironment
  };
}

const samePolicy = (a: TokenPolicy, b: TokenPolicy) =>
  a.max_lifetime_days === b.max_lifetime_days &&
  a.allow_all_access_tokens === b.allow_all_access_tokens &&
  a.require_environment_on_trust_policies === b.require_environment_on_trust_policies;

/**
 * Whether saving would change anything. An invalid form counts as dirty: the
 * person has typed something, and the Save button's job is then to say why it
 * can't be saved, not to look as if nothing happened.
 */
export function isPolicyDirty(form: PolicyFormState, saved: TokenPolicy): boolean {
  const next = formToPolicy(form);
  return next === null || !samePolicy(next, saved);
}

/**
 * What saving will do to tokens and policies that already exist, in plain
 * words. Only tightening has consequences worth a sentence; loosening just
 * unblocks things.
 */
export function policyChangeNotes(form: PolicyFormState, saved: TokenPolicy): string[] {
  const next = formToPolicy(form);
  if (!next) return [];
  const notes: string[] = [];

  const tighterLifetime =
    next.max_lifetime_days !== null &&
    (saved.max_lifetime_days === null || next.max_lifetime_days < saved.max_lifetime_days);
  if (tighterLifetime) {
    notes.push(
      `Tokens that expire more than ${next.max_lifetime_days} days out, or never, stop working in this organization until their owner shortens them. They are not revoked.`
    );
  }
  if (saved.allow_all_access_tokens && !next.allow_all_access_tokens) {
    notes.push(
      "All-access personal tokens stop working in this organization. Their owners can keep using them elsewhere, or add a grant for this organization."
    );
  }
  if (saved.require_environment_on_trust_policies && !next.require_environment_on_trust_policies) {
    notes.push(
      "Trusted-access policies can then be saved without an environment. For such a policy, anyone who can push to the repository could get a token."
    );
  }
  return notes;
}
