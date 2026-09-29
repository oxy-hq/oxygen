// What the deploy train says in Slack, in words everyone in the channel can use.
//
// #product-releases-prod is read by product, support and leadership as much as by
// engineers: it is where people learn when a change reaches customers. The first
// versions of these messages were written for the person who built the train —
// "waiting 2266m without reaching prod. Prod gate says: staging checks are
// pending" — and told everyone else nothing they could act on, including the
// engineer who had to act. So every message here keeps four rules, and the
// release announcement in oxy-hq/infrastructure (`oxy-prod-announce.yaml`) keeps
// the same four:
//
//   1. The headline says what happened, in plain words.
//   2. The next line says what it means for customers.
//   3. "What to do" names who acts — or the message says nobody needs to.
//   4. Engineer links go last, on one "Details:" line.
//
// No jargon in the prose ("pin", "digest", "gate", "bump", "train"); durations in
// minutes, hours or days; times as Slack date tokens, so every reader sees their
// own timezone. The machine-readable markers `promote.yaml` writes on the bump PR
// (promote.ts's `*_MARKER`) are a separate contract and are not built here.
//
// The gate's reasons are mapped from `prodGate`'s `why` strings. That coupling is
// the price of keeping the decision code untouched, so the self-test drives
// `prodGate` through every branch and fails if any reason falls through to the
// generic explanation.
//
// Usage:
//   node .github/scripts/deploy-messages.ts ready|stuck --plan plan2.json [--run-url <url>]
//   node .github/scripts/deploy-messages.ts build-broken --plan plan.json
//   node .github/scripts/deploy-messages.ts --self-test
//
// INTERNAL_TOKEN (read access to oxy-hq/oxygen-internal) adds the change counts.
// Without it, or when the lookup fails, the message leaves the counts out rather
// than guessing, and says why on stderr.

import { readFile } from "node:fs/promises";
import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { prodGate, SOAK_MINUTES, type ProdState } from "./promote.ts";

const INTERNAL = "oxy-hq/oxygen-internal";
const WINDOW_TEXT = "Mon–Thu, 09:00–17:00 Vietnam time";

// ── Pure: small words ───────────────────────────────────────────────────────

/** A Slack date token: rendered in the reader's own timezone, with a UTC fallback. */
export function slackTime(iso: string): string {
  const unix = Math.floor(Date.parse(iso) / 1000);
  return `<!date^${unix}^{date_short_pretty} at {time}|${iso.slice(0, 16).replace("T", " ")} UTC>`;
}

/** "40 minutes", "38 hours", "2.5 days". */
export function humanDuration(minutes: number): string {
  if (minutes < 90) return `${Math.max(1, Math.round(minutes))} minutes`;
  if (minutes < 48 * 60) return `${Math.round(minutes / 60)} hours`;
  return `${(minutes / 1440).toFixed(1).replace(/\.0$/, "")} days`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

// ── Pure: what is in a release ──────────────────────────────────────────────

export interface Changes {
  total: number;
  features: number;
  fixes: number;
  speedups: number;
  internal: number;
}

/**
 * Counts squash subjects by their Conventional Commit type. `total` can exceed
 * the subjects given (the compare API lists at most 250); the unlisted rest are
 * counted as internal, the overwhelmingly common kind, and never as features.
 */
export function summarize(subjects: string[], total: number = subjects.length): Changes {
  const c: Changes = { total, features: 0, fixes: 0, speedups: 0, internal: 0 };
  for (const s of subjects) {
    const type = /^(\w+)(\([^)]*\))?!?:/.exec(s)?.[1]?.toLowerCase();
    if (type === "feat") c.features++;
    else if (type === "fix") c.fixes++;
    else if (type === "perf") c.speedups++;
    else c.internal++;
  }
  c.internal += Math.max(0, total - subjects.length);
  return c;
}

/** "37 changes (3 new features, 5 fixes, 29 internal)". */
export function changesText(c: Changes): string {
  const parts = [
    c.features ? plural(c.features, "new feature") : "",
    c.fixes ? plural(c.fixes, "fix", "fixes") : "",
    c.speedups ? plural(c.speedups, "speed-up") : "",
    c.internal ? `${c.internal} internal` : ""
  ].filter(Boolean);
  return `${plural(c.total, "change")}${parts.length ? ` (${parts.join(", ")})` : ""}`;
}

// ── Pure: why a release is not moving ───────────────────────────────────────

