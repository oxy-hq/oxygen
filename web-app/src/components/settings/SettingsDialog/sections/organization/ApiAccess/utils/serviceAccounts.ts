import type { ServiceAccount } from "@/types/orgApiAccess";

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/**
 * What stops working with an account, as a noun phrase a confirmation can
 * drop into a sentence: "its 3 tokens and 1 trusted-access policy". `null`
 * when it holds neither, so the dialog can say that plainly instead.
 */
export function casualties(
  account: Pick<ServiceAccount, "token_count" | "trust_policy_count">
): string | null {
  const parts: string[] = [];
  if (account.token_count > 0) parts.push(count(account.token_count, "token", "tokens"));
  if (account.trust_policy_count > 0) {
    parts.push(
      count(account.trust_policy_count, "trusted-access policy", "trusted-access policies")
    );
  }
  return parts.length > 0 ? `its ${parts.join(" and ")}` : null;
}

/** "3 tokens", "1 token", or a dash-free "None" for a table cell. */
export const countLabel = (n: number, one: string, many: string): string =>
  n === 0 ? "None" : count(n, one, many);
