// Which PRs a release announcement lists, in its order. Mirrors the RENDER
// filter in oxy-hq/infrastructure's oxy-prod-announce.yaml: squash subjects,
// oldest first, grouped New (feat) then Fixed (fix), the first five of each.
// CROSS-REPO CONTRACT — change the two together.

import { commitType } from "./detect";

export interface AnnouncedPr {
  pr: number;
  type: "feat" | "fix";
  /** The bullet text, without the conventional prefix or the `(#N)` suffix. */
  text: string;
}

export const PER_SECTION = 5;

export function announcedPrs(subjects: string[]): AnnouncedPr[] {
  const parsed: AnnouncedPr[] = [];
  for (const subject of subjects) {
    const type = commitType(subject);
    const pr = /\(#(\d+)\)\s*$/.exec(subject)?.[1];
    if ((type !== "feat" && type !== "fix") || !pr) continue;
    const text = subject.replace(/^[A-Za-z]+(\([^)]*\))?!?:\s*/, "").replace(/\s*\(#\d+\)\s*$/, "");
    parsed.push({ pr: Number(pr), type, text: text.charAt(0).toUpperCase() + text.slice(1) });
  }
  const feats = parsed.filter((p) => p.type === "feat").slice(0, PER_SECTION);
  const fixes = parsed.filter((p) => p.type === "fix").slice(0, PER_SECTION);
  return [...feats, ...fixes];
}

/** Slack mrkdwn escaping, as the announcement applies it. */
export function slackEscape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function threadComment(repo: string, item: AnnouncedPr, headline: string): string {
  const url = `https://github.com/${repo}/pull/${item.pr}`;
  return `*${slackEscape(headline)}*\n${slackEscape(item.text)} · <${url}|#${item.pr}>`;
}
