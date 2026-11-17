// The weekly report, in Markdown: figures first, one line per item.

import type { Escape } from "./escapes.ts";
import type { Aggregate, Bucket, PassTotals } from "./metrics.ts";

const pct = (v: number | null): string => (v === null ? "n/a" : `${v}%`);
const secs = (ms: number | null): string => (ms === null ? "n/a" : `${Math.round(ms / 1000)}s`);
const prs = (list: readonly number[]): string =>
  list.length ? list.map((n) => `#${n}`).join(", ") : "none";

function bucketTable(title: string, buckets: Record<string, Bucket>): string {
  const rows = Object.entries(buckets)
    .sort((a, b) => b[1].n - a[1].n)
    .map(
      ([k, b]) =>
        `| ${k} | ${b.n} | ${pct(Math.round((100 * b.addressed) / b.n))} | ${b.withdrawn} | ${b.open} | ${b.replied} |`
    );
  if (!rows.length) return "";
  return [
    `#### ${title}`,
    "",
    "| | findings | addressed | withdrawn | open at merge | replied to |",
    "|---|---|---|---|---|---|",
    ...rows,
    ""
  ].join("\n");
}

/** The passes that are not lenses: every other pass is named after the lens that ran. */
const CORE_PASSES = new Set(["general", "recheck", "verify"]);

const share = (part: number, total: number): string =>
  total > 0 ? `${Math.round((100 * part) / total)}%` : "n/a";

/** Thousands of tokens per metered run, or a dash when no run of this pass was metered. */
const perRun = (tokens: number, t: PassTotals): string =>
  t.metered ? `${Math.round(tokens / t.metered / 1000)}K` : "–";

const totalCost = (byPass: Aggregate["byPass"]): number =>
  Object.values(byPass).reduce((n, t) => n + t.cost, 0);

/** Where the money goes, by pass: the table the cost levers are chosen from. */
export function passTable(byPass: Aggregate["byPass"]): string {
  const total = totalCost(byPass);
  const rows = Object.entries(byPass)
    .sort((a, b) => b[1].cost - a[1].cost)
    .map(
      ([name, t]) =>
        `| ${name} | ${t.runs} | $${t.cost.toFixed(2)} | ${share(t.cost, total)} | $${(t.cost / t.runs).toFixed(2)} | ${perRun(t.cacheWrite, t)} | ${perRun(t.cacheRead, t)} | ${perRun(t.input, t)} | ${perRun(t.output, t)} |`
    );
  if (!rows.length) return "No pass recorded its cost in this window.";
  return [
    "#### Cost by pass",
    "",
    "Tokens are per run, over the runs that recorded them. A cache write costs 1.25x input and a read a twentieth of it, so a pass whose first call writes the prefix another pass already wrote is paying twice for it.",
    "",
    "| | runs | cost | share | per run | cache written | cache read | fresh input | output |",
    "|---|---|---|---|---|---|---|---|---|",
    ...rows,
    ""
  ].join("\n");
}

/** One headline line: the general pass, all lenses together, verify and recheck. */
export function passSplit(byPass: Aggregate["byPass"]): string | null {
  const total = totalCost(byPass);
  if (total <= 0) return null;
  const cost = (name: string) => byPass[name]?.cost ?? 0;
  const lenses = Object.entries(byPass)
    .filter(([name]) => !CORE_PASSES.has(name))
    .reduce((n, [, t]) => n + t.cost, 0);
  const parts: [string, number][] = [
    ["general", cost("general")],
    ["lenses", lenses],
    ["verify", cost("verify")],
    ["recheck", cost("recheck")]
  ];
  return `Spend by pass: ${parts.map(([n, c]) => `${n} ${share(c, total)}`).join(" · ")}.`;
}

/** Whether a one-click suggestion gets acted on more than a fix in words. */
function suggestionLine({ with: w, without: wo }: Aggregate["suggestions"]): string {
  if (!w.n) return "One-click suggestions: none posted.";
  const rate = (b: Bucket) => pct(b.n ? Math.round((100 * b.addressed) / b.n) : null);
  return `One-click suggestions: ${w.n} posted, ${rate(w)} addressed; should-fix and blocking findings without one: ${rate(wo)} of ${wo.n}.`;
}

