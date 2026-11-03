// The deploy train's four DORA numbers: how often prod moves, how long a merged
// change waits to get there, how often a move is undone, and how long the undoing
// takes. Run weekly by `deploy-metrics.yaml`; readable by hand.
//
// promote.ts decides what prod SHOULD run; this reads what prod DID run. Same
// shape: every decision is a pure function exported and pinned by `--self-test`,
// and the network lives at the bottom.
//
// The one record that covers every path a deploy has ever taken is the history of
// `imageTag` in the prod values file on oxy-hq/infrastructure's main. Three facts
// about that history decide how a deploy is recognised, and each of them breaks a
// simpler rule:
//
//   * **Most prod deploys had no PR.** Until the reconciler, the usual path was a
//     direct push to infra main (`build: update image tag to 0.5.149`), so
//     "merged PRs titled oxy-prod" counts a minority of them. Every commit on main
//     that moves the tag is a deploy; its time is the PR's `merged_at` when there
//     was a PR, and the commit's committer date when there was not.
//   * **A title is not evidence of a pin.** `deploy(oxy-prod): oxy 0.5.136 →
//     0.5.141` pinned 0.5.142, and an Airhouse retention PR (#1808) moved oxy from
//     0.5.110 to 0.5.112 without mentioning it. Pins are read from the DIFF. The
//     title is consulted for one thing: whether it calls itself a rollback.
//   * **Two pin shapes, one commit space.** Historical pins are releases
//     (`0.5.150`, a bare tag in oxygen-internal); the reconciler's are
//     `main-<sha>`, a MIRROR sha whose copybara `GitOrigin-RevId` trailer names
//     the internal commit. Both resolve to oxygen-internal commits, so a single
//     compare answers "which way did it move" and "what did it ship".
//
// Usage:
//   GH_TOKEN=… [INFRA_TOKEN=…] node .github/scripts/deploy-metrics.ts [--days 7]
//     [--now <ISO>] [--json] [--format slack|markdown] [--summary-file <path>]
//   node .github/scripts/deploy-metrics.ts --self-test
//
// GH_TOKEN reads oxy-hq/oxygen (public) and oxy-hq/oxygen-internal. INFRA_TOKEN
// reads oxy-hq/infrastructure and defaults to GH_TOKEN, so a personal
// `gh auth token` covers all three. It writes nothing anywhere except the
// optional `--summary-file`, which it appends to.