export interface Explained {
  /** Finishes "Why: …". */
  reason: string;
  /** Finishes "What to do: …". */
  action: string;
  /** False for the generic fallback — the self-test holds every gate branch to true. */
  known: boolean;
}

/**
 * Sentry's list of the unresolved issues this build brought to staging — the
 * same query `promote.yaml` counts for the gate, so the link opens exactly what
 * blocked it. The org is `SENTRY_ORG` in promote.yaml. The project is explicit:
 * without it the page falls back to the reader's last-used selection, and a
 * reader last on another project opens the link to an empty list. The gate
 * counts in project `oxy`; its numeric id comes from the issues themselves when
 * the gate kept them, else `-1` (all projects), which still contains them.
 */
export function sentryIssuesUrl(version: string | null, build: string, projectId?: string | number | null): string | null {
  if (!version) return null;
  const query = `is:unresolved first-release:oxy@${version}+${build}`;
  return `https://oxygen-intelligence.sentry.io/issues/?project=${projectId ?? -1}&environment=staging&query=${encodeURIComponent(query)}`;
}

/** The gate's reason, said for people. Order does not matter: the patterns are disjoint. */
export function explainGate(why: string, { sentryUrl = null }: { sentryUrl?: string | null } = {}): Explained {
  const known = (reason: string, action: string): Explained => ({ reason, action, known: true });
  let m: RegExpExecArray | null;
  if (/^no promotable digest/.test(why))
    return known("there is no new signed build to release.", "an engineer checks the latest image build.");
  if (/^prod serves it/.test(why))
    return known("production already runs this build.", "nothing.");
  if (/^a rollback PR is open/.test(why))
    return known(
      "a rollback of production is open and waiting for a decision.",
      "an engineer merges or closes the rollback PR, then releases by hand."
    );
  if (/release-hold/.test(why))
    return known(
      "someone put this release on hold.",
      "whoever held it removes the `release-hold` label from the release PR once the reason is resolved — the PR says why."
    );
  if (/^staging does not serve the candidate/.test(why))
    return known(
      "staging has not started running this build.",
      "an engineer checks why staging did not update (the staging deploy in ArgoCD)."
    );
  if ((m = /^staging checks are (\S+)/.exec(why))) {
    if (m[1] === "failed")
      return known("staging's release tests failed for this build.", "an engineer opens the failed test run and decides: fix forward, or skip this build.");
    return known(
      m[1] === "not-run"
        ? "staging's release tests did not reach a result (they timed out or were cut short)."
        : "staging's release tests have not reported a result for this build.",
      "nothing at first — the pipeline re-runs the tests every few hours. If this message repeats, an engineer opens the latest release-test run to see why no result arrives."
    );
  }
  if ((m = /^soaked (\d+)m of (\d+)m/.exec(why)))
    return known(`it is still being watched on staging (${m[1]} of ${m[2]} minutes).`, "nothing — it continues on its own.");
  if (/^new-Sentry-issue count was not measured/.test(why))
    return known(
      "the error check on staging could not run, so the release cannot be judged safe.",
      "an engineer restores the pipeline's Sentry access (`SENTRY_READ_TOKEN`)."
    );
  if ((m = /^(\d+) new unresolved Sentry issue/.exec(why))) {
    const one = Number(m[1]) === 1;
    const it = one ? "it" : "them";
    const where = sentryUrl ? `<${sentryUrl}|in Sentry>` : "in Sentry";
    return known(
      `${plural(Number(m[1]), "new error")} appeared on staging with this build.`,
      `an engineer triages ${it} ${where}. Resolving or ignoring ${it} lets the release continue.`
    );
  }
  if (/^outside the promote window/.test(why))
    return known(`releases go out ${WINDOW_TEXT}, and it is outside that window.`, "nothing — it can ship in the next window.");
  if (/moved on ghcr after staging was pinned/.test(why))
    return known(
      "the build was republished after staging tested it, so what would ship is not what was tested.",
      "an engineer finds out why it was republished; staging then has to test the new copy."
    );
  return { reason: `${why}.`, action: "an engineer reads the pipeline run.", known: false };
}

// ── Pure: the messages ──────────────────────────────────────────────────────

