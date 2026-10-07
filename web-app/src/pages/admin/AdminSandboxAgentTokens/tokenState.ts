import {
  endedSentence,
  liveLead,
  peoplePhrase,
  type TokenSummaryLine,
  tallyTokens
} from "@/pages/admin/components/TokenList/summary";
import type { Token } from "@/types/apiToken";

// A row's state is read the same way on every staff token list, so it lives with the list's
// shared pieces. This page's own part is the sentence below.
export { endedAt, tokenState } from "@/pages/admin/components/TokenList/tokenState";

/**
 * The list in one sentence: how many tokens work now and for how many people, then how many no
 * longer do. The lead answers "is an agent running".
 */
export const summarize = (
  tokens: Pick<Token, "status" | "expires_at" | "owner">[]
): TokenSummaryLine => {
  const { live, ended, people } = tallyTokens(tokens);

  if (live.length === 0) {
    return { lead: "No agent holds a token right now.", rest: endedSentence(ended, false) };
  }
  return {
    lead: liveLead(live.length),
    rest: `, minted by ${peoplePhrase(people)}.${endedSentence(ended, true)}`
  };
};
