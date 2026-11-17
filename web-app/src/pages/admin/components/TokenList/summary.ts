/** The Tokens page in one sentence, as `TokenListSummary` sets it. */
export interface TokenSummaryLine {
  /** What works right now: set in the foreground. */
  lead: string;
  /** Whose the live ones are, and how many have ended. May be empty. */
  rest: string;
}

/** "1 token is live", "4 tokens are live". */
export const liveLead = (live: number): string =>
  live === 1 ? "1 token is live" : `${live} tokens are live`;

/**
 * The sentence about the ended tokens, with its leading space. After the live ones it counts
 * "more"; with nothing live, the ended ones are all there is below. Empty when none has ended.
 */
export const endedSentence = (ended: number, anyLive: boolean): string => {
  if (ended === 0) return "";
  if (anyLive) return ` ${ended} more ${ended === 1 ? "has" : "have"} expired or been revoked.`;
  return ended === 1
    ? " The one below has expired or been revoked."
    : ` The ${ended} below have expired or been revoked.`;
};