export interface ReleaseFacts {
  build: string;
  /** The version the build reports, when staging serves it and says so. */
  version: string | null;
  /** What customers run now. */
  prodVersion: string | null;
  /** Prod's build id: the only thing that tells it apart when both report one version. */
  prodBuild: string | null;
  prUrl: string | null;
  prNumber: number | null;
  compareUrl: string | null;
  runUrl: string | null;
  changes: Changes | null;
  /** The Sentry issues the gate counted, when Sentry is why it is blocked. */
  issues?: SentryIssue[] | null;
}

/** One issue as Sentry's issues API returns it; only what the messages show. */
export interface SentryIssue {
  title?: string;
  level?: string;
  count?: string | number;
  permalink?: string;
  project?: { id?: string | number };
}

/**
 * A title safe to show: one line, no characters Slack or markdown would read as
 * markup, and cut at a word with "…". A title cut mid-word was read as meaning
 * something it did not ("…fleet-wide. Low" looked like a priority). `&` is
 * escaped rather than stripped, after the cut so an entity is never split: both
 * Slack and GitHub decode entities, so an unescaped `&lt;` in an error message
 * would show as `<` — something other than the error text.
 */
export function issueTitle(raw: string | undefined, max = 90): string {
  const t = (raw || "untitled").replace(/[\r\n`<>|*_~[\]]+/g, " ").replace(/\s+/g, " ").trim();
  const cut = t.slice(0, max);
  const shown =
    t.length <= max ? t : `${cut.slice(0, cut.lastIndexOf(" ") > max / 2 ? cut.lastIndexOf(" ") : max).trimEnd()}…`;
  return shown.replace(/&/g, "&amp;");
}

/** The issues as a list: Slack bullets, or markdown for a PR comment. At most 3, then a count. */
export function issueList(issues: SentryIssue[], format: "slack" | "markdown"): string {
  const shown = issues.slice(0, 3).map((i) => {
    const events = i.count === undefined ? "" : ` · ${i.count} ${Number(i.count) === 1 ? "event" : "events"}`;
    const level = i.level ? `\`${i.level}\` ` : "";
    if (format === "markdown") return `- ${level}${issueTitle(i.title)}${events}${i.permalink ? ` · ${i.permalink}` : ""}`;
    return `• ${level}${i.permalink ? `<${i.permalink}|${issueTitle(i.title)}>` : issueTitle(i.title)}${events}`;
  });
  const more = issues.length > 3 ? `${format === "markdown" ? "- " : "• "}…and ${issues.length - 3} more` : "";
  return lines(...shown, more);
}

const blockedBySentry = (why: string) => /^\d+ new unresolved Sentry issue/.test(why);

const name = (f: ReleaseFacts) =>
  f.version ? `Oxygen ${f.version} (build \`${f.build}\`)` : `Oxygen build \`${f.build}\``;

const details = (links: [string | null, string][]) => {
  const shown = links.filter(([url]) => url).map(([url, text]) => `<${url}|${text}>`);
  return shown.length ? `Details: ${shown.join(" · ")}` : "";
};

const lines = (...ls: string[]) => ls.filter(Boolean).join("\n");

/** Every gate passed; a person merges. */
export function readyMessage(f: ReleaseFacts): string {
  return lines(
    `:large_green_circle: *${name(f)} is ready to go live*`,
    `It passed every check on staging: the release tests, ${SOAK_MINUTES} minutes running there, and no new errors. Customers get it about 5 minutes after an engineer merges the release PR.`,
    f.changes ? `*What's in it:* ${changesText(f.changes)}.` : "",
    `*What to do:* an engineer merges ${f.prUrl ? `<${f.prUrl}|release PR #${f.prNumber}>` : "the release PR"} (${WINDOW_TEXT}).`,
    details([
      [f.compareUrl, "all changes"],
      [f.runUrl, "pipeline run"]
    ])
  );
}

