import { isAxiosError } from "axios";

/** Why an org's token policy makes a token inert there (`Token.blocked_orgs[].reason`). */
const BLOCK_REASONS: Record<string, string> = {
  max_lifetime: "Its expiry is further out than this organization allows",
  all_access_disallowed: "This organization doesn't accept all-access tokens"
};

/** One sentence for a policy block, in the interface's words; an unknown reason reads plainly. */
export const policyBlockReason = (reason: string): string =>
  BLOCK_REASONS[reason] ?? "This organization's token policy doesn't allow it";

/**
 * The 400 `exceeds_policy` body names the tightest lifetime cap among the token's orgs. Returns
 * the sentence to show, or `undefined` for any other error. Regenerate keeps the expiry, so it is
 * refused only for a token already past the cap, and the way out is a shorter expiry first.
 */
export function exceedsPolicyMessage(
  err: unknown,
  action: "set_expiry" | "regenerate" = "set_expiry"
): string | undefined {
  if (!isAxiosError(err)) return undefined;
  const data: unknown = err.response?.data;
  if (!data || typeof data !== "object") return undefined;
  const body = data as { code?: unknown; max_lifetime_days?: unknown };
  if (body.code !== "exceeds_policy") return undefined;
  const days = body.max_lifetime_days;
  const span =
    typeof days === "number" ? (days === 1 ? "1 day" : `${days} days`) : "a shorter time";
  const cap = `An organization this token reaches allows tokens to last at most ${span}.`;
  return action === "regenerate"
    ? `${cap} This token's expiry is past that. Extend it to an earlier date, then regenerate.`
    : `${cap} Pick an earlier expiry.`;
}
