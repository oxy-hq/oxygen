import {
  endedSentence,
  liveLead,
  peoplePhrase,
  type TokenSummaryLine,
  tallyTokens
} from "@/pages/admin/components/TokenList/summary";
import type { Token } from "@/types/apiToken";

/** How many of the live tokens reach everything their owner can, with its leading space. */
const allAccessSentence = (live: number, allAccess: number): string => {
  if (live === 1) return allAccess === 1 ? " It is all-access." : " It is not all-access.";
  if (allAccess === 0) return " None of them is all-access.";
  if (allAccess === live)
    return live === 2 ? " Both are all-access." : " All of them are all-access.";
  return ` ${allAccess} of them ${allAccess === 1 ? "is" : "are"} all-access.`;
};

/**
 * The list in one sentence or two: how many tokens work now and for how many people, how many of
 * those reach everything their owner can, then how many no longer work. It is about the whole
 * list, whatever the filter shows.
 */
export const summarize = (
  tokens: Pick<Token, "status" | "expires_at" | "owner" | "all_access">[]
): TokenSummaryLine => {
  const { live, ended, people } = tallyTokens(tokens);

  if (live.length === 0) {
    return {
      lead: "No token with staff or partner standing works right now.",
      rest: endedSentence(ended, false)
    };
  }
  const allAccess = live.filter((token) => token.all_access).length;
  return {
    lead: liveLead(live.length),
    rest: `, held by ${peoplePhrase(people)}.${allAccessSentence(live.length, allAccess)}${endedSentence(ended, true)}`
  };
};