/** The release has not reached customers and it has been too long, or a person must act. */
export function stuckMessage(
  f: ReleaseFacts,
  { blocked, gateWhy, onStagingSince, now }: { blocked: boolean; gateWhy: string; onStagingSince: string | null; now: string }
): string {
  const e = explainGate(gateWhy, { sentryUrl: sentryIssuesUrl(f.version, f.build, f.issues?.[0]?.project?.id) });
  const waited = onStagingSince ? (Date.parse(now) - Date.parse(onStagingSince)) / 60000 : null;
  const head = blocked
    ? `:octagonal_sign: *A release needs a person before it can reach customers* — ${name(f)}`
    : `:hourglass_flowing_sand: *Changes are waiting to reach customers* — ${name(f)}${waited === null ? "" : ` has been on staging for ${humanDuration(waited)}`}`;
  // Builds between two releases report the same version, so "still on Oxygen
  // 0.5.153" next to "Oxygen 0.5.153 is waiting" reads as a contradiction. The
  // version names prod only when it differs; otherwise the build does.
  const prodName =
    f.prodVersion && f.prodVersion !== f.version
      ? `Oxygen ${f.prodVersion}`
      : f.prodBuild
        ? `build \`${f.prodBuild}\``
        : f.prodVersion
          ? `Oxygen ${f.prodVersion}`
          : "the previous release";
  const customers = `Customers are still on ${prodName}; production itself is fine.`;
  return lines(
    head,
    `${customers}${f.changes ? ` Waiting to ship: ${changesText(f.changes)}.` : ""}${onStagingSince ? ` On staging since ${slackTime(onStagingSince)}.` : ""}`,
    `*Why:* ${e.reason}`,
    blockedBySentry(gateWhy) && f.issues?.length ? issueList(f.issues, "slack") : "",
    `*What to do:* ${e.action}`,
    details([
      [f.prUrl, f.prNumber ? `release PR #${f.prNumber}` : "release PR"],
      [f.runUrl, "pipeline run"],
      [f.compareUrl, "what's waiting"]
    ])
  );
}

/** The newest image did not build, so nothing newer than the last good one can ship. */
export function buildBrokenMessage({ failed, lastGood, runUrl }: { failed: string; lastGood: string | null; runUrl: string | null }): string {
  return lines(
    `:red_circle: *The newest Oxygen build failed to package* — build \`${failed}\``,
    `${lastGood ? `Nothing newer than build \`${lastGood}\`` : "No newer build"} can be released until a build succeeds. Customers are not affected.`,
    `*What to do:* an engineer opens ${runUrl ? `<${runUrl}|the failed build>` : "the failed build in oxy-hq/oxygen's Actions"} and fixes or re-runs it.`
  );
}

// ── Reads ───────────────────────────────────────────────────────────────────

interface Plan {
  at: string;
  candidate: { sha: string; internal?: string | null } | null;
  serving: Record<string, { sha?: string | null; version?: string | null; internal?: string | null } | null>;
  bumpPr: { number: number } | null;
  soakStartedAt: string | null;
  prod: { kind: string; why: string };
  buildRun?: { sha: string; url: string } | null;
}

/** The oxygen-internal commits between prod and the candidate, or null with a reason on stderr. */
async function fetchChanges(from: string, to: string, token: string | undefined): Promise<Changes | null> {
  if (!token) {
    console.error("::warning::INTERNAL_TOKEN unset; the message leaves out the change counts");
    return null;
  }
  try {
    const res = await fetch(`https://api.github.com/repos/${INTERNAL}/compare/${from}...${to}?per_page=250`, {
      headers: { accept: "application/vnd.github+json", authorization: `Bearer ${token}` }
    });
    if (!res.ok) throw new Error(`compare ${from}...${to} → ${res.status}`);
    const body = (await res.json()) as { total_commits: number; commits: { commit: { message: string } }[] };
    return summarize(body.commits.map((c) => c.commit.message.split("\n")[0] ?? ""), body.total_commits);
  } catch (e) {
    console.error(`::warning::no change counts: ${(e as Error).message}`);
    return null;
  }
}

async function factsFrom(plan: Plan, runUrl: string | null, infraRepo: string): Promise<ReleaseFacts> {
  const build = plan.candidate?.sha ?? "unknown";
  const staging = plan.serving.staging;
  const prod = plan.serving.prod;
  const from = prod?.internal;
  const to = plan.candidate?.internal;
  return {
    build,
    version: staging?.sha === build ? (staging?.version ?? null) : null,
    prodVersion: prod?.version ?? null,
    prodBuild: prod?.sha ?? null,
    prNumber: plan.bumpPr?.number ?? null,
    prUrl: plan.bumpPr ? `https://github.com/${infraRepo}/pull/${plan.bumpPr.number}` : null,
    compareUrl: from && to ? `https://github.com/${INTERNAL}/compare/${from}...${to}` : null,
    runUrl,
    changes: from && to ? await fetchChanges(from, to, process.env.INTERNAL_TOKEN) : null
  };
}

// ── Self-test ───────────────────────────────────────────────────────────────