import { readFileSync } from "node:fs";
import { appendFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { humanDuration } from "./deploy-messages.ts";
import { originRevId, proposedSha } from "./promote.ts";

// ── Shapes ──────────────────────────────────────────────────────────────────

type Token = string;

interface Tokens {
  /** oxy-hq/oxygen and oxy-hq/oxygen-internal. */
  gh: Token;
  /** oxy-hq/infrastructure. */
  infra: Token;
}

/** An `imageTag` value, as far as it says anything about which code it runs. */
type Pin =
  | { kind: "main"; sha: string; raw: string }
  | { kind: "semver"; version: string; raw: string }
  | { kind: "other"; raw: string };

/** Which way a change moved prod along oxygen-internal's history. */
type Direction = "forward" | "backward" | "same" | "unknown";

type Kind = "deploy" | "rollback";

/** An internal commit a deploy newly put in front of customers. */
interface Shipped {
  sha: string;
  /** Committer date of the commit on oxygen-internal main — its merge, for a squash merge. */
  committedAt: string;
}

/** What a commit on infra main did to the prod pin, before any resolution. */
interface Move {
  from: Pin;
  to: Pin;
  rollbackTitle: boolean;
  /** What the title claims it pins. Only reported; the diff is what counts. */
  titleTarget: Pin | null;
}

interface RawChange extends Move {
  /** The infra commit on main. */
  sha: string;
  /** The PR it landed through, or null for a direct push. */
  pr: number | null;
  title: string;
  /** ISO. The PR's merged_at, or a direct push's committer date. */
  at: string;
}

interface ProdChange extends RawChange {
  direction: Direction;
  kind: Kind;
  internal: { from: string | null; to: string | null };
  /** Null when not read: a rollback, a change before the window, or a pin that did not resolve. */
  shipped: Shipped[] | null;
  /** What the compare said the range holds; above `shipped.length` means truncated. */
  totalCommits: number | null;
}

interface Window {
  start: Date;
  end: Date;
}

interface WindowMetrics {
  start: string;
  end: string;
  days: number;
  deploys: number;
  rollbacks: number;
  viaPr: number;
  viaPush: number;
  leadTime: {
    medianMs: number | null;
    p90Ms: number | null;
    commits: number;
    truncatedDeploys: number;
    unresolvedDeploys: number;
    why: string | null;
  };
  changeFailureRate: {
    rate: number | null;
    failed: { sha: string; pr: number | null; pin: string; at: string }[];
    why: string | null;
  };
  timeToRestore: {
    medianMs: number | null;
    restores: { rollback: string; at: string; undid: string | null; ms: number | null }[];
    why: string | null;
  };
  staging: {
    /** Moves of the staging pin in the window: how often staging takes new code. */
    pins: number;
    /** Per release: its pin's first arrival on staging → the prod change. */
    medianMs: number | null;
    maxMs: number | null;
    measured: number;
    /** Releases whose pin was not on staging in the STAGE_LOOKBACK_DAYS before. */
    skipped: string[];
    why: string | null;
  };
  releaseChecks: {
    checked: number;
    passed: number;
    failed: number;
    held: number;
    why: string | null;
  };
  infraChanges: {
    /** Commits that changed production's infrastructure without moving the image. */
    total: number;
    /** Of those, the ones their author typed `fix` or `revert`. */
    fixes: { sha: string; at: string; title: string }[];
    why: string | null;
  };
}

/** When a pin first reached the staging values file. */
interface StagingPin {
  pin: string;
  at: string;
}

/** A verdict label put on the prod bump PR (release-checks-gate.yaml on infra). */
interface GateEvent {
  pr: number;
  at: string;
  verdict: "passed" | "failed" | "hold";
}

/** A commit on infra main under one of the customer-facing prod product directories. */
interface InfraCommit {
  sha: string;
  at: string;
  title: string;
}

/**
 * What infra main records beside the prod pin. Each is null when it could not be
 * read, with the reason in `why`, so a failed read is "not measured", never 0.
 */
interface Signals {
  staging: StagingPin[] | null;
  gate: GateEvent[] | null;
  infra: InfraCommit[] | null;
  why: { staging?: string; gate?: string; infra?: string };
}

interface Report {
  generatedAt: string;
  basis: string;
  window: WindowMetrics;
  /** The trailing 30 days, for context; null when the window already is that. */
  context: WindowMetrics | null;
  changes: ProdChange[];
}

const INFRA = "oxy-hq/infrastructure";
const MIRROR = "oxy-hq/oxygen";
const INTERNAL = "oxy-hq/oxygen-internal";
const API = "https://api.github.com";

export const PROD_VALUES =
  "oxy-workload/664267706513/us-west-2/oxy-prod/products/oxy/gitops/values/oxy.yaml";

/** Staging runs in the dev account; promote.ts pins it, and prod later takes the same pin. */
export const STAGING_VALUES =
  "oxy-workload/575455576647/us-west-2/oxy-dev/products/oxy/gitops/values/oxy-staging.yaml";

/**
 * What customers use in prod: the oxy app, Airhouse and Pokehouse (ClickHouse).
 * Monitoring, ferret, logging and the account's terraform sit beside them and
 * are left out: an alert tuned or an internal tool fixed is not a repair to
 * what customers run.
 */
export const PROD_PRODUCT_DIRS = ["oxy", "airhouse", "pokehouse"].map(
  (p) => `oxy-workload/664267706513/us-west-2/oxy-prod/products/${p}`
);

/**
 * The branch public-image-bump.yaml force-pushes each release candidate to; one
 * PR on it carries many candidates, each judged by a fresh verdict label.
 */
const BUMP_BRANCH = "chore/bump-oxy-prod-image";

/**
 * How long before a release its pin is looked for on staging. Past it the
 * release counts as having skipped staging. Bump PRs have sat open for six days.
 */
const STAGE_LOOKBACK_DAYS = 14;

/** Commit reads in flight at once; a 44-day staging history is ~350 of them. */
const READ_CONCURRENCY = 8;

const DAY_MS = 86_400_000;

/** The context line's length. */
const CONTEXT_DAYS = 30;

/**
 * How much history before the earliest window is read. A rollback early in the
 * window needs the deploy it undid, which may be older than the window; this is
 * how far back that is looked for.
 */
const LOOKBACK_DAYS = 30;

/**
 * Compare pages (100 commits each) read per deploy. Past it the deploy's commit
 * list is truncated and the summary says so — the prod pin sat still for weeks at
 * a time in July, and one of those catch-up deploys is thousands of commits.
 */
const COMPARE_PAGES = 5;

/** Every flag the CLI reads. The self-test holds the workflow to this list. */
const FLAGS = ["days", "now", "json", "format", "summary-file", "self-test"];

const BASIS =
  "lead time = committer date of each newly shipped commit on oxygen-internal main → " +
  "the infra change's merged_at (committer date for a direct push); deploys and rollbacks " +
  "are commits on infra main that move imageTag in the prod values file; time on staging = " +
  "the release's pin first reaching the staging values file → the prod change; release checks = " +
  "verdict labels on the prod bump PR; infrastructure fixes = commits under the prod oxy, airhouse and pokehouse directories " +
  "that are not a release (by sha or PR number) and are typed fix or revert";

// ── Pure: recognising a deploy ──────────────────────────────────────────────

/**
 * An `imageTag` value parsed into what it pins.
 *
 * A digest suffix (`main-abc1234@sha256:…`) pins the same code as the bare tag,
 * so it is dropped: two spellings of one commit are not a move.
 */
export function parsePin(raw: string): Pin {
  const value = raw
    .replace(/\s+#.*$/, "")
    .trim()
    .replace(/^["']|["']$/g, "");
  const main = /^main-([0-9a-f]{7,40})(?:@\S+)?$/.exec(value);
  if (main?.[1]) return { kind: "main", sha: main[1], raw: value };
  const semver = /^v?(\d+\.\d+\.\d+)$/.exec(value);
  if (semver?.[1]) return { kind: "semver", version: semver[1], raw: value };
  return { kind: "other", raw: value };
}

/** A pin as a person reads it, and as two pins are compared. */
export function pinLabel(pin: Pin): string {
  if (pin.kind === "main") return `main-${pin.sha.slice(0, 7)}`;
  if (pin.kind === "semver") return pin.version;
  return pin.raw;
}

/** The value of the values file's `imageTag:` line, or null. */
export function imageTagOf(yaml: string): string | null {
  return /^\s*imageTag:\s*(.+)$/m.exec(yaml)?.[1] ?? null;
}

/** The pin a unified-diff patch moves `imageTag` from and to, or null if it does not. */
export function imageTagMove(patch: string | null | undefined): { from: Pin; to: Pin } | null {
  const line = (sign: "-" | "+") =>
    new RegExp(`^\\${sign}\\s*imageTag:\\s*(.+)$`, "m").exec(patch ?? "")?.[1];
  const before = line("-");
  const after = line("+");
  if (before === undefined || after === undefined) return null;
  const from = parsePin(before);
  const to = parsePin(after);
  return pinLabel(from) === pinLabel(to) ? null : { from, to };
}

/** Whether a title calls the change a rollback. Title-only; the pin order is the other half. */
export function saysRollback(title: string | null | undefined): boolean {
  return /\broll(?:ed|ing|s)?[\s-]*back\b|\brevert/i.test(title ?? "");
}

/**
 * What a title claims it pins, from its `→ <ref>` suffix.
 *
 * `proposedSha` (promote.ts) reads the reconciler's `main-` form; a squash
 * commit's ` (#123)` suffix is removed first, since it is what makes the arrow
 * stop being the end of the line. A digest-pinned ref comes back from it with the
 * digest still attached (`abc1234@sha256:…`), which `parsePin` drops. The
 * hand-written release titles put more after the version (`→ 0.5.141, matching
 * staging`), so a release is read wherever the arrow is.
 */
export function titleTarget(title: string | null | undefined): Pin | null {
  const bare = (title ?? "").replace(/\s+\(#\d+\)$/, "");
  const sha = proposedSha(bare);
  if (sha) return parsePin(`main-${sha}`);
  const release = /→\s*(v?\d+\.\d+\.\d+)\b/.exec(bare)?.[1];
  return release ? parsePin(release) : null;
}

/**
 * Whether a commit moved prod, and between which pins.
 *
 * Null for a commit that does not touch the prod values file, and for one that
 * touches it without moving `imageTag` (env, sizing, a revert of either).
 */
export function prodMove(
  commit: { title: string; files: { filename: string; patch?: string | null }[] },
  prodPath: string = PROD_VALUES
): Move | null {
  const file = commit.files.find((f) => f.filename === prodPath);
  const move = file ? imageTagMove(file.patch) : null;
  if (!move) return null;
  return {
    ...move,
    rollbackTitle: saysRollback(commit.title),
    titleTarget: titleTarget(commit.title)
  };
}

/** Release order, numerically: 0.5.10 is after 0.5.9. */
export function semverOrder(from: string, to: string): Direction {
  const a = from.split(".").map(Number);
  const b = to.split(".").map(Number);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const d = (b[i] ?? 0) - (a[i] ?? 0);
    if (d !== 0) return d > 0 ? "forward" : "backward";
  }
  return "same";
}

/**
 * Which way a change moved. Two releases order themselves; anything else is the
 * compare API's `from...to` status, and a status that is not a plain "ahead" or
 * "behind" (diverged, or no compare at all) is not guessed at.
 */
export function directionOf(from: Pin, to: Pin, compareStatus: string | null): Direction {
  if (from.kind === "semver" && to.kind === "semver") return semverOrder(from.version, to.version);
  if (compareStatus === "ahead") return "forward";
  if (compareStatus === "behind") return "backward";
  if (compareStatus === "identical") return "same";
  return "unknown";
}

/** A rollback says so, or moves prod to older code. Everything else that moves the pin is a deploy. */
export function kindOf(rollbackTitle: boolean, direction: Direction): Kind {
  return rollbackTitle || direction === "backward" ? "rollback" : "deploy";
}

/**
 * One change per PR. A rebase or merge-commit merge lands a PR as several commits
 * on main, each of which moved the pin a step; what the PR did is first `from` to
 * last `to`. Direct pushes are each their own change. A PR that moved the pin and
 * moved it back did nothing, and is dropped.
 */
export function collapseByPr(changes: RawChange[]): RawChange[] {
  const out: RawChange[] = [];
  const slot = new Map<number, number>();
  for (const change of changes) {
    const i = change.pr === null ? undefined : slot.get(change.pr);
    const prev = i === undefined ? undefined : out[i];
    if (i === undefined || prev === undefined) {
      if (change.pr !== null) slot.set(change.pr, out.length);
      out.push(change);
      continue;
    }
    out[i] = {
      ...change,
      from: prev.from,
      rollbackTitle: prev.rollbackTitle || change.rollbackTitle
    };
  }
  return out.filter((c) => pinLabel(c.from) !== pinLabel(c.to));
}

// ── Pure: the metrics ───────────────────────────────────────────────────────

const within = (change: { at: string }, w: Window): boolean => {
  const t = Date.parse(change.at);
  return t >= w.start.getTime() && t < w.end.getTime();
};

/** Linear-interpolated percentile (`p` in 0..1), the numpy default. Null for no samples. */
export function percentile(samples: number[], p: number): number | null {
  if (!samples.length) return null;
  const sorted = [...samples].sort((a, b) => a - b);
  const rank = (sorted.length - 1) * p;
  const lo = sorted[Math.floor(rank)] ?? 0;
  const hi = sorted[Math.ceil(rank)] ?? lo;
  return lo + (hi - lo) * (rank - Math.floor(rank));
}

/**
 * Merge-to-prod time for every commit a deploy in the window newly shipped.
 *
 * A commit shipped twice — out, rolled back, out again — is counted once, at its
 * first arrival: the question is how long a change waited, not how often it moved.
 */
export function leadTimes(changes: ProdChange[], w: Window): WindowMetrics["leadTime"] {
  const first = new Map<string, number>();
  let truncatedDeploys = 0;
  let unresolvedDeploys = 0;
  let deploys = 0;
  for (const change of changes) {
    if (change.kind !== "deploy" || !within(change, w)) continue;
    deploys++;
    if (!change.shipped) {
      unresolvedDeploys++;
      continue;
    }
    if ((change.totalCommits ?? 0) > change.shipped.length) truncatedDeploys++;
    for (const commit of change.shipped) {
      if (first.has(commit.sha)) continue;
      first.set(commit.sha, Date.parse(change.at) - Date.parse(commit.committedAt));
    }
  }
  const samples = [...first.values()];
  const why = samples.length
    ? null
    : !deploys
      ? "no deploys in the window"
      : unresolvedDeploys
        ? "no deploy's pins resolved to oxygen-internal commits"
        : "the deploys shipped no new commits";
  return {
    medianMs: percentile(samples, 0.5),
    p90Ms: percentile(samples, 0.9),
    commits: samples.length,
    truncatedDeploys,
    unresolvedDeploys,
    why
  };
}

/**
 * The deploys whose next prod change rolled them back. `changes` is the whole
 * history read, oldest first, so a deploy late in one window rolled back early in
 * the next is still failed.
 */
export function failedDeploys(changes: ProdChange[]): ProdChange[] {
  return changes.filter((change, i) => {
    const next = changes[i + 1];
    return (
      change.kind === "deploy" &&
      next?.kind === "rollback" &&
      pinLabel(next.from) === pinLabel(change.to)
    );
  });
}

/** The deploy a rollback undid: the latest earlier deploy that put its `from` pin on prod. */
export function undoneBy(changes: ProdChange[], rollbackIndex: number): ProdChange | null {
  const rollback = changes[rollbackIndex];
  if (!rollback) return null;
  for (let i = rollbackIndex - 1; i >= 0; i--) {
    const earlier = changes[i];
    if (earlier?.kind === "deploy" && pinLabel(earlier.to) === pinLabel(rollback.from))
      return earlier;
  }
  return null;
}

/** Rollbacks in the window ÷ deploys in the window, with the deploys that were rolled back. */
export function changeFailureRate(
  changes: ProdChange[],
  w: Window
): WindowMetrics["changeFailureRate"] {
  const deploys = changes.filter((c) => c.kind === "deploy" && within(c, w)).length;
  const rollbacks = changes.filter((c) => c.kind === "rollback" && within(c, w)).length;
  const failed = failedDeploys(changes)
    .filter((c) => within(c, w))
    .map((c) => ({ sha: c.sha, pr: c.pr, pin: pinLabel(c.to), at: c.at }));
  return {
    rate: deploys ? rollbacks / deploys : null,
    failed,
    why: deploys ? null : "no deploys in the window to fail"
  };
}

/** For each rollback in the window: how long the deploy it undid was on prod. */
export function timeToRestore(changes: ProdChange[], w: Window): WindowMetrics["timeToRestore"] {
  const restores = changes.flatMap((change, i) => {
    if (change.kind !== "rollback" || !within(change, w)) return [];
    const undid = undoneBy(changes, i);
    return [
      {
        rollback: `${pinLabel(change.from)} → ${pinLabel(change.to)}`,
        at: change.at,
        undid: undid ? pinLabel(undid.to) : null,
        ms: undid ? Date.parse(change.at) - Date.parse(undid.at) : null
      }
    ];
  });
  const samples = restores.flatMap((r) => (r.ms === null ? [] : [r.ms]));
  const why = samples.length
    ? null
    : restores.length
      ? "no deploy found before the rollback(s) that put the rolled-back pin on prod"
      : "no rollbacks in the window";
  return { medianMs: percentile(samples, 0.5), restores, why };
}

/**
 * How long each release in the window sat on staging first: its pin's earliest
 * arrival on staging in the STAGE_LOOKBACK_DAYS before → the prod change. A
 * release whose pin was never there skipped staging, and is named. Rollbacks go
 * to code that already ran, so they are not asked.
 */
export function stagingDwell(
  changes: ProdChange[],
  staging: StagingPin[] | null,
  w: Window,
  why?: string
): WindowMetrics["staging"] {
  if (!staging)
    return {
      pins: 0,
      medianMs: null,
      maxMs: null,
      measured: 0,
      skipped: [],
      why: why ?? "not read"
    };
  const samples: number[] = [];
  const skipped: string[] = [];
  for (const change of changes) {
    if (change.kind !== "deploy" || !within(change, w)) continue;
    const at = Date.parse(change.at);
    const pin = pinLabel(change.to);
    const arrivals = staging
      .filter((s) => s.pin === pin)
      .map((s) => Date.parse(s.at))
      .filter((t) => t <= at && t >= at - STAGE_LOOKBACK_DAYS * DAY_MS);
    if (arrivals.length) samples.push(at - Math.min(...arrivals));
    else skipped.push(pin);
  }
  return {
    pins: staging.filter((s) => within(s, w)).length,
    medianMs: percentile(samples, 0.5),
    maxMs: samples.length ? Math.max(...samples) : null,
    measured: samples.length,
    skipped,
    why: samples.length
      ? null
      : skipped.length
        ? "no release's pin was on staging first"
        : "no releases in the window"
  };
}

/**
 * The release checks' verdicts on the prod bump PR in the window. Each candidate
 * force-pushed to the PR gets a fresh `release-checks-*` label, so a verdict
 * label is one candidate judged; `release-hold` is a person stopping one.
 */
export function releaseChecks(
  gate: GateEvent[] | null,
  w: Window,
  why?: string
): WindowMetrics["releaseChecks"] {
  if (!gate) return { checked: 0, passed: 0, failed: 0, held: 0, why: why ?? "not read" };
  const inWindow = gate.filter((g) => within(g, w));
  const count = (verdict: GateEvent["verdict"]) =>
    inWindow.filter((g) => g.verdict === verdict).length;
  const passed = count("passed");
  const failed = count("failed");
  return {
    checked: passed + failed,
    passed,
    failed,
    held: count("hold"),
    why: passed + failed ? null : "no release candidate was checked in the window"
  };
}

/** Whether a commit title types itself a fix or a revert (conventional commits, or git's revert). */
export function isFixTitle(title: string): boolean {
  return /^(?:fix|revert)(?:\([^)]*\))?!?:/i.test(title) || /^Revert "/.test(title);
}

/**
 * Changes to production that are not releases: commits under PROD_PRODUCT_DIRS,
 * minus the ones that moved the image (those are deploys, matched by
 * sha or by the PR number in a squash title). The `fix`/`revert` ones are
 * repairs made outside a release, which the rollback count cannot see.
 */
export function infraChanges(
  infra: InfraCommit[] | null,
  changes: ProdChange[],
  w: Window,
  why?: string
): WindowMetrics["infraChanges"] {
  if (!infra) return { total: 0, fixes: [], why: why ?? "not read" };
  const deployShas = new Set(changes.map((c) => c.sha));
  const deployPrs = new Set(changes.flatMap((c) => (c.pr === null ? [] : [c.pr])));
  const others = infra.filter((c) => {
    if (!within(c, w) || deployShas.has(c.sha)) return false;
    const pr = /\(#(\d+)\)$/.exec(c.title)?.[1];
    return !(pr && deployPrs.has(Number(pr)));
  });
  return {
    total: others.length,
    fixes: others.filter((c) => isFixTitle(c.title)),
    why: null
  };
}

export function windowMetrics(
  changes: ProdChange[],
  w: Window,
  signals: Signals | null = null
): WindowMetrics {
  const moves = changes.filter((c) => within(c, w));
  return {
    start: w.start.toISOString(),
    end: w.end.toISOString(),
    days: Math.round((w.end.getTime() - w.start.getTime()) / DAY_MS),
    deploys: moves.filter((c) => c.kind === "deploy").length,
    rollbacks: moves.filter((c) => c.kind === "rollback").length,
    viaPr: moves.filter((c) => c.pr !== null).length,
    viaPush: moves.filter((c) => c.pr === null).length,
    leadTime: leadTimes(changes, w),
    changeFailureRate: changeFailureRate(changes, w),
    timeToRestore: timeToRestore(changes, w),
    staging: stagingDwell(changes, signals?.staging ?? null, w, signals?.why.staging),
    releaseChecks: releaseChecks(signals?.gate ?? null, w, signals?.why.gate),
    infraChanges: infraChanges(signals?.infra ?? null, changes, w, signals?.why.infra)
  };
}

/** `--days`: a whole number of days, 1–365, or null for anything else. */
export function parseDays(raw: string | null): number | null {
  const n = Number(raw);
  return raw !== null && /^\d+$/.test(raw) && n >= 1 && n <= 365 ? n : null;
}

// ── Pure: saying it ─────────────────────────────────────────────────────────

export function fmtDuration(ms: number | null): string {
  if (ms === null) return "n/a";
  const minutes = ms / 60000;
  if (minutes < 60) return `${Math.round(minutes)}m`;
  if (minutes < 48 * 60) return `${(minutes / 60).toFixed(1)}h`;
  return `${(minutes / 1440).toFixed(1)}d`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const pct = (rate: number) => `${Math.round(rate * 100)}%`;

function windowLines(m: WindowMetrics, b: (s: string) => string): string[] {
  const lead = m.leadTime;
  const cfr = m.changeFailureRate;
  const ttr = m.timeToRestore;
  const failed = cfr.failed.length
    ? `; rolled back: ${cfr.failed.map((f) => f.pin).join(", ")}`
    : "";
  const caveats = [
    lead.truncatedDeploys
      ? `${plural(lead.truncatedDeploys, "deploy")} truncated at ${COMPARE_PAGES * 100} commits`
      : "",
    lead.unresolvedDeploys ? `${plural(lead.unresolvedDeploys, "deploy")} unresolved` : ""
  ].filter(Boolean);
  // Said for the whole channel, not only engineers (see deploy-messages.ts):
  // plain names first, and "not measured — why" where a number would mislead.
  const via = `${m.viaPr} through a reviewed release PR${m.viaPush ? `, ${m.viaPush} changed production directly with no PR` : ""}`;
  return [
    `${b("Releases")}: ${m.deploys ? `${m.deploys} (${via})` : "none"}`,
    lead.medianMs === null
      ? `${b("Time from merge to customers")}: not measured — ${lead.why}`
      : `${b("Time from merge to customers")}: typically ${human(lead.medianMs)}; the slowest 10% took ${human(lead.p90Ms)} (${plural(lead.commits, "change")} shipped${caveats.length ? `; ${caveats.join(", ")}` : ""})`,
    cfr.rate === null
      ? `${b("Releases rolled back")}: not measured — ${cfr.why}`
      : `${b("Releases rolled back")}: ${m.rollbacks ? `${m.rollbacks} of ${m.deploys} (${pct(cfr.rate)})${failed}` : `none (0 of ${m.deploys})`}`,
    ttr.medianMs === null
      ? `${b("Time to recover from a bad release")}: ${m.rollbacks ? `not measured — ${ttr.why}` : "nothing to recover from — no rollbacks"}`
      : `${b("Time to recover from a bad release")}: typically ${human(ttr.medianMs)} (${plural(ttr.restores.length, "rollback")})`
  ];
}

/**
 * What infra main says around the four: the staging stage inside lead time, the
 * gate in front of prod, and changes to prod that were not releases. Same voice.
 */
function aroundLines(m: WindowMetrics, b: (s: string) => string): string[] {
  const st = m.staging;
  const rc = m.releaseChecks;
  const ic = m.infraChanges;
  const skipped = st.skipped.length
    ? `; ${plural(st.skipped.length, "release")} went out without reaching staging first (${st.skipped.join(", ")})`
    : "";
  const notRead = (why: string | null) => why === null || /^(?:not read|could not read)/.test(why);
  const fixes = ic.fixes
    .slice(0, 3)
    .map((f) => f.title.replace(/\s+\(#\d+\)$/, ""))
    .join("; ");
  return [
    st.medianMs === null
      ? `${b("Time on staging first")}: not measured — ${st.why}${skipped}`
      : `${b("Time on staging first")}: typically ${human(st.medianMs)}, longest ${human(st.maxMs)} (${plural(st.measured, "release")}; staging took new code ${plural(st.pins, "time")})${skipped}`,
    rc.checked === 0
      ? `${b("Release checks on staging")}: ${notRead(rc.why) ? `not measured — ${rc.why}` : "none run"}${rc.held ? `; ${rc.held} held by a person` : ""}`
      : `${b("Release checks on staging")}: ${plural(rc.checked, "candidate")} checked, ${rc.failed ? `${rc.failed} failed` : "all passed"}${rc.held ? `; ${rc.held} held by a person` : ""}`,
    ic.why !== null
      ? `${b("Other changes to production")}: not measured — ${ic.why}`
      : `${b("Other changes to production")}: ${ic.total ? `${plural(ic.total, "infrastructure change")}, ${ic.fixes.length ? `${ic.fixes.length} of them fixes (${fixes}${ic.fixes.length > 3 ? "; …" : ""})` : "none of them fixes"}` : "none"}`
  ];
}

/** "24 hours", or "n/a" when there is nothing to say. */
const human = (ms: number | null) => (ms === null ? "n/a" : humanDuration(ms / 60000));

function contextLine(m: WindowMetrics): string {
  const lead =
    m.leadTime.medianMs === null
      ? "not measured"
      : `typically ${human(m.leadTime.medianMs)}, slowest 10% ${human(m.leadTime.p90Ms)}`;
  return `Last ${m.days} days: ${plural(m.deploys, "release")} · ${m.rollbacks ? `${m.rollbacks} rolled back` : "none rolled back"} · merge to customers ${lead}`;
}

/** The human summary: Slack mrkdwn, or GitHub markdown for a step summary. */
export function render(report: Report, format: "slack" | "markdown"): string {
  const b = format === "slack" ? (s: string) => `*${s}*` : (s: string) => `**${s}**`;
  const bullet = format === "slack" ? "•" : "-";
  const w = report.window;
  const day = (iso: string) =>
    new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", timeZone: "UTC" });
  const head = `${b(`Releases to production, last ${w.days} days`)} (${day(w.start)} – ${day(w.end)})`;
  const lines = [head, ...windowLines(w, b).map((l) => `${bullet} ${l}`)];
  lines.push(`${b("Around the releases")}`, ...aroundLines(w, b).map((l) => `${bullet} ${l}`));
  if (report.context) lines.push(`_${contextLine(report.context)}_`);
  lines.push(
    "_The first four are the DORA delivery measures: deploy frequency, lead time, change failure rate, time to restore. The rest are read from oxy-hq/infrastructure._"
  );
  return `${lines.join("\n")}\n`;
}

// ── Reads ───────────────────────────────────────────────────────────────────

type GhError = Error & { status: number };

async function gh<T = unknown>(path: string, token: Token): Promise<T> {
  const res = await fetch(`${API}${path}`, {
    headers: {
      accept: "application/vnd.github+json",
      authorization: `Bearer ${token}`,
      "x-github-api-version": "2022-11-28"
    }
  });
  if (!res.ok) {
    const err = new Error(`GET ${path} → ${res.status} ${await res.text()}`) as GhError;
    err.status = res.status;
    throw err;
  }
  return (await res.json()) as T;
}

/** "Does not exist", as opposed to "could not ask" — only the first may read as absence. */
const notFound = (e: unknown): boolean => [404, 422].includes((e as GhError).status);

/** The infra commits on main that touched the prod values file, newest first. */
async function prodCommits(tokens: Tokens, since: Date, until: Date): Promise<string[]> {
  const shas: string[] = [];
  for (let page = 1; ; page++) {
    const batch = await gh<{ sha: string }[]>(
      `/repos/${INFRA}/commits?sha=main&path=${PROD_VALUES}&since=${since.toISOString()}` +
        `&until=${until.toISOString()}&per_page=100&page=${page}`,
      tokens.infra
    );
    shas.push(...batch.map((c) => c.sha));
    if (batch.length < 100) return shas;
    // A page cap would be a silently partial history; the date range bounds it instead.
  }
}

/** A values file's `imageTag` at a ref, for when the commit API leaves the patch out. */
async function imageTagAt(ref: string, tokens: Tokens, path = PROD_VALUES): Promise<string | null> {
  try {
    const file = await gh<{ content?: string }>(
      `/repos/${INFRA}/contents/${path}?ref=${ref}`,
      tokens.infra
    );
    return imageTagOf(Buffer.from(file.content ?? "", "base64").toString("utf8"));
  } catch (e) {
    if (notFound(e)) return null;
    throw e;
  }
}

/** The merged PR that brought an infra commit to main, if it came through one. */
async function mergedPr(
  sha: string,
  tokens: Tokens
): Promise<{ number: number; title: string; mergedAt: string } | null> {
  const prs = await gh<
    { number: number; title: string; merged_at: string | null; base: { ref: string } }[]
  >(`/repos/${INFRA}/commits/${sha}/pulls`, tokens.infra);
  const pr = prs.find((p) => p.merged_at && p.base.ref === "main");
  return pr?.merged_at ? { number: pr.number, title: pr.title, mergedAt: pr.merged_at } : null;
}

/**
 * What one infra commit did to prod, or null if it did not move the pin.
 *
 * The commit API omits a patch for a large diff and lists at most 300 files, so
 * a prod file that is touched but not shown is read at the commit and its parent
 * instead — a move that cannot be seen is not the same as no move.
 */
async function readChange(sha: string, tokens: Tokens): Promise<RawChange | null> {
  const commit = await gh<{
    commit: { message: string; committer: { date: string } };
    parents: { sha: string }[];
    files?: { filename: string; patch?: string }[];
  }>(`/repos/${INFRA}/commits/${sha}`, tokens.infra);
  const subject = commit.commit.message.split("\n")[0] ?? "";
  let files = commit.files ?? [];
  if (!files.find((f) => f.filename === PROD_VALUES)?.patch) {
    const parent = commit.parents[0]?.sha;
    const before = parent ? await imageTagAt(parent, tokens) : null;
    const after = await imageTagAt(sha, tokens);
    const patch = `-imageTag: ${before ?? ""}\n+imageTag: ${after ?? ""}\n`;
    files = before !== null && after !== null ? [{ filename: PROD_VALUES, patch }] : [];
  }
  // Asked before the PR lookup: most commits on this file are config, not deploys.
  if (!imageTagMove(files.find((f) => f.filename === PROD_VALUES)?.patch)) return null;
  const pr = await mergedPr(sha, tokens);
  const title = pr?.title ?? subject;
  const move = prodMove({ title, files });
  if (!move) return null;
  return {
    ...move,
    sha,
    pr: pr?.number ?? null,
    title,
    at: pr?.mergedAt ?? commit.commit.committer.date
  };
}

/** The oxygen-internal commit a pin runs, or null when it cannot be named. */
async function internalOf(
  pin: Pin,
  tokens: Tokens,
  cache: Map<string, string | null>
): Promise<string | null> {
  const key = pinLabel(pin);
  const cached = cache.get(key);
  if (cached !== undefined) return cached;
  let sha: string | null = null;
  if (pin.kind === "main") {
    try {
      const commit = await gh<{ commit?: { message?: string } }>(
        `/repos/${MIRROR}/commits/${pin.sha}`,
        tokens.gh
      );
      sha = originRevId(commit.commit?.message);
    } catch (e) {
      if (!notFound(e)) throw e;
    }
  } else if (pin.kind === "semver") {
    // oxygen-internal tags releases bare (`0.5.150`); `v` is the fallback, not the rule.
    for (const tag of [pin.version, `v${pin.version}`]) {
      try {
        sha = (await gh<{ sha: string }>(`/repos/${INTERNAL}/commits/${tag}`, tokens.gh)).sha;
        break;
      } catch (e) {
        if (!notFound(e)) throw e;
      }
    }
  }
  cache.set(key, sha);
  return sha;
}

/**
 * `base...head` on oxygen-internal: the direction, and — when `pages` > 0 — the
 * commits head has that base does not, up to `pages` × 100.
 */
async function compareInternal(
  base: string,
  head: string,
  tokens: Tokens,
  pages: number
): Promise<{ status: string; total: number; commits: Shipped[] }> {
  type Page = {
    status: string;
    total_commits: number;
    commits: { sha: string; commit: { committer: { date: string } } }[];
  };
  const path = `/repos/${INTERNAL}/compare/${base}...${head}`;
  const first = await gh<Page>(`${path}?per_page=${pages ? 100 : 1}&page=1`, tokens.gh);
  if (!pages) return { status: first.status, total: first.total_commits, commits: [] };
  const commits = [...first.commits];
  for (let page = 2; page <= pages && commits.length < first.total_commits; page++) {
    const next = await gh<Page>(`${path}?per_page=100&page=${page}`, tokens.gh);
    if (!next.commits.length) break;
    commits.push(...next.commits);
  }
  return {
    status: first.status,
    total: first.total_commits,
    commits: commits.map((c) => ({ sha: c.sha, committedAt: c.commit.committer.date }))
  };
}

/**
 * Direction, kind and — for a deploy inside a window — what it shipped.
 *
 * A change older than every window is only needed for what it was (a rollback's
 * `undoneBy` looks for it), so two releases are ordered without a single call.
 */
async function resolve(
  change: RawChange,
  {
    tokens,
    cache,
    inWindow
  }: { tokens: Tokens; cache: Map<string, string | null>; inWindow: boolean }
): Promise<ProdChange> {
  const releases = change.from.kind === "semver" && change.to.kind === "semver";
  const internal = { from: null as string | null, to: null as string | null };
  let cmp: { status: string; total: number; commits: Shipped[] } | null = null;
  if (inWindow || !releases) {
    internal.from = await internalOf(change.from, tokens, cache);
    internal.to = await internalOf(change.to, tokens, cache);
    // A rollback's compare is `behind` with no commits, so reading it costs one
    // page either way; only a change outside every window skips the commit list.
    if (internal.from && internal.to)
      cmp = await compareInternal(internal.from, internal.to, tokens, inWindow ? COMPARE_PAGES : 0);
  }
  const direction = directionOf(change.from, change.to, cmp?.status ?? null);
  const kind = kindOf(change.rollbackTitle, direction);
  const read = kind === "deploy" && inWindow && cmp !== null;
  return {
    ...change,
    direction,
    kind,
    internal,
    shipped: read ? (cmp?.commits ?? null) : null,
    totalCommits: read ? (cmp?.total ?? null) : null
  };
}

/** Every prod change from `since` to `now`, oldest first, resolved. */
async function history(
  tokens: Tokens,
  { since, now, earliestWindow }: { since: Date; now: Date; earliestWindow: Date }
): Promise<ProdChange[]> {
  const raw: RawChange[] = [];
  // Oldest first, so `collapseByPr` sees a PR's commits in the order they landed.
  for (const sha of (await prodCommits(tokens, since, now)).reverse()) {
    const change = await readChange(sha, tokens);
    if (change) raw.push(change);
  }
  const ordered = collapseByPr(raw).sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  const cache = new Map<string, string | null>();
  const out: ProdChange[] = [];
  for (const change of ordered) {
    const inWindow = Date.parse(change.at) >= earliestWindow.getTime();
    out.push(await resolve(change, { tokens, cache, inWindow }));
  }
  return out;
}

/** `fn` over `items`, at most `limit` at a time, results in input order. */
async function mapLimit<T, R>(
  items: T[],
  limit: number,
  fn: (item: T) => Promise<R>
): Promise<R[]> {
  const out: R[] = new Array(items.length);
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const i = next++;
      out[i] = await fn(items[i] as T);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
  return out;
}

/** Commits on infra main under `path` from `since` to `until`, newest first. */
async function infraCommits(
  path: string,
  tokens: Tokens,
  since: Date,
  until: Date
): Promise<InfraCommit[]> {
  const out: InfraCommit[] = [];
  for (let page = 1; ; page++) {
    const batch = await gh<
      { sha: string; commit: { message: string; committer: { date: string } } }[]
    >(
      `/repos/${INFRA}/commits?sha=main&path=${path}&since=${since.toISOString()}` +
        `&until=${until.toISOString()}&per_page=100&page=${page}`,
      tokens.infra
    );
    out.push(
      ...batch.map((c) => ({
        sha: c.sha,
        at: c.commit.committer.date,
        title: c.commit.message.split("\n")[0] ?? ""
      }))
    );
    if (batch.length < 100) return out;
  }
}

/**
 * Every move of the staging pin from `since` to `until`. Read from each commit's
 * diff, like prod's, falling back to the file at the commit and its parent.
 */
async function stagingPins(tokens: Tokens, since: Date, until: Date): Promise<StagingPin[]> {
  const commits = await infraCommits(STAGING_VALUES, tokens, since, until);
  const pins = await mapLimit(commits, READ_CONCURRENCY, async (c) => {
    const commit = await gh<{
      parents: { sha: string }[];
      files?: { filename: string; patch?: string }[];
    }>(`/repos/${INFRA}/commits/${c.sha}`, tokens.infra);
    let patch = commit.files?.find((f) => f.filename === STAGING_VALUES)?.patch;
    if (!patch) {
      const parent = commit.parents[0]?.sha;
      const before = parent ? await imageTagAt(parent, tokens, STAGING_VALUES) : null;
      const after = await imageTagAt(c.sha, tokens, STAGING_VALUES);
      patch = `-imageTag: ${before ?? ""}\n+imageTag: ${after ?? ""}\n`;
    }
    const move = imageTagMove(patch);
    return move ? { pin: pinLabel(move.to), at: c.at } : null;
  });
  return pins.filter((p): p is StagingPin => p !== null);
}

/** Commits under any of PROD_PRODUCT_DIRS, once each, newest first. */
async function productCommits(tokens: Tokens, since: Date, until: Date): Promise<InfraCommit[]> {
  const perDir = await Promise.all(
    PROD_PRODUCT_DIRS.map((dir) => infraCommits(dir, tokens, since, until))
  );
  const seen = new Map(perDir.flat().map((c) => [c.sha, c]));
  return [...seen.values()].sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
}

/** The verdict labels put on prod bump PRs from `since` on. */
async function gateEvents(tokens: Tokens, since: Date): Promise<GateEvent[]> {
  const prs = await gh<{ number: number; updated_at: string }[]>(
    `/repos/${INFRA}/pulls?head=oxy-hq:${BUMP_BRANCH}&state=all&sort=updated&direction=desc&per_page=100`,
    tokens.infra
  );
  const verdicts: Record<string, GateEvent["verdict"]> = {
    "release-checks-passed": "passed",
    "release-checks-failed": "failed",
    "release-hold": "hold"
  };
  const recent = prs.filter((p) => Date.parse(p.updated_at) >= since.getTime());
  const perPr = await mapLimit(recent, READ_CONCURRENCY, async (pr) => {
    const events: GateEvent[] = [];
    for (let page = 1; ; page++) {
      const batch = await gh<{ event: string; created_at: string; label?: { name: string } }[]>(
        `/repos/${INFRA}/issues/${pr.number}/timeline?per_page=100&page=${page}`,
        tokens.infra
      );
      for (const e of batch) {
        const verdict = e.event === "labeled" ? verdicts[e.label?.name ?? ""] : undefined;
        if (verdict && Date.parse(e.created_at) >= since.getTime())
          events.push({ pr: pr.number, at: e.created_at, verdict });
      }
      if (batch.length < 100) return events;
    }
  });
  return perPr.flat();
}

/**
 * Everything infra main records beside the prod pin, from `since` to `now`. A
 * read that fails is reported as not measured, with why; it never stops the
 * report, which existed before these did.
 */
async function readSignals(tokens: Tokens, since: Date, now: Date): Promise<Signals> {
  const signals: Signals = { staging: null, gate: null, infra: null, why: {} };
  const why = (e: unknown) =>
    `could not read infra (${(e as GhError).status ?? (e as Error).message})`;
  const stageSince = new Date(since.getTime() - STAGE_LOOKBACK_DAYS * DAY_MS);
  await Promise.all([
    stagingPins(tokens, stageSince, now).then(
      (s) => (signals.staging = s),
      (e) => (signals.why.staging = why(e))
    ),
    gateEvents(tokens, since).then(
      (g) => (signals.gate = g),
      (e) => (signals.why.gate = why(e))
    ),
    productCommits(tokens, since, now).then(
      (c) => (signals.infra = c),
      (e) => (signals.why.infra = why(e))
    )
  ]);
  return signals;
}

export function buildReport(
  changes: ProdChange[],
  now: Date,
  days: number,
  signals: Signals | null = null
): Report {
  const back = (n: number) => new Date(now.getTime() - n * DAY_MS);
  return {
    generatedAt: now.toISOString(),
    basis: BASIS,
    window: windowMetrics(changes, { start: back(days), end: now }, signals),
    context:
      days === CONTEXT_DAYS
        ? null
        : windowMetrics(changes, { start: back(CONTEXT_DAYS), end: now }, signals),
    changes
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
  const patch = (from: string, to: string) =>
    `@@ -15,7 +15,7 @@ app:\n   image: ghcr.io/oxy-hq/oxygen\n-  imageTag: "${from}"\n+  imageTag: "${to}"\n`;
  const touching = (title: string, from: string, to: string) =>
    prodMove({ title, files: [{ filename: PROD_VALUES, patch: patch(from, to) }] });
  const label = (m: Move | null) =>
    m
      ? `${pinLabel(m.from)} → ${pinLabel(m.to)}${m.rollbackTitle ? " (rollback title)" : ""}`
      : null;

  // ── The classifier ────────────────────────────────────────────────────────
  is(
    "an old semver PR title: the diff's pins",
    label(
      touching("chore(oxy-prod): bump oxy image 0.5.149 → 0.5.150 (#2117)", "0.5.149", "0.5.150")
    ),
    "0.5.149 → 0.5.150"
  );
  is(
    "a direct push with no PR and no arrow still moves prod",
    label(touching("build: update image tag to 0.5.149", "0.5.148", "0.5.149")),
    "0.5.148 → 0.5.149"
  );
  const lying = touching(
    "deploy(oxy-prod): oxy 0.5.136 → 0.5.141, matching staging (#2039)",
    "0.5.136",
    "0.5.142"
  );
  is("a title naming the wrong version loses to the diff", label(lying), "0.5.136 → 0.5.142");
  is("...and what it claimed is kept, for the record", lying?.titleTarget, parsePin("0.5.141"));
  is(
    "a new-style main- title",
    label(
      touching(
        "chore(oxy-prod): bump oxy image main-999aaaa → main-abc1234",
        "main-999aaaa",
        "main-abc1234"
      )
    ),
    "main-999aaaa → main-abc1234"
  );
  is(
    "a new-style rollback title is a rollback title",
    label(
      touching(
        "chore(oxy-prod): roll back oxy image main-abc1234 → main-999aaaa",
        "main-abc1234",
        "main-999aaaa"
      )
    ),
    "main-abc1234 → main-999aaaa (rollback title)"
  );
  for (const t of ["revert oxy to 0.5.89", "Rollback prod", "rolled back oxy", 'Revert "bump"'])
    is(`"${t}" says rollback`, saysRollback(t), true);
  for (const t of [
    "Rollbar wiring",
    "chore(oxy-prod): bump oxy image 0.5.1 → 0.5.2",
    "Update imageTag from '0.5.111' to '0.5.110'"
  ])
    is(`"${t}" does not say rollback`, saysRollback(t), false);
  const digest = `main-abc1234@sha256:${"0".repeat(64)}`;
  const digestTitle = titleTarget(`chore(oxy-prod): bump oxy image main-999aaaa → ${digest}`);
  is(
    "a digest-pinned title names the bare sha",
    digestTitle?.kind === "main" ? digestTitle.sha : digestTitle,
    "abc1234"
  );
  is(
    "a digest-pinned title with the ellipsis form too",
    titleTarget("… → main-abc1234@sha256:…")?.kind,
    "main"
  );
  is(
    "a digest-pinned value in the diff is the same commit",
    pinLabel(parsePin(`"${digest}"`)),
    "main-abc1234"
  );
  is(
    "a digest added to an unchanged sha is not a move",
    imageTagMove(patch("main-abc1234", digest)),
    null
  );
  is(
    "a squash title's (#n) suffix does not hide the arrow",
    titleTarget("chore(oxy-prod): bump oxy image main-999aaaa → main-abc1234 (#2200)")?.kind,
    "main"
  );
  is(
    "a PR that does not touch the prod file is not a deploy",
    prodMove({
      title: "chore(oxy-staging): bump oxy image 0.5.149 → 0.5.150",
      files: [
        {
          filename: PROD_VALUES.replace("oxy.yaml", "oxy-staging.yaml"),
          patch: patch("0.5.149", "0.5.150")
        }
      ]
    }),
    null
  );
  is(
    "a PR that touches the prod file but not imageTag is not a deploy",
    prodMove({
      title: "feat(sentry): prod environment (#2095)",
      files: [
        { filename: PROD_VALUES, patch: "@@ -40 +40 @@\n-  SENTRY_ENV: x\n+  SENTRY_ENV: prod\n" }
      ]
    }),
    null
  );
  is(
    "a revert that touches the file but not imageTag is not a rollback of anything",
    prodMove({
      title: 'Revert "feat(observability): prod cutover" (#1777)',
      files: [{ filename: PROD_VALUES, patch: "-  a: 1\n+  a: 2\n" }]
    }),
    null
  );
  is(
    "a trailing comment on the tag line is not part of the pin",
    parsePin('"0.5.150" # pinned by hand').kind,
    "semver"
  );
  is(
    "imageTag is read from a whole values file",
    imageTagOf('app:\n  imageTag: "0.5.150"\n'),
    '"0.5.150"'
  );

  // Direction and kind.
  is("releases order numerically, not as text", semverOrder("0.5.9", "0.5.10"), "forward");
  is("an older release is backward", semverOrder("0.5.111", "0.5.110"), "backward");
  is(
    "an unlabelled move to an older release is a rollback",
    kindOf(false, directionOf(parsePin("0.5.111"), parsePin("0.5.110"), null)),
    "rollback"
  );
  is(
    "a main- move reads the compare status",
    directionOf(parsePin("main-aaaaaaa"), parsePin("main-bbbbbbb"), "behind"),
    "backward"
  );
  is(
    "a diverged compare is not guessed at",
    directionOf(parsePin("main-aaaaaaa"), parsePin("main-bbbbbbb"), "diverged"),
    "unknown"
  );
  is(
    "an unknown direction is a deploy unless the title says otherwise",
    kindOf(false, "unknown"),
    "deploy"
  );
  is("a rollback title wins over a forward move", kindOf(true, "forward"), "rollback");

  // One PR landed as several commits is one change.
  const raw = (pr: number | null, from: string, to: string, at: string): RawChange => ({
    sha: `${from}-${to}`,
    pr,
    title: "t",
    at,
    from: parsePin(from),
    to: parsePin(to),
    rollbackTitle: false,
    titleTarget: null
  });
  is(
    "a PR's commits collapse to first from → last to; direct pushes stay separate",
    collapseByPr([
      raw(null, "0.5.1", "0.5.2", "2026-09-01T00:00:00Z"),
      raw(7, "0.5.2", "0.5.3", "2026-09-02T00:00:00Z"),
      raw(7, "0.5.3", "0.5.4", "2026-09-02T00:00:00Z"),
      raw(null, "0.5.4", "0.5.5", "2026-09-03T00:00:00Z")
    ]).map((c) => `${pinLabel(c.from)}→${pinLabel(c.to)}`),
    ["0.5.1→0.5.2", "0.5.2→0.5.4", "0.5.4→0.5.5"]
  );

  // ── The metrics ───────────────────────────────────────────────────────────
  const now = new Date("2026-09-24T02:00:00Z");
  const ago = (days: number, minutes = 0) =>
    new Date(now.getTime() - days * DAY_MS + minutes * 60000).toISOString();
  const change = (
    at: string,
    from: string,
    to: string,
    extra: Partial<ProdChange> = {}
  ): ProdChange => {
    const f = parsePin(from);
    const t = parsePin(to);
    const rollbackTitle = extra.rollbackTitle ?? false;
    const direction = directionOf(f, t, null);
    return {
      sha: `${from}-${to}`,
      pr: null,
      title: "",
      at,
      from: f,
      to: t,
      rollbackTitle,
      titleTarget: null,
      direction,
      kind: kindOf(rollbackTitle, direction),
      internal: { from: null, to: null },
      shipped: [],
      totalCommits: 0,
      ...extra
    };
  };
  const week: Window = { start: new Date(ago(7)), end: now };
  const month: Window = { start: new Date(ago(30)), end: now };
  const commitsAt = (...hoursBefore: [string, number, number][]) =>
    hoursBefore.map(([sha, days, h]) => ({ sha, committedAt: ago(days, -h * 60) }));

  const history: ProdChange[] = [
    change(ago(20), "0.5.1", "0.5.2", {
      pr: 10,
      shipped: commitsAt(["a", 20, 30]),
      totalCommits: 1
    }),
    change(ago(5), "0.5.2", "0.5.3", {
      shipped: commitsAt(["b", 5, 2], ["c", 5, 10]),
      totalCommits: 2
    }),
    change(ago(5, 40), "0.5.3", "0.5.2"),
    change(ago(2), "0.5.2", "0.5.4", {
      pr: 11,
      // `b` again — it went out with 0.5.3, was rolled back, and returns here.
      shipped: commitsAt(["b", 5, 2], ["d", 2, 4]),
      totalCommits: 2
    })
  ];
  const w7 = windowMetrics(history, week);
  is("frequency counts deploys and rollbacks separately", [w7.deploys, w7.rollbacks], [2, 1]);
  is("...and says how each landed", [w7.viaPr, w7.viaPush], [1, 2]);
  is("a rollback is recognised by order alone", history[2]?.kind, "rollback");
  is("CFR is rollbacks over deploys", w7.changeFailureRate.rate, 0.5);
  is(
    "the rolled-back deploy is named",
    w7.changeFailureRate.failed.map((f) => f.pin),
    ["0.5.3"]
  );
  is(
    "time to restore is rollback minus the deploy it undid",
    w7.timeToRestore.restores[0]?.ms,
    40 * 60000
  );
  is("...and says which deploy that was", w7.timeToRestore.restores[0]?.undid, "0.5.3");
  is("lead time counts a re-shipped commit once, at its first arrival", w7.leadTime.commits, 3);
  is("lead-time median over b, c, d (2h, 4h, 10h) is 4h", w7.leadTime.medianMs, 4 * 3600000);
  const w30 = windowMetrics(history, month);
  is("the 30-day line sees the older deploy too", [w30.deploys, w30.rollbacks], [3, 1]);
  is("a deploy followed by a deploy is not failed", failedDeploys(history).length, 1);

  // An empty window is n/a with a reason, never a zero.
  const empty = windowMetrics([], week);
  is("an empty window counts nothing", [empty.deploys, empty.rollbacks], [0, 0]);
  is("an empty window's CFR is n/a, not 0", empty.changeFailureRate.rate, null);
  is(
    "...lead time too",
    [empty.leadTime.medianMs, empty.leadTime.why],
    [null, "no deploys in the window"]
  );
  is("...and restore", empty.timeToRestore.why, "no rollbacks in the window");
  const shown = render(
    { generatedAt: now.toISOString(), basis: BASIS, window: empty, context: null, changes: [] },
    "slack"
  );
  is("the rendered empty window says it was not measured", shown.includes("not measured"), true);
  is("...and never claims a 0% failure rate", shown.includes("0%"), false);
  is("...or a rollback count", shown.includes("0 of"), false);

  // A rollback with nothing before it to have undone.
  const orphan = windowMetrics([change(ago(1), "0.5.9", "0.5.8")], week);
  is("an orphan rollback still counts as a rollback", orphan.rollbacks, 1);
  is("...its restore time is unknown, not zero", orphan.timeToRestore.restores[0]?.ms, null);
  is(
    "...and the median says why",
    orphan.timeToRestore.medianMs === null && orphan.timeToRestore.why !== null,
    true
  );
  is("...and with no deploys CFR is n/a rather than infinite", orphan.changeFailureRate.rate, null);

  // A rollback past several deploys undid the one that put its pin on prod.
  const chained = [
    change(ago(3), "0.5.1", "0.5.2"),
    change(ago(2), "0.5.2", "0.5.3"),
    change(ago(1), "0.5.3", "0.5.1", { rollbackTitle: true })
  ];
  is(
    "a chained rollback undid the latest deploy of its from pin",
    undoneBy(chained, 2)?.at,
    ago(2)
  );

  // Percentiles.
  is("one sample is its own median", percentile([5], 0.5), 5);
  is("...and its own p90", percentile([5], 0.9), 5);
  is("median of 1..10 interpolates", percentile([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 0.5), 5.5);
  is(
    "p90 of 1..10 interpolates",
    Number(percentile([10, 9, 8, 7, 6, 5, 4, 3, 2, 1], 0.9)?.toFixed(6)),
    9.1
  );
  is("no samples is no percentile", percentile([], 0.5), null);

  // A deploy whose pins did not resolve is counted, and said, not dropped.
  const unresolved = windowMetrics(
    [change(ago(1), "0.5.1", "0.5.2", { shipped: null, totalCommits: null })],
    week
  );
  is("an unresolved deploy is still a deploy", unresolved.deploys, 1);
  is("...and lead time says it could not see it", unresolved.leadTime.unresolvedDeploys, 1);
  const truncated = windowMetrics(
    [change(ago(1), "0.5.1", "0.5.2", { shipped: commitsAt(["x", 1, 1]), totalCommits: 900 })],
    week
  );
  is("a compare past the cap is counted as truncated", truncated.leadTime.truncatedDeploys, 1);

  // ── What infra main says around the releases ──────────────────────────────
  // The week's deploys are 0.5.3 (ago 5) and 0.5.4 (ago 2); 0.5.3 was rolled back.
  const signals: Signals = {
    staging: [
      { pin: "0.5.4", at: ago(30) }, // too long before its release to count
      { pin: "0.5.3", at: ago(6) }, // first arrival: a day before prod
      { pin: "0.5.3", at: ago(5, -30) }, // again, 30m before prod; the first arrival counts
      { pin: "0.5.4", at: ago(1) } // after its release: not "staging first"
    ],
    gate: [
      { pr: 11, at: ago(10), verdict: "failed" }, // before the week
      { pr: 11, at: ago(6), verdict: "passed" },
      { pr: 11, at: ago(4), verdict: "failed" },
      { pr: 11, at: ago(4, 30), verdict: "hold" },
      { pr: 11, at: ago(3), verdict: "passed" }
    ],
    infra: [
      { sha: "0.5.2-0.5.4", at: ago(2), title: "chore(oxy-prod): bump oxy image (#11)" },
      { sha: "x1", at: ago(2), title: "chore(oxy-prod): bump oxy image, earlier commit (#11)" },
      { sha: "x2", at: ago(3), title: "fix(airhouse-prod): connection caps 48 → 96 (#20)" },
      { sha: "x3", at: ago(4), title: "feat(airhouse-prod): bigger nodes (#21)" },
      { sha: "x4", at: ago(9), title: "fix(oxy): before the week (#22)" }
    ],
    why: {}
  };
  const around = windowMetrics(history, week, signals);
  is(
    "time on staging is the pin's first arrival there → prod",
    [around.staging.measured, around.staging.medianMs],
    [1, DAY_MS]
  );
  is(
    "a release whose pin was not on staging before it is named as skipping it",
    around.staging.skipped,
    ["0.5.4"]
  );
  is("staging's own moves in the week are counted", around.staging.pins, 3);
  is("release checks count the week's verdicts, a hold apart", around.releaseChecks, {
    checked: 3,
    passed: 2,
    failed: 1,
    held: 1,
    why: null
  });
  is(
    "other changes leave out the release's own commits, by sha and by PR",
    around.infraChanges.total,
    2
  );
  is(
    "...and the fixes among them are the ones typed fix",
    around.infraChanges.fixes.map((f) => f.sha),
    ["x2"]
  );
  for (const [title, fix] of [
    ["fix(airhouse-prod): caps (#1)", true],
    ["fix: plain", true],
    ["fix!: breaking", true],
    ['Revert "feat: a thing"', true],
    ["revert(oxy): b", true],
    ["feat: fix the thing", false],
    ["fixup! squash me", false],
    ["chore(oxy-prod): bump oxy image", false]
  ] as const)
    is(`${JSON.stringify(title)} is ${fix ? "" : "not "}a fix`, isFixTitle(title), fix);

  const aroundShown = render(
    { generatedAt: now.toISOString(), basis: BASIS, window: around, context: null, changes: [] },
    "slack"
  );
  for (const want of [
    "*Time on staging first*: typically 24 hours, longest 24 hours (1 release; staging took new code 3 times); 1 release went out without reaching staging first (0.5.4)",
    "*Release checks on staging*: 3 candidates checked, 1 failed; 1 held by a person",
    "*Other changes to production*: 2 infrastructure changes, 1 of them fixes (fix(airhouse-prod): connection caps 48 → 96)"
  ])
    is(`rendered: ${want.slice(0, 40)}…`, aroundShown.includes(want), true);

  // Unread is "not measured", never a zero, and a failed read says why.
  const unread = windowMetrics(history, week, {
    staging: null,
    gate: null,
    infra: null,
    why: { staging: "could not read infra (403)" }
  });
  is(
    "an unread staging history keeps its reason",
    unread.staging.why,
    "could not read infra (403)"
  );
  const unreadShown = render(
    { generatedAt: now.toISOString(), basis: BASIS, window: unread, context: null, changes: [] },
    "slack"
  );
  is(
    "...and says so instead of a number",
    [
      unreadShown.includes("Time on staging first*: not measured — could not read infra (403)"),
      unreadShown.includes("Release checks on staging*: not measured — not read"),
      unreadShown.includes("Other changes to production*: not measured — not read")
    ],
    [true, true, true]
  );
  is(
    "a week with no candidates checked says none ran, not that it could not tell",
    render(
      {
        generatedAt: now.toISOString(),
        basis: BASIS,
        window: windowMetrics(history, week, { ...signals, gate: [] }),
        context: null,
        changes: []
      },
      "slack"
    ).includes("Release checks on staging*: none run"),
    true
  );

  // Flags and formatting.
  is("--days takes a whole number", parseDays("7"), 7);
  for (const bad of ["0", "-1", "7.5", "abc", "", "400"])
    is(`--days ${JSON.stringify(bad)} is refused`, parseDays(bad), null);
  is("durations under an hour are minutes", fmtDuration(40 * 60000), "40m");
  is("under two days, hours", fmtDuration(26 * 3600000), "26.0h");
  is("beyond, days", fmtDuration(4 * DAY_MS), "4.0d");

  // The workflow is the one caller that runs unattended, so a flag it passes that
  // this file does not read is a weekly report built on a silently ignored input.
  const workflow = readFileSync(
    new URL("../workflows/deploy-metrics.yaml", import.meta.url),
    "utf8"
  );
  const passed = [...workflow.matchAll(/deploy-metrics\.ts((?:[^\n]*\\\n)*[^\n]*)/g)].flatMap((m) =>
    [...(m[1] ?? "").matchAll(/--([a-z][a-z-]*)/g)].map((f) => f[1] ?? "")
  );
  is(
    `every flag deploy-metrics.yaml passes is one this file reads (${passed.join(", ")})`,
    passed.filter((f) => !FLAGS.includes(f)),
    []
  );
  is(
    "...and it found the invocations to check",
    passed.includes("days") && passed.includes("summary-file"),
    true
  );

  if (fails.length) {
    console.error(
      `deploy-metrics.ts self-test: ${fails.length} failure(s)\n\n  ${fails.join("\n  ")}\n`
    );
    process.exit(1);
  }
  console.log("deploy-metrics.ts self-test: all classifier, metric and rendering cases pass");
}

// ── Entry ───────────────────────────────────────────────────────────────────

/** The value after `--name`, or `fallback`; never `true` (see promote.ts's `arg`). */
function arg(name: string): string | null;
function arg(name: string, fallback: string): string;
function arg(name: string, fallback: string | null = null): string | null {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return fallback;
  const value = process.argv[i + 1];
  return value === undefined || value.startsWith("--") ? fallback : value;
}

async function main(): Promise<void> {
  const token = process.env.GH_TOKEN;
  const days = parseDays(arg("days", "7"));
  const now = arg("now") ? new Date(arg("now") as string) : new Date();
  const format = arg("format", "slack");
  if (
    !token ||
    days === null ||
    Number.isNaN(now.getTime()) ||
    !["slack", "markdown"].includes(format)
  ) {
    console.error(
      "usage: GH_TOKEN=… [INFRA_TOKEN=…] deploy-metrics.ts [--days 1-365] [--now <ISO>] " +
        "[--json] [--format slack|markdown] [--summary-file <path>] | --self-test"
    );
    process.exit(1);
  }
  const tokens = { gh: token, infra: process.env.INFRA_TOKEN || token };
  const span = Math.max(days, CONTEXT_DAYS);
  const earliestWindow = new Date(now.getTime() - span * DAY_MS);
  const [changes, signals] = await Promise.all([
    history(tokens, {
      since: new Date(now.getTime() - (span + LOOKBACK_DAYS) * DAY_MS),
      now,
      earliestWindow
    }),
    readSignals(tokens, earliestWindow, now)
  ]);
  const report = buildReport(changes, now, days, signals);
  const summaryFile = arg("summary-file");
  if (summaryFile) await appendFile(summaryFile, render(report, "markdown"));
  if (process.argv.includes("--json")) console.log(JSON.stringify(report, null, 2));
  else process.stdout.write(render(report, format as "slack" | "markdown"));
}

const invokedDirectly = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;

if (!invokedDirectly) {
  // imported: exports only
} else if (process.argv.includes("--self-test")) {
  selfTest();
} else {
  await main();
}