function escapesSection(escapes: readonly Escape[]): string {
  const broad = escapes.filter((e) => e.broad).map((e) => e.fix);
  const lines: string[] = [];
  for (const e of escapes) {
    if (e.broad) continue;
    const verdict = e.unreviewed
      ? "its PR had no review"
      : e.legacy
        ? "reviewed by the previous reviewer (not checked)"
        : e.flagged
          ? `flagged as ${e.finding ?? "?"} (${e.status ?? "?"})`
          : "**not flagged**";
    const files = e.files.map((f) => `\`${f}\``).join(", ");
    lines.push(`- #${e.fix} "${e.fixTitle}" fixes lines #${e.intro} wrote (${files}): ${verdict}`);
  }
  if (!lines.length && !broad.length)
    return "No `fix:` PR in the window changed lines written by an earlier PR.";
  if (broad.length)
    lines.push(`- Too broad to attribute (over 300 old lines changed): ${prs(broad)}`);
  return lines.join("\n");
}

/** Fix PRs whose likeliest source PR was reviewed and the review did not flag the spot. */
export function escapedCount(escapes: readonly Escape[]): number {
  return new Set(
    escapes.filter((e) => !e.broad && e.flagged === false && !e.unreviewed).map((e) => e.fix)
  ).size;
}

/** The figures, one line each: the report's opening and the whole Slack post. */
export function headline({
  agg,
  escapes
}: {
  agg: Aggregate;
  escapes: readonly Escape[];
}): string[] {
  const f = agg.findings;
  const c = agg.coverage;
  const legacy = new Set(escapes.filter((e) => !e.broad && e.legacy).map((e) => e.fix)).size;
  const split = passSplit(agg.byPass);
  return [
    `${agg.prs} PRs merged. ${c.reviewedHead} had a finished review of the merged commit; ${c.mergedUnreviewed.length} did not.`,
    `${f.n} findings from the review pipeline: ${pct(f.addressedPct)} addressed before merge, ${pct(f.withdrawnPct)} withdrawn as wrong, ${pct(f.openPct)} still open at merge.`,
    `Cost $${agg.cost.total.toFixed(2)} (about $${agg.cost.perPr.toFixed(2)} per PR). Review time p50 ${secs(agg.cost.p50ms)}, p90 ${secs(agg.cost.p90ms)}.`,
    ...(split ? [split] : []),
    `Possible escaped bugs: ${escapedCount(escapes)} fix PR(s) changed lines whose original PR the pipeline reviewed without flagging them; ${legacy} more trace to PRs the previous reviewer saw.`
  ];
}

export function renderReport({
  agg,
  escapes,
  window
}: {
  agg: Aggregate;
  escapes: readonly Escape[];
  window: { from: string; to: string };
}): string {
  const c = agg.coverage;
  const head = [
    `## Code review report: ${window.from} to ${window.to}`,
    "",
    ...headline({ agg, escapes }).map((l) => `- ${l}`),
    ""
  ];
  const highRisk = agg.people.highRisk.length
    ? agg.people.highRisk
        .map(
          (h) =>
            `#${h.number} (${h.areas.join(", ")}${h.approved ? ", approved" : ", **no approval**"})`
        )
        .join(", ")
    : "none";
  const body = [
    "### Coverage",
    `- Current pipeline: ${c.v2} · previous reviewer: ${c.legacy} · no review: ${c.none}`,
    `- Merged without a finished review of the merged commit: ${prs(c.mergedUnreviewed)}`,
    `- Review did not finish: ${prs(c.incomplete)}`,
    `- Overridden with \`review-override\`: ${prs(agg.people.overrides)}`,
    "",
    "### Where findings land",
    bucketTable("By category", agg.byCategory),
    bucketTable("By severity", agg.bySeverity),
    bucketTable("By pass", agg.byLens),
    suggestionLine(agg.suggestions),
    `Verifier: rejected ${agg.verifier.rejected} of ${agg.verifier.candidates} candidates before posting.`,
    "",
    "### Cost",
    passTable(agg.byPass),
    "### Escaped bugs",
    escapesSection(escapes),
    "",
    "### Rules flagged on more than one PR",
    agg.recurring.length
      ? agg.recurring
          .map(
            (r) =>
              `- ${r.rule}: ${prs(r.prs)}. A lint rule or architecture test could catch this instead.`
          )
          .join("\n")
      : "None.",
    "",
    "### People and size",
    `- PRs with an approval from someone other than the author: ${agg.people.approved} of ${agg.prs}`,
    `- High-risk PRs: ${highRisk}`,
    `- Median PR size ${agg.size.median ?? "n/a"} changed lines; ${agg.size.over1000} over 1,000`
  ];
  return [...head, ...body].join("\n");
}