function selfTest(): void {
  const fails: string[] = [];
  const is = (what: string, got: unknown, want: unknown) => {
    const g = JSON.stringify(got);
    const w = JSON.stringify(want);
    if (g !== w) fails.push(`${what}\n    got  ${g}\n    want ${w}`);
  };
  const has = (what: string, text: string, needle: string, want = true) =>
    is(`${what} ${want ? "says" : "does not say"} ${JSON.stringify(needle)}`, text.includes(needle), want);

  // Every branch of the real gate is explained in words, never the fallback.
  const now = new Date("2026-09-23T03:00:00Z"); // Wed 10:00 in Ho Chi Minh
  const green: ProdState = {
    candidate: "abc1234",
    prodSha: "999aaaa",
    stagingSha: "abc1234",
    checks: "passed",
    servedSince: new Date(now.getTime() - 45 * 60000),
    held: false,
    rollbackOpen: false,
    sentryNewIssues: 0
  };
  const branches: [string, ProdState, Date?][] = [
    ["no candidate", { ...green, candidate: null }],
    ["prod current", { ...green, prodSha: "abc1234" }],
    ["rollback open", { ...green, rollbackOpen: true }],
    ["held", { ...green, held: true }],
    [
      "drift",
      { ...green, drift: "main-abc1234 moved on ghcr after staging was pinned (staging ran 111111111111, ghcr now 222222222222); not proposing" }
    ],
    ["staging behind", { ...green, stagingSha: "0000000" }],
    ["checks pending", { ...green, checks: "pending" }],
    ["checks not run", { ...green, checks: "not-run" }],
    ["checks absent", { ...green, checks: null }],
    ["soaking", { ...green, servedSince: new Date(now.getTime() - 10 * 60000) }],
    ["sentry unmeasured", { ...green, sentryNewIssues: -1 }],
    ["sentry issues", { ...green, sentryNewIssues: 3 }],
    ["outside the window", green, new Date("2026-09-25T03:00:00Z")],
    ["go", green]
  ];
  for (const [label, state, at] of branches) {
    const gate = prodGate(state, { now: at ?? now });
    if (gate.kind === "go") continue;
    is(`the gate's "${gate.why}" (${label}) is explained, not passed through`, explainGate(gate.why).known, true);
  }
  is("an unknown reason still says something", explainGate("gremlins").reason, "gremlins.");
  is("...and asks an engineer to look", explainGate("gremlins").known, false);
  has("a failed check", explainGate("staging checks are failed").reason, "failed");
  has("a Sentry count", explainGate("3 new unresolved Sentry issue(s) in this release on staging").reason, "3 new errors");
  is(
    "one new error is \"it\", and links the gate's own Sentry query",
    explainGate("1 new unresolved Sentry issue(s) in this release on staging", { sentryUrl: sentryIssuesUrl("0.5.153", "1a8c22d") }).action,
    "an engineer triages it <https://oxygen-intelligence.sentry.io/issues/?project=-1&environment=staging&query=is%3Aunresolved%20first-release%3Aoxy%400.5.153%2B1a8c22d|in Sentry>. Resolving or ignoring it lets the release continue."
  );
  is("with no version there is no link to build", sentryIssuesUrl(null, "1a8c22d"), null);
  has("the link names the issues' own project when it is known", sentryIssuesUrl("0.5.153", "1a8c22d", 4507123) ?? "", "?project=4507123&");
  is("an entity in an error message shows as written, not decoded", issueTitle("a &lt; b & c"), "a &amp;lt; b &amp; c");
  is(
    "...and a cut never splits one",
    issueTitle(`${"word ".repeat(17)}a&b`, 90).includes("&amp;") || !issueTitle(`${"word ".repeat(17)}a&b`, 90).includes("&"),
    true
  );

  // Counting a release.
  const c = summarize(
    ["feat: a", "fix(web-app): b", "fix!: c", "perf: d", "refactor: e", "chore: f", "Merge branch 'x'"],
    9
  );
  is("types are counted, and the unlisted rest are internal", c, { total: 9, features: 1, fixes: 2, speedups: 1, internal: 5 });
  is("said in words", changesText(c), "9 changes (1 new feature, 2 fixes, 1 speed-up, 5 internal)");
  is("one change is singular", changesText(summarize(["fix: a"])), "1 change (1 fix)");

  // Durations and times.
  is("minutes", humanDuration(40), "40 minutes");
  is("hours", humanDuration(2266), "38 hours");
  is("days", humanDuration(3.5 * 1440), "3.5 days");
  is("whole days drop the decimal", humanDuration(3 * 1440), "3 days");
  is(
    "a Slack date token with a UTC fallback",
    slackTime("2026-09-26T11:50:14.000Z"),
    "<!date^1790423414^{date_short_pretty} at {time}|2026-09-26 11:50 UTC>"
  );

  // The messages.
  const facts: ReleaseFacts = {
    build: "b7f49d7",
    version: "0.5.153",
    prodVersion: "0.5.152",
    prodBuild: "2ef5e38",
    prUrl: "https://github.com/oxy-hq/infrastructure/pull/2139",
    prNumber: 2139,
    compareUrl: "https://github.com/oxy-hq/oxygen-internal/compare/a...b",
    runUrl: "https://github.com/oxy-hq/oxygen-internal/actions/runs/1",
    changes: summarize(["feat: a", "fix: b", "chore: c"], 37)
  };
  const stuck = stuckMessage(facts, {
    blocked: false,
    gateWhy: "staging checks are pending",
    onStagingSince: "2026-09-26T11:50:14.000Z",
    now: "2026-09-28T01:36:00.000Z"
  });
  has("the stuck message", stuck, "*Changes are waiting to reach customers* — Oxygen 0.5.153 (build `b7f49d7`) has been on staging for 38 hours");
  has("the stuck message", stuck, "Customers are still on Oxygen 0.5.152; production itself is fine. Waiting to ship: 37 changes (1 new feature, 1 fix, 35 internal).");
  has("the stuck message", stuck, "*Why:* staging's release tests have not reported a result for this build.");
  has("the stuck message", stuck, "Details: <https://github.com/oxy-hq/infrastructure/pull/2139|release PR #2139>");
  for (const jargon of ["2266m", "gate", "bump", "digest", "train"])
    has("the stuck message", stuck.split("Details:")[0] ?? "", jargon, false);
  const blocked = stuckMessage(facts, { blocked: true, gateWhy: "the bump PR carries release-hold; a person clears it", onStagingSince: null, now: "2026-09-28T01:36:00.000Z" });
  has("a blocked release", blocked, ":octagonal_sign: *A release needs a person before it can reach customers*");
  has("a blocked release", blocked, "*Why:* someone put this release on hold.");
  const sameVersion = stuckMessage({ ...facts, version: "0.5.153", prodVersion: "0.5.153", prodBuild: "b7f49d7", build: "c3bed2e" }, { blocked: false, gateWhy: "staging checks are pending", onStagingSince: null, now: "2026-09-28T01:36:00.000Z" });
  has("when both report one version, prod is named by its build", sameVersion, "Customers are still on build `b7f49d7`; production itself is fine.");
  has("...never by a version that reads as the one waiting", sameVersion, "still on Oxygen 0.5.153", false);
  const bare = stuckMessage({ ...facts, version: null, prodVersion: null, prodBuild: null, changes: null, compareUrl: null, prUrl: null, prNumber: null, runUrl: null }, { blocked: false, gateWhy: "staging checks are pending", onStagingSince: null, now: "2026-09-28T01:36:00.000Z" });
  has("...and with nothing known it says so plainly", bare, "Customers are still on the previous release");
  has("with nothing known it still names the build", bare, "Oxygen build `b7f49d7`");
  has("...and has no empty Details line", bare, "Details:", false);
  const ready = readyMessage(facts);
  has("the ready message", ready, ":large_green_circle: *Oxygen 0.5.153 (build `b7f49d7`) is ready to go live*");
  has("the ready message", ready, `${SOAK_MINUTES} minutes running there`);
  has("the ready message", ready, "*What to do:* an engineer merges <https://github.com/oxy-hq/infrastructure/pull/2139|release PR #2139>");
  const broken = buildBrokenMessage({ failed: "abc1234", lastGood: "b7f49d7", runUrl: "https://x/run" });
  has("a broken build", broken, "Nothing newer than build `b7f49d7` can be released until a build succeeds. Customers are not affected.");

  // Naming the Sentry issues (the real ones that blocked 2026-09-28/29).
  const longTitle =
    "connection budget is oversubscribed: at this pool ceiling the server can afford only this many processes fleet-wide. Lower the pool or raise the budget";
  const cutTitle = issueTitle(longTitle);
  is("a long title is cut with an ellipsis", cutTitle.endsWith("…"), true);
  is("...at a word, so a fragment never reads as a label", /\bLow…$/.test(cutTitle), false);
  is("markup characters are flattened", issueTitle("a `b` <c>|d\ne"), "a b c d e");
  const blocking: SentryIssue[] = [
    { title: "executor failed to start task", level: "error", count: "2", permalink: "https://oxygen-intelligence.sentry.io/issues/7759589828/" }
  ];
  is(
    "a Slack line links the issue by its title",
    issueList(blocking, "slack"),
    "• `error` <https://oxygen-intelligence.sentry.io/issues/7759589828/|executor failed to start task> · 2 events"
  );
  is(
    "a markdown line for the PR comment",
    issueList(blocking, "markdown"),
    "- `error` executor failed to start task · 2 events · https://oxygen-intelligence.sentry.io/issues/7759589828/"
  );
  has("more than three are counted, not listed", issueList([...blocking, ...blocking, ...blocking, ...blocking, ...blocking], "slack"), "…and 2 more");
  const sentryBlocked = stuckMessage(
    { ...facts, build: "1a8c22d", version: "0.5.153", prodVersion: "0.5.153", prodBuild: "b7f49d7", issues: blocking },
    { blocked: true, gateWhy: "1 new unresolved Sentry issue(s) in this release on staging", onStagingSince: null, now: "2026-09-29T02:05:00.000Z" }
  );
  has("a Sentry block names the issue", sentryBlocked, "|executor failed to start task> · 2 events");
  has("...and says \"it\" for one", sentryBlocked, "an engineer triages it <https://oxygen-intelligence.sentry.io/issues/");
  has(
    "a block for another reason lists no issues",
    stuckMessage({ ...facts, issues: blocking }, { blocked: true, gateWhy: "the bump PR carries release-hold; a person clears it", onStagingSince: null, now: "2026-09-29T02:05:00.000Z" }),
    "executor failed",
    false
  );

  if (fails.length) {
    console.error(`deploy-messages.ts self-test: ${fails.length} failure(s)\n  ${fails.join("\n  ")}`);
    process.exit(1);
  }
  console.log("deploy-messages.ts self-test: every gate reason is explained and every message reads right");
}

