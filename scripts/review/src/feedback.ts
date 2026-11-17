// What people said about findings: replies in a finding's inline thread, and PR comments
// that name its id. Bots are first-class here: an agent's or another bot's reply or
// dispute counts exactly like a person's. Only the reviewer's own comments are left out,
// so it never grades itself.
//
// Nobody is asked to rate a finding. What it led to is the measure: fixed or obsolete
// before merge (addressed), withdrawn when a reply showed it was wrong, or still open at
// merge. A reply is the one thing a person writes, and it does work: the recheck pass
// reads it and withdraws the finding when the reason holds.

import type { Finding, GhUser, IssueComment, ReviewComment } from "./types.ts";

const MENTION = /\bF(\d{1,3})\b/g;
const OWN_MARKER = "<!-- oxy-review:";

type Said = Pick<IssueComment, "user" | "body">;

const idsIn = (text: string): string[] => [...text.matchAll(MENTION)].map((m) => `F${m[1] ?? ""}`);

export function isBot(user: GhUser | null, botLogins: readonly string[]): boolean {
  if (!user) return true;
  if (user.type === "Bot" || /\[bot\]$/.test(user.login)) return true;
  return botLogins.includes(user.login.replace(/\[bot\]$/, ""));
}

/** A comment the reviewer itself posted: its identity, carrying its marker. */
export function isOwn(comment: Said, reviewerLogins: readonly string[]): boolean {
  const login = (comment.user?.login ?? "").replace(/\[bot\]$/, "");
  return reviewerLogins.includes(login) && (comment.body ?? "").includes(OWN_MARKER);
}

/** Who said it, for a prompt: `name` or `name (bot)`. */
export const speaker = (c: Said): string =>
  `${c.user?.login ?? "unknown"}${c.user?.type === "Bot" ? " (bot)" : ""}`;

/**
 * What anyone but the reviewer said about each finding, for the recheck pass to weigh
 * and the metrics to count: replies in its inline thread, and PR comments that name its id.
 */
export function threadReplies({
  reviewComments,
  issueComments = [],
  findings,
  reviewerLogins
}: {
  reviewComments: readonly ReviewComment[];
  issueComments?: readonly IssueComment[];
  findings: readonly Pick<Finding, "id" | "inline">[];
  reviewerLogins: readonly string[];
}): Map<string, string[]> {
  const byInline = new Map(findings.filter((f) => f.inline).map((f) => [f.inline, f.id]));
  const ids = new Set(findings.map((f) => f.id));
  const out = new Map<string, string[]>();
  const add = (id: string, c: Said) => {
    const list = out.get(id) ?? [];
    list.push(`${speaker(c)}: ${(c.body ?? "").slice(0, 1200)}`);
    out.set(id, list);
  };
  for (const c of reviewComments) {
    const id = byInline.get(c.in_reply_to_id ?? null);
    if (id && !isOwn(c, reviewerLogins)) add(id, c);
  }
  for (const c of issueComments) {
    if (isOwn(c, reviewerLogins)) continue;
    for (const id of new Set(idsIn(c.body ?? ""))) if (ids.has(id)) add(id, c);
  }
  return out;
}

/** The PR's conversation, newest last, for the reviewer to read as context. */
export function discussion(
  issueComments: readonly IssueComment[],
  reviewerLogins: readonly string[],
  { max = 8, chars = 600 }: { max?: number; chars?: number } = {}
): string[] {
  return issueComments
    .filter((c) => !isOwn(c, reviewerLogins))
    .slice(-max)
    .map(
      (c) =>
        `${speaker(c)}: ${(c.body ?? "")
          .replace(/<!--[\s\S]*?-->/g, "")
          .trim()
          .slice(0, chars)}`
    )
    .filter((line) => !line.endsWith(": "));
}
