// Which PRs a release shipped that could be pictured, in the order they are
// tried: features, then fixes, oldest first within each.
//
// The announcement (oxy-hq/infrastructure's oxy-prod-announce.yaml) reads the
// same squash subjects, lists the first five of each and counts the rest as
// "+N more". A picture is not held to that cut: its reply carries its own
// headline and PR link, and the one feature with a screen to show is as likely
// to be the eighth as the first. The release's spend cap bounds the list.

import { commitType } from "./detect";

export interface ShippedPr {
  pr: number;
  type: "feat" | "fix";
  /** The bullet text, without the conventional prefix or the `(#N)` suffix. */
  text: string;
}

export function shippedPrs(subjects: string[]): ShippedPr[] {
  const parsed: ShippedPr[] = [];
  for (const subject of subjects) {
    const type = commitType(subject);
    const pr = /\(#(\d+)\)\s*$/.exec(subject)?.[1];
    if ((type !== "feat" && type !== "fix") || !pr) continue;
    const text = subject.replace(/^[A-Za-z]+(\([^)]*\))?!?:\s*/, "").replace(/\s*\(#\d+\)\s*$/, "");
    parsed.push({ pr: Number(pr), type, text: text.charAt(0).toUpperCase() + text.slice(1) });
  }
  // Features first: they are what a reader came for, and what the cap should buy.
  return [...parsed.filter((p) => p.type === "feat"), ...parsed.filter((p) => p.type === "fix")];
}

/** Slack mrkdwn escaping, as the announcement applies it. */
function slackEscape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function threadComment(repo: string, item: ShippedPr, headline: string): string {
  const url = `https://github.com/${repo}/pull/${item.pr}`;
  return `*${slackEscape(headline)}*\n${slackEscape(item.text)} · <${url}|#${item.pr}>`;
}