// ── Entry ───────────────────────────────────────────────────────────────────

function arg(flag: string): string | null {
  const i = process.argv.indexOf(`--${flag}`);
  const value = i === -1 ? undefined : process.argv[i + 1];
  return value === undefined || value.startsWith("--") ? null : value;
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href;

if (!invokedDirectly) {
  // imported: exports only
} else if (process.argv.includes("--self-test")) {
  selfTest();
} else {
  const kind = process.argv[2];
  const planPath = arg("plan");
  if (!planPath || !["ready", "stuck", "build-broken", "sentry-list"].includes(kind ?? "")) {
    console.error(
      "usage: deploy-messages.ts ready|stuck|build-broken|sentry-list --plan <plan.json> [--sentry <issues.json>] [--run-url <url>] | --self-test"
    );
    process.exit(1);
  }
  const plan = JSON.parse(await readFile(planPath, "utf8")) as Plan;
  const runUrl = arg("run-url");
  // The issues the gate counted (promote.yaml's Sentry step). Absent or not a
  // list means the message says the count only, as before.
  const sentryPath = arg("sentry");
  const issues = sentryPath
    ? await readFile(sentryPath, "utf8")
        .then((s) => JSON.parse(s) as unknown)
        .then((j) => (Array.isArray(j) ? (j as SentryIssue[]) : null))
        .catch(() => null)
    : null;
  if (kind === "sentry-list") {
    // For the PR comment: the list, only when Sentry is the reason.
    if (issues?.length && blockedBySentry(plan.prod.why)) console.log(issueList(issues, "markdown"));
  } else if (kind === "build-broken") {
    console.log(buildBrokenMessage({ failed: plan.buildRun?.sha ?? "unknown", lastGood: plan.candidate?.sha ?? null, runUrl: plan.buildRun?.url ?? runUrl }));
  } else {
    const facts = { ...(await factsFrom(plan, runUrl, process.env.INFRA_REPO || "oxy-hq/infrastructure")), issues };
    console.log(
      kind === "ready"
        ? readyMessage(facts)
        : stuckMessage(facts, { blocked: plan.prod.kind === "blocked", gateWhy: plan.prod.why, onStagingSince: plan.soakStartedAt, now: plan.at })
    );
  }
}
