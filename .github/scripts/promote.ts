// Decides what each environment should be running, and whether it may move.
//
// This is the reasoning half of the deploy train (`internal-docs/deploy-pipeline.md`);
// `promote.yaml` does the writing. The split is deliberate: the effects need
// tokens, git and a checkout, while the decision is the part that has to be
// reviewable in a diff and provable without a live repo (`--self-test`).
//
// It is a RECONCILER, not an event handler. Every run asks the same question from
// scratch — "for each environment, what should it serve, and is it?" — and never
// depends on having seen a build finish. That shape was chosen because the thing
// it replaces was an event chain across three repos where every link failed
// silently at least once: a held PR quietly refused every later release, the run
// stayed green, and the only trace was a warning in a public Actions log nobody
// reads. A reconciler cannot accumulate that kind of silence.
//
// Four facts about this setup are load-bearing, and each one is a trap if you
// assume otherwise:
//
//   * **The sha is the MIRROR's.** Images are built in `oxy-hq/oxygen`, so
//     `main-<sha>` and `/api/version`'s `build_info.git_commit_short` are MIRROR
//     commits.
//     Copybara writes `GitOrigin-RevId: <internal sha>` into every mirrored commit
//     message and that trailer is the only mapping back. A compare link built from
//     a mirror sha against oxygen-internal points at a commit that does not exist.
//   * **A published image does not mean green CI.** `Public Release` triggers on a
//     push to the mirror's main; `CI check` runs in oxygen-internal. So an image
//     exists for commits whose tests failed. The candidate has to be checked
//     against the INTERNAL commit's CI conclusion.
//   * **An absent CI run is not a failure.** `ci.yaml` is `paths-ignore`d for
//     `docs/**` and `internal-docs/**`, so a docs-only commit legitimately has no
//     run. Treating "no run" as "not verified" would stall the train behind a
//     typo fix. A *failed* run is the signal; absence is not.
//   * **`main-<sha>` is published last** by `create-manifests`, so its presence on
//     ghcr is the "this build finished" marker. Do not look for `main-latest`:
//     it is a moving tag and says nothing about which digest it points at.
//
// Usage:
//   GH_TOKEN=… node .github/scripts/promote.ts --plan [--target staging|prod|all]
//     [--sha <short>] [--now <ISO>] [--out plan.json]
//   node .github/scripts/promote.ts --self-test
//
// `--plan` prints the plan as JSON on stdout and, with `--out`, writes it too.
// It performs no writes of any kind. Exit code is 0 whenever it produced a plan —
// "nothing to promote" is the common answer and is a success.

import { readFileSync, realpathSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

// ── Shapes ──────────────────────────────────────────────────────────────────
// Stated rather than inferred: these are the contract between this file, the
// GitHub API, and the `jq` expressions in promote.yaml that read the plan.

/** A GitHub Actions-style token. */
type Token = string;

/** Which end of a repeated marker to believe. */
type Pick = "earliest" | "latest";

/** The verdict `custom-app-checks.yaml` records on the bump PR, as a label. */
type Checks = "passed" | "failed" | "not-run" | "pending" | null;

interface Candidate {
  /** The MIRROR's short sha, as `main-<sha>` spells it. */
  sha: string;
  /** The oxygen-internal commit behind it, via copybara's trailer. */
  internal: string | null;
  ci: string | null;
  subject: string;
  /** ISO 8601, from the mirror commit. Null when it could not be read. */
  committedAt: string | null;
  /**
   * The manifest-list digest behind `main-<sha>`, read off the registry. What
   * actually gets pinned: a tag in ghcr can be overwritten, a digest cannot.
   */
  digest: string | null;
}

interface Serving {
  sha: string | null;
  version: string | null;
  /** Only resolved for the environments a compare range is built from. */
  internal?: string | null;
}

interface BumpPr {
  number: number;
  /** The BARE sha the title proposes, or null for a semver/absent title. */
  proposes: string | null;
  /**
   * The `imageTag` the PR branch's prod values file pins — the bytes, where the
   * title only names the sha. `undefined` when it could not be read.
   */
  proposesPin?: string | null;
  labels: string[];
  held: boolean;
  rollbackOpen: boolean;
  checks: Checks;
}

interface StagingState {
  candidate: string | null;
  /** False when the targets file names no dev environment at all. */
  devConfigured?: boolean;
  stagingSha?: string | null;
  devSha?: string | null;
  devReachable?: boolean;
  candidateCommittedAt?: Date | null;
}

interface ProdState {
  candidate: string | null;
  prodSha?: string | null;
  stagingSha?: string | null;
  checks?: Checks;
  servedSince?: Date | null;
  held?: boolean;
  rollbackOpen?: boolean;
  sentryNewIssues?: number;
  /** `pinDrift`'s verdict: the digest staging ran is not the one on ghcr now. */
  drift?: string | null;
}

interface DispatchState {
  prProposesCandidate: boolean;
  stagingServesCandidate: boolean;
  checks: Checks;
  dispatchedAt: Date | null;
}

interface StuckState {
  kind?: string;
  /** A deliberate stop is not an incident. */
  held?: boolean;
  soakStartedAt?: Date | null;
  alertedAt?: Date | null;
}

interface Window {
  days: number[];
  fromHour: number;
  toHour: number;
  zone: string;
}

const MIRROR = "oxy-hq/oxygen";
const INFRA = "oxy-hq/infrastructure";
/** The same paths promote.yaml's `env:` names; the self-test holds them equal. */
export const PROD_VALUES =
  "oxy-workload/664267706513/us-west-2/oxy-prod/products/oxy/gitops/values/oxy.yaml";
export const STAGING_VALUES =
  "oxy-workload/575455576647/us-west-2/oxy-dev/products/oxy/gitops/values/oxy-staging.yaml";
/**
 * How recently the mirror's last `Public Release` on main must have failed for
 * this pass to say so. The reconciler keeps no state, so "say it once" is "say it
 * on the passes that start within this long of the failure": two, or three when
 * GitHub's cron runs late — never every 15 minutes for as long as it stays red.
 */
const BUILD_ALERT_MINUTES = 30;
const INTERNAL = "oxy-hq/oxygen-internal";
const API = "https://api.github.com";

/** How long a digest must have been serving staging, green, before prod may have it. */
const SOAK_MINUTES = 30;

/**
 * How long a `pending` verdict may stand before the checks are dispatched again.
 *
 * Above `custom-app-checks.yaml`'s own 120-minute job timeout, so a run that is
 * merely slow is never duplicated. It exists because `pending` is a promise that
 * something will report, and a run cancelled between its dispatch and its
 * `always()` record step breaks that promise silently — which stalls the train in
 * the same shape as the defect this constant was added alongside.
 */
const STALE_CHECKS_MINUTES = 150;

/**
 * How long a digest may sit un-promoted before somebody is told.
 *
 * A reconciler that only ever speaks in its own run summary is as quiet as the
 * event chain it replaced — the failure it was built to stop is "green run,
 * nothing happening, nobody knows". Six hours is long enough that an ordinary
 * wait (build, soak, a checks run) never pages anyone, and short enough that a
 * train stopped on a Monday is not discovered on a Tuesday.
 */
const STUCK_MINUTES = 360;

/**
 * How long dev gets to prove a digest boots before staging stops waiting.
 *
 * Measured from the COMMIT, not from when the image appeared, because nothing
 * records the latter — so it has to cover the build too (~40-70 min) plus dev's
 * rollout. Two hours is comfortably past that and still bounds the stall: dev
 * being down holds staging for a while, never forever.
 */
const DEV_GRACE_MINUTES = 120;

/**
 * When prod may move on its own. Mon–Thu, working hours, Asia/Ho_Chi_Minh.
 *
 * Friday is deliberately absent rather than "Mon–Fri": the point of a window is
 * that somebody is around to notice, and on Friday evening nobody is. A dispatch
 * ignores the window — a human asking for it at 22:00 has taken that decision.
 */
const WINDOW = { days: [1, 2, 3, 4], fromHour: 9, toHour: 17, zone: "Asia/Ho_Chi_Minh" };

/** Unresolved Sentry issues first seen in the candidate's release, above which prod waits. */
const SENTRY_NEW_ISSUE_LIMIT = 0;

/** A CI conclusion that means "this commit is not promotable". Absence is not here. */
const CI_FAILED = new Set(["failure", "cancelled", "timed_out", "startup_failure"]);

// ── Pure decisions ──────────────────────────────────────────────────────────
// Everything below this line is a function of its arguments, so `--self-test`
// can pin it. Anything that reaches the network lives further down.

/**
 * Whether `at` falls inside the promote window.
 *
 * Reads the local weekday and hour out of `Intl` rather than doing arithmetic on
 * a UTC offset: Asia/Ho_Chi_Minh does not observe DST today, but a hardcoded +7
 * is a fact about the zone that the zone is entitled to change.
 */
export function inWindow(at: Date, window: Window = WINDOW): boolean {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone: window.zone,
    weekday: "short",
    hour: "numeric",
    hour12: false
  }).formatToParts(at);
  const weekday = parts.find((p) => p.type === "weekday")?.value;
  const hour = Number(parts.find((p) => p.type === "hour")?.value);
  // `?? ""` keeps the day at -1 if Intl ever stops emitting the part, which
  // `days.includes` rejects — closed, which is the right direction for a window.
  const day = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].indexOf(weekday ?? "");
  return window.days.includes(day) && hour >= window.fromHour && hour < window.toHour;
}

/**
 * Whether a candidate may be pinned on staging yet.
 *
 * dev exists to be the first rung, and after the deploy train it nearly stopped
 * being one: staging used to run the last *release* and now runs `main`, so the
 * only things left dividing them are that staging filters on CI and is
 * prod-shaped (RDS IAM, the serve/ide split, real S3). Requiring dev to have
 * actually served the digest puts a rung back — it catches "this image does not
 * boot" and "this migration wedges startup" one environment earlier, which is
 * what a first rung is for and is provable even though dev is unmonitored.
 *
 * It is not a hard gate, because dev being broken is not evidence about the
 * candidate forever. After [`DEV_GRACE_MINUTES`] staging stops waiting and says
 * why. A shas-have-no-order caveat is why this is a timeout and not a
 * "dev is ahead" comparison: `main-0000fff` is not newer than `main-fff0000`.
 */
export function stagingGate(
  state: StagingState,
  { now }: { now: Date }
): { promote: boolean; why: string } {
  const { candidate, stagingSha, devSha, devReachable, candidateCommittedAt } = state;
  const devConfigured = state.devConfigured ?? true;
  if (!candidate) return { promote: false, why: "no promotable digest" };
  if (stagingSha === candidate) return { promote: false, why: "staging serves it" };
  if (devSha === candidate) return { promote: true, why: "dev serves it" };
  if (!devConfigured)
    return { promote: true, why: "no dev environment configured; no rung to wait on" };

  const age = candidateCommittedAt
    ? (now.getTime() - candidateCommittedAt.getTime()) / 60000
    : Infinity;
  if (age > DEV_GRACE_MINUTES)
    return {
      promote: true,
      why: `dev has not served it ${Math.floor(age)}m after the commit; not waiting longer`
    };
  if (!devReachable)
    return { promote: false, why: "dev is not answering; holding staging until it does" };
  return {
    promote: false,
    why: `waiting for dev to serve it (${Math.floor(age)}m since the commit)`
  };
}

/**
 * Whether a candidate may be promoted to prod, and if not, the single reason.
 *
 * One reason, not a list: a plan that says "four gates are open" invites picking
 * the convenient one. The first unmet gate in this order is the one to act on,
 * and the order runs from "a person has to decide" down to "wait".
 *
 * A failing gate is not an error. `waiting` means the reconciler will ask again;
 * `blocked` means it will not until somebody does something.
 */
export function prodGate(
  state: ProdState,
  { now, dispatched = false }: { now: Date; dispatched?: boolean }
): { promote: boolean; kind: "waiting" | "current" | "blocked" | "go"; why: string } {
  const {
    candidate,
    prodSha,
    stagingSha,
    checks,
    servedSince,
    held,
    rollbackOpen,
    sentryNewIssues = 0,
    drift = null
  } = state;

  if (!candidate) return { promote: false, kind: "waiting", why: "no promotable digest" };
  if (candidate === prodSha) return { promote: false, kind: "current", why: "prod serves it" };
  if (rollbackOpen)
    return {
      promote: false,
      kind: "blocked",
      why: "a rollback PR is open; merge or close it, then promote by hand"
    };
  if (held)
    return {
      promote: false,
      kind: "blocked",
      why: "the bump PR carries release-hold; a person clears it"
    };
  if (drift) return { promote: false, kind: "blocked", why: drift };
  if (stagingSha !== candidate)
    return { promote: false, kind: "waiting", why: "staging does not serve the candidate yet" };
  if (checks !== "passed")
    return {
      promote: false,
      // `not-run` and `failed` are both "not a pass", but only one of them is
      // somebody's decision: a failed check sets release-hold and is caught above,
      // so anything left here is a check that has not reported and will be
      // re-dispatched.
      kind: "waiting",
      why: `staging checks are ${checks ?? "pending"}`
    };
  const soakedFor = servedSince ? (now.getTime() - servedSince.getTime()) / 60000 : 0;
  if (soakedFor < SOAK_MINUTES)
    return {
      promote: false,
      kind: "waiting",
      why: `soaked ${Math.floor(soakedFor)}m of ${SOAK_MINUTES}m on staging`
    };
  if (sentryNewIssues < 0)
    return {
      promote: false,
      kind: "blocked",
      why: "new-Sentry-issue count was not measured (SENTRY_READ_TOKEN unset?); a gate that passes unmeasured is not a gate"
    };
  if (sentryNewIssues > SENTRY_NEW_ISSUE_LIMIT)
    return {
      promote: false,
      kind: "blocked",
      why: `${sentryNewIssues} new unresolved Sentry issue(s) in this release on staging`
    };
  if (!dispatched && !inWindow(now))
    return {
      promote: false,
      kind: "waiting",
      why: "outside the promote window (Mon–Thu 09:00–17:00 Asia/Ho_Chi_Minh)"
    };
  return { promote: true, kind: "go", why: "every gate passed" };
}

/** The internal sha a mirrored commit came from, or null. */
export function originRevId(commitMessage: string | null | undefined): string | null {
  const line = /^GitOrigin-RevId:\s*([0-9a-f]{7,40})\s*$/im.exec(commitMessage ?? "");
  return line?.[1] ?? null;
}

/**
 * The sha a bump PR proposes, from its title's `→ <ref>` suffix.
 *
 * The title carries the ref as the values file spells it — `main-abc1234` — and
 * every comparison downstream is against a bare sha. Returning the prefixed form
 * made `proposes === candidate.sha` false on every pass, which forced `checks` to
 * null (so prod could never promote) AND made `prodNeedsProposal` permanently
 * true, so every 15-minute pass re-pushed the branch and replaced
 * `release-checks-passed` with `release-checks-pending` — erasing the verdict it
 * was waiting for. Both silent.
 *
 * A semver title (`→ 0.5.150`, the publish train's shape) yields null rather than
 * a fake sha: it proposes something, but not a digest this reconciler can match.
 */
export function proposedSha(title: string | null | undefined): string | null {
  const ref = /→ (\S+)$/.exec(title ?? "")?.[1];
  // The sha only: a title that ever carries a digest — whole, or elided to
  // `@sha256:…` as a human writes it — still yields the bare sha every
  // comparison here is made in, and `main-<not a sha>` yields null.
  return /^main-([0-9a-f]{7,40})(?:@|$)/.exec(ref ?? "")?.[1] ?? null;
}

/** A manifest digest as the registry spells it. */
const DIGEST_RE = /^sha256:[0-9a-f]{64}$/;

/**
 * The `imageTag` value that pins a digest: `main-<sha>@sha256:<digest>`.
 *
 * Written into the tag field on purpose. The chart renders
 * `"<image>:<imageTag>"`, and `name:tag@digest` is a valid reference that
 * Kubernetes resolves BY THE DIGEST — so the pin is immutable with no chart
 * change, and the human-readable `main-<sha>` stays in the values file for
 * whoever reads it. Refuses a malformed digest rather than writing one.
 */
export function pinRef(sha: string, digest: string): string {
  if (!/^[0-9a-f]{7,40}$/.test(sha)) throw new Error(`not a sha: ${sha}`);
  if (!DIGEST_RE.test(digest)) throw new Error(`not a sha256 digest: ${digest}`);
  return `main-${sha}@${digest}`;
}

/** `main-<sha>[@sha256:<digest>]` back into its parts; null for anything else. */
export function parsePin(tag: string): { sha: string; digest: string | null } | null {
  const m = /^main-([0-9a-f]{7,40})(?:@(sha256:[0-9a-f]{64}))?$/.exec(tag);
  return m?.[1] ? { sha: m[1], digest: m[2] ?? null } : null;
}

/**
 * Whether `tag` may be written into a values file: a `main-<sha>@sha256:<digest>`
 * pin and nothing else. The CLI is the last thing between the plan and the file,
 * and an empty `--tag` used to write `imageTag: ""` and exit 0 — which the chart
 * renders as `default .Chart.AppVersion`, deploying the chart's own version with
 * no error anywhere. A bare `main-<sha>` is refused too: that is a mutable tag,
 * and this train pins bytes.
 */
export function assertPinnable(tag: string | undefined): string {
  const pin = tag ? parsePin(tag) : null;
  if (!pin?.digest) throw new Error(`refusing to pin '${tag ?? ""}': not main-<sha>@sha256:<digest>`);
  return tag as string;
}

/** The `imageTag` a Helm values file pins, or null when it has none. */
export function readImageTag(yaml: string): string | null {
  return /^\s*imageTag:\s*["']?([^"'\s#]+)["']?/m.exec(yaml)?.[1] ?? null;
}

/**
 * Why prod must not be proposed the candidate's CURRENT digest, or null.
 *
 * The gates judge by sha — `/api/version` reports nothing else — but prod is
 * pinned by digest, read off ghcr when the proposal is written. If `main-<sha>`
 * moved on ghcr after staging was pinned (a re-run, a re-push), those differ, and
 * the sha-level gates would wave through bytes staging never ran. So the digest
 * staging's values file pins is the one prod may have, and a mismatch holds.
 * A staging pin with no digest (a bare tag, from before digest pins) or for
 * another sha has nothing to compare, and does not hold.
 */
export function pinDrift({
  candidateSha,
  candidatePin,
  stagingPin
}: {
  candidateSha: string | null;
  candidatePin: string | null;
  stagingPin: string | null;
}): string | null {
  const staged = stagingPin ? parsePin(stagingPin) : null;
  const fresh = candidatePin ? parsePin(candidatePin) : null;
  if (!candidateSha || !staged?.digest || !fresh?.digest || staged.sha !== candidateSha) return null;
  if (staged.digest === fresh.digest) return null;
  return (
    `main-${candidateSha} moved on ghcr after staging was pinned (staging ran ` +
    `${staged.digest.slice(7, 19)}, ghcr now ${fresh.digest.slice(7, 19)}); not proposing ` +
    "bytes staging never ran — the next main build supersedes it"
  );
}

/**
 * Whether the open bump PR proposes exactly the candidate.
 *
 * The title names the sha; the branch's values file names the bytes. Both have
 * to match, or a PR opened before digest pins (bare `main-<sha>`) or before the
 * tag moved is merged as-is by a human who reads the matching title. An
 * UNREADABLE branch pin (`undefined`) trusts the title: re-proposing on a flaky
 * read force-pushes the branch and clears the verdict the PR was waiting on.
 */
export function proposesCandidate(
  pr: { proposes: string | null; proposesPin?: string | null } | null,
  candidateSha: string | null,
  candidatePin: string | null
): boolean {
  if (!candidateSha || pr?.proposes !== candidateSha) return false;
  if (pr.proposesPin === undefined || !candidatePin) return true;
  return pr.proposesPin === candidatePin;
}

/** `imageTag: "…"` replaced in a Helm values file, preserving everything else. */
export function setImageTag(yaml: string, tag: string): string {
  const line = /^(\s*imageTag:\s*).*$/m;
  if (!line.test(yaml)) throw new Error("no imageTag line in the values file");
  return yaml.replace(line, `$1"${tag}"`);
}

// ── Reads ───────────────────────────────────────────────────────────────────

async function gh<T = unknown>(
  path: string,
  { token, accept = "application/vnd.github+json" }: { token: Token; accept?: string }
): Promise<T> {
  const res = await fetch(`${API}${path}`, {
    headers: {
      accept,
      authorization: `Bearer ${token}`,
      "x-github-api-version": "2022-11-28"
    }
  });
  if (!res.ok) throw new Error(`GET ${path} → ${res.status} ${await res.text()}`);
  return (await res.json()) as T;
}

/**
 * The digest behind `IMAGE:main-<sha>`, or null when the tag does not exist —
 * asked of the registry rather than of a workflow run's conclusion.
 *
 * The digest is the point. A tag on ghcr is mutable, so pinning `main-<sha>`
 * alone pins whatever that name points at when the pod pulls; pinning the digest
 * pins the bytes the gates actually judged.
 *
 * A green `Public Release` is not proof of an image — it reports success even when
 * every build job was skipped — and that mismatch has already shipped an
 * ImagePullBackOff to the single-replica ide StatefulSet once.
 */
async function imageDigest(sha: string, token: Token): Promise<string | null> {
  // Basic, not Bearer: this endpoint exchanges a GitHub credential for a pull
  // token and takes it the way `curl -u x:$GH_TOKEN` sends it, which is what
  // public-release.yaml does for the same call. Sending the base64 under the
  // Bearer scheme is a 401 that looks like "no such image".
  const auth = await fetch(
    "https://ghcr.io/token?service=ghcr.io&scope=repository:oxy-hq/oxygen:pull",
    { headers: { authorization: `Basic ${btoa(`x:${token}`)}` } }
  );
  if (!auth.ok) {
    throw new Error(`ghcr token exchange failed: ${auth.status} ${await auth.text()}`);
  }
  const { token: pull } = (await auth.json()) as { token?: string };
  if (!pull) throw new Error("ghcr token exchange returned no token");

  const res = await fetch(`https://ghcr.io/v2/oxy-hq/oxygen/manifests/main-${sha}`, {
    method: "HEAD",
    headers: {
      authorization: `Bearer ${pull}`,
      accept:
        "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json"
    }
  });
  if (res.status === 200) {
    const digest = res.headers.get("docker-content-digest");
    // A 200 with no digest header is a registry we do not understand, not an
    // image we can pin. Refusing is the direction that cannot pin the wrong bytes.
    if (!digest || !DIGEST_RE.test(digest)) {
      throw new Error(`ghcr answered 200 for main-${sha} without a usable digest (${digest})`);
    }
    return digest;
  }
  if (res.status === 404) return null;
  // Anything else — 401, 429, a 5xx — is "could not ask", and answering "no" to
  // that walks the whole commit list, finds nothing, and reports a green run
  // saying there is no promotable digest. The train would stop and nothing would
  // say so.
  throw new Error(`ghcr manifest HEAD for main-${sha}: unexpected status ${res.status}`);
}

/** The CI conclusion for an INTERNAL commit. `null` when no run exists. */
async function ciConclusion(internalSha: string, token: Token): Promise<string | null> {
  const runs = await gh<{ workflow_runs?: { conclusion: string | null }[] }>(
    `/repos/${INTERNAL}/actions/workflows/ci.yaml/runs?head_sha=${internalSha}&per_page=5`,
    { token }
  );
  const run = runs.workflow_runs?.[0];
  return run?.conclusion ?? null;
}

/** What an environment is actually serving, straight from the running process. */
async function servedSha(baseUrl: string): Promise<Serving | null> {
  try {
    const res = await fetch(`${baseUrl}/api/version`, { signal: AbortSignal.timeout(10_000) });
    if (!res.ok) return null;
    // `git_commit_short` is nested under `build_info`; only `version` is at the
    // top level. Read flat it is always undefined, and undefined reads as "not
    // serving the candidate" — which holds every gate here, silently, forever.
    const body = (await res.json()) as {
      version?: string;
      build_info?: { git_commit_short?: string };
    };
    return { sha: body.build_info?.git_commit_short ?? null, version: body.version ?? null };
  } catch {
    return null;
  }
}

/**
 * The newest mirror commit that has an image and did not fail CI.
 *
 * Walks newest-first and stops at the first hit, so a broken HEAD does not block
 * the commit under it — the train keeps moving at the last good digest rather
 * than stalling until someone notices.
 */
async function newestPromotable(token: Token, limit = 30): Promise<Candidate | null> {
  type ApiCommit = {
    sha: string;
    commit?: { message?: string; committer?: { date?: string } };
  };
  const commits = await gh<ApiCommit[]>(`/repos/${MIRROR}/commits?sha=main&per_page=${limit}`, {
    token
  });
  for (const commit of commits) {
    const sha = commit.sha.slice(0, 7);
    const digest = await imageDigest(sha, token);
    if (!digest) continue;
    const internal = originRevId(commit.commit?.message);
    const ci: string | null = internal ? await ciConclusion(internal, token) : null;
    if (ci !== null && CI_FAILED.has(ci)) continue;
    return {
      sha,
      internal,
      ci,
      subject: commit.commit?.message?.split("\n")[0] ?? "",
      committedAt: commit.commit?.committer?.date ?? null,
      digest
    };
  }
  return null;
}

/**
 * The internal commit behind a mirror sha, via the copybara trailer.
 *
 * Needed because everything an environment reports is a mirror sha, while every
 * question worth asking about the code ("what migrations does this promotion
 * bring?", "what PRs are in it?") is a question about oxygen-internal.
 */
async function internalShaFor(mirrorSha: string | null, token: Token): Promise<string | null> {
  if (!mirrorSha) return null;
  try {
    const commit = await gh<{ commit?: { message?: string } }>(
      `/repos/${MIRROR}/commits/${mirrorSha}`,
      { token }
    );
    return originRevId(commit.commit?.message);
  } catch {
    return null;
  }
}

/** A candidate named by a dispatch, with the same fields a walked one carries. */
async function dispatchedCandidate(sha: string, token: Token): Promise<Candidate> {
  let committedAt = null;
  try {
    const commit = await gh<{ commit?: { committer?: { date?: string } } }>(
      `/repos/${MIRROR}/commits/${sha}`,
      { token }
    );
    committedAt = commit.commit?.committer?.date ?? null;
  } catch {
    /* the grace then reads as expired, which is the permissive direction — a
       human naming a sha has already decided. */
  }
  return {
    sha,
    internal: await internalShaFor(sha, token),
    ci: null,
    subject: "(named by dispatch)",
    committedAt,
    // A named sha still has to exist to be pinned — a rollback to a digest that
    // is not on ghcr is an ImagePullBackOff on the single ide pod, not a rollback.
    digest: await imageDigest(sha, token)
  };
}

/**
 * The `imageTag` a values file in the infra repo pins at `ref`: the tag, null
 * when the file has none, `undefined` when it could not be read at all.
 */
async function pinnedAt(path: string, ref: string, token: Token): Promise<string | null | undefined> {
  try {
    const file = await gh<{ content?: string }>(
      `/repos/${INFRA}/contents/${path}?ref=${encodeURIComponent(ref)}`,
      { token }
    );
    return file.content ? readImageTag(Buffer.from(file.content, "base64").toString("utf8")) : undefined;
  } catch {
    return undefined;
  }
}

interface ReleaseRun {
  conclusion: string | null;
  url: string;
  at: string | null;
  sha: string;
}

/**
 * The mirror's most recent COMPLETED `Public Release` on main. Signing happens
 * there, and a signing failure publishes no `main-<sha>` — which the walk above
 * reads as "no image yet" and quietly skips, so the train sits on the last signed
 * build and nothing here would ever say the build is broken.
 */
async function latestReleaseRun(token: Token): Promise<ReleaseRun | null> {
  try {
    const { workflow_runs: runs } = await gh<{
      workflow_runs: { conclusion: string | null; html_url: string; updated_at?: string; head_sha: string }[];
    }>(
      `/repos/${MIRROR}/actions/workflows/public-release.yaml/runs?branch=main&status=completed&per_page=1`,
      { token }
    );
    const run = runs[0];
    return run
      ? { conclusion: run.conclusion, url: run.html_url, at: run.updated_at ?? null, sha: run.head_sha.slice(0, 7) }
      : null;
  } catch {
    return null;
  }
}

/** Whether this pass should say the mirror's image build is broken. */
export function buildAlert(
  run: ReleaseRun | null,
  { now }: { now: Date }
): { alert: boolean; why: string } {
  if (!run) return { alert: false, why: "no completed release run to judge" };
  if (run.conclusion !== "failure")
    return { alert: false, why: `last release run: ${run.conclusion ?? "unknown"}` };
  const age = run.at ? (now.getTime() - new Date(run.at).getTime()) / 60000 : Infinity;
  if (age > BUILD_ALERT_MINUTES)
    return { alert: false, why: `release run for ${run.sha} failed ${Math.floor(age)}m ago; already said` };
  return {
    alert: true,
    why: `the mirror's Public Release for ${run.sha} failed — no signed main-${run.sha}, so the train cannot move past the last good build: ${run.url}`
  };
}

/** The open prod bump PR's labels and the version it proposes. */
async function bumpPr(
  token: Token,
  { repo, branch }: { repo: string; branch: string }
): Promise<BumpPr | null> {
  const prs = await gh<
    { number: number; title: string; labels: { name: string }[]; head: { ref: string } }[]
  >(
    `/repos/${repo}/pulls?head=${repo.split("/")[0]}:${branch}&state=open`,
    { token }
  );
  const pr = prs[0];
  if (!pr) return null;
  const labels: string[] = pr.labels.map((l: { name: string }) => l.name);
  return {
    number: pr.number,
    proposes: proposedSha(pr.title),
    proposesPin: await pinnedAt(PROD_VALUES, pr.head.ref, token),
    labels,
    held: labels.includes("release-hold"),
    rollbackOpen: labels.includes("release-rollback"),
    checks: labels.includes("release-checks-passed")
      ? "passed"
      : labels.includes("release-checks-failed")
        ? "failed"
        : labels.includes("release-checks-not-run")
          ? "not-run"
          : labels.includes("release-checks-pending")
            ? "pending"
            : null
  };
}

/** What `promote.yaml` writes on the bump PR the moment staging serves a digest. */
export const SOAK_MARKER = (sha: string): string => `soak-start: main-${sha}`;

/** What it writes when it dispatches the staging checks for a digest. */
export const CHECKS_MARKER = (sha: string): string => `checks-dispatched: main-${sha}`;

/** What it writes when it has told a human the bump is theirs to merge. */
export const READY_MARKER = (sha: string): string => `ready-for-merge: main-${sha}`;

/** What it writes when it has raised the alarm about this digest being stuck. */
export const STUCK_MARKER = (sha: string): string => `stuck-alert: main-${sha}`;

/**
 * Whether anybody should be told this digest is not moving.
 *
 * Two shapes, and the distinction is who has to act:
 *
 * * `blocked` — a hold, an open rollback, new Sentry issues, an unmeasured gate.
 *   By construction a person has to do something, so it is said at once.
 * * `waiting` — the ordinary state, and only worth saying when it has gone on
 *   too long. Measured from the soak start, which is when staging first served
 *   the digest; with no soak marker there is nothing to measure and nothing is
 *   claimed.
 *
 * `alertedAt` is the last alert for this digest, so a stuck train says so again
 * every [`STUCK_MINUTES`] rather than once and then never again.
 */
export function stuckAlert(
  state: StuckState,
  { now }: { now: Date }
): { alert: boolean; why: string } {
  const { kind, soakStartedAt, alertedAt, held } = state;
  const since = (t: Date | null | undefined) =>
    t ? (now.getTime() - t.getTime()) / 60000 : Infinity;
  if (since(alertedAt) < STUCK_MINUTES) return { alert: false, why: "already said so" };
  if (held)
    return {
      // Somebody stopped the train on purpose. Saying it once is useful; paging
      // them about their own decision every six hours is how a channel gets
      // muted, and a muted channel loses the alerts that are not decisions.
      alert: false,
      why: "held on purpose; the person who set release-hold already knows"
    };
  if (kind === "blocked") return { alert: true, why: "blocked; a person has to clear it" };
  if (kind !== "waiting") return { alert: false, why: kind ?? "no verdict" };
  if (!soakStartedAt) return { alert: false, why: "not on staging yet; nothing to time" };
  const waited = since(soakStartedAt);
  if (waited < STUCK_MINUTES)
    return {
      alert: false,
      why: `waiting ${Math.floor(waited)}m, under the ${STUCK_MINUTES}m alarm`
    };
  return { alert: true, why: `waiting ${Math.floor(waited)}m without reaching prod` };
}

/**
 * Whether the staging checks should be (re)dispatched for the candidate.
 *
 * Pure, and exported, because the first version of this lived in three `if:`
 * expressions in the workflow — where a self-test cannot see it — and deadlocked:
 * the proposal stamped `release-checks-pending`, this gate refused to dispatch
 * while pending, and the only thing that clears pending is the run that was never
 * dispatched. `pending` now means *dispatched*, which is the whole repair; the
 * staleness escape is what stops a dispatched-but-dead run re-creating the stall.
 */
export function shouldDispatchChecks(
  state: DispatchState,
  { now }: { now: Date }
): { dispatch: boolean; why: string } {
  const { prProposesCandidate, stagingServesCandidate, checks, dispatchedAt } = state;
  if (!prProposesCandidate)
    return { dispatch: false, why: "the bump PR does not propose the candidate yet" };
  if (!stagingServesCandidate)
    return { dispatch: false, why: "staging does not serve the candidate yet" };
  if (checks === "passed" || checks === "failed")
    return { dispatch: false, why: `already ${checks}` };
  if (checks === "pending") {
    // No marker means the label outlived whatever wrote it — dispatch, because a
    // pending nobody dispatched is exactly the stall this function exists for.
    const age = dispatchedAt ? (now.getTime() - dispatchedAt.getTime()) / 60000 : Infinity;
    if (age < STALE_CHECKS_MINUTES)
      return { dispatch: false, why: `a run dispatched ${Math.floor(age)}m ago is in flight` };
    return { dispatch: true, why: "pending with no run reporting; re-dispatching" };
  }
  // null (never asked) or not-run (asked, no verdict).
  return {
    dispatch: true,
    why: checks === "not-run" ? "retrying a no-verdict run" : "not asked yet"
  };
}

/**
 * Which of several identical markers to believe.
 *
 * The two markers want opposite halves, and giving them one rule was a defect:
 *
 * * `soak-start` wants the **earliest** — a re-dispatch must not restart a soak
 *   that is already running.
 * * `checks-dispatched` wants the **latest** — the question it answers is "how
 *   long has the run in flight been in flight", and an earliest-wins read pins
 *   that to the first dispatch forever, so the first re-dispatch is followed by
 *   one on every pass thereafter: a comment on the infra PR every 15 minutes and
 *   a duplicate check run queued behind the healthy one, until something records
 *   a verdict.
 *
 * Exported and self-tested because "one `Math.min` away from being covered" is
 * exactly the shape of the two decisions that have already bitten this branch.
 */
export function pickStamp(stamps: Date[], pick: Pick = "earliest"): Date | null {
  const at = stamps.map((d) => d.getTime());
  if (!at.length) return null;
  return new Date(pick === "latest" ? Math.max(...at) : Math.min(...at));
}

/**
 * When `promote.yaml` last (or first) wrote a marker on the bump PR.
 *
 * The reconciler is stateless between runs, so anything it has to remember —
 * when a soak started, when the checks were dispatched — lives as a comment it
 * posts itself. Not "the first comment that mentions the sha": on a passing run
 * the checks workflow adds a *label* and no comment at all, so matching on a
 * mention made the soak depend on an unrelated Sentry step happening to post,
 * and when it did not, prod sat at `soaked 0m of 30m` forever.
 *
 * Newest-first, because the bump PR is long-lived (edited and force-pushed,
 * never recreated) and accumulates a comment per promotion; oldest-first, one
 * page, and the marker falls off page 1 after a hundred of them. Which of the
 * matches on that page wins is [`pickStamp`]'s question, and the two markers
 * answer it differently.
 */
async function markerAt(
  token: Token,
  {
    repo,
    pr,
    marker,
    pick = "earliest"
  }: { repo: string; pr?: number; marker?: string; pick?: Pick }
): Promise<Date | null> {
  if (!pr || !marker) return null;
  const comments = await gh<{ body?: string; created_at: string }[]>(
    `/repos/${repo}/issues/${pr}/comments?per_page=100&sort=created&direction=desc`,
    { token }
  );
  return pickStamp(
    comments
      .filter((c: { body?: string }) => c.body?.includes(marker))
      .map((c: { created_at: string }) => new Date(c.created_at)),
    pick
  );
}

// ── Plan ────────────────────────────────────────────────────────────────────

async function plan({
  target,
  sha,
  now,
  token,
  infraToken,
  envs,
  sentryNewIssues,
  dispatched
}: {
  target: string;
  sha: string | null;
  now: Date;
  token: Token;
  /**
   * For oxy-hq/infrastructure only — the bump PR, its markers, the values files.
   * A separate token because no one token reads both sides: the workflow's
   * GITHUB_TOKEN cannot see the private infra repo (every scheduled pass died at
   * `GET …/pulls → 404`), and the infra App token is scoped to that repo alone,
   * so it cannot read oxygen-internal's CI status.
   */
  infraToken: Token;
  envs: Record<string, { baseUrl: string }>;
  sentryNewIssues: number;
  dispatched: boolean;
}) {
  // A dispatched sha still gets its internal commit resolved. Without it
  // `candidate.internal` is null and the migration delta on a rollback PR — the
  // field that exists to say whether that rollback is safe — reads "not
  // determined" on exactly the runs that need it.
  const candidate = sha ? await dispatchedCandidate(sha, token) : await newestPromotable(token);

  // dev is in the environment file for its baseUrl only — the custom-app checks
  // never run there. One file names every environment.
  // A missing environment is a broken targets file, not an environment that is
  // simply down — and the difference matters, because `servedSha` answering null
  // reads as "not serving the candidate" and would hold the train on a typo.
  const baseUrl = (name: string): string => {
    const url = envs[name]?.baseUrl;
    if (!url) throw new Error(`no baseUrl for '${name}' in the targets file`);
    return url;
  };
  // An ABSENT dev entry is "there is no first rung", not "the rung is down" —
  // the second holds staging for the full grace on every candidate, and a
  // targets file with no dev key would have paid that forever. Every other
  // environment throws on absence; dev is the one that may legitimately not be
  // configured, so it says which of the two it is.
  const devConfigured = Boolean(envs.dev?.baseUrl);
  const dev = devConfigured ? await servedSha(baseUrl("dev")) : null;
  const staging = await servedSha(baseUrl("staging"));
  const prod = await servedSha(baseUrl("prod"));
  if (prod?.sha) prod.internal = await internalShaFor(prod.sha, token);
  if (staging?.sha) staging.internal = await internalShaFor(staging.sha, token);
  const pr = await bumpPr(infraToken, {
    repo: "oxy-hq/infrastructure",
    branch: "chore/bump-oxy-prod-image"
  });

  const infra = { repo: "oxy-hq/infrastructure", pr: pr?.number };
  const since = candidate
    ? await markerAt(infraToken, { ...infra, marker: SOAK_MARKER(candidate.sha) })
    : null;
  const readyAt = candidate
    ? await markerAt(infraToken, { ...infra, marker: READY_MARKER(candidate.sha) })
    : null;
  const alertedAt = candidate
    ? await markerAt(infraToken, { ...infra, marker: STUCK_MARKER(candidate.sha), pick: "latest" })
    : null;
  const dispatchedAt = candidate
    ? await markerAt(infraToken, {
        ...infra,
        marker: CHECKS_MARKER(candidate.sha),
        // The MOST RECENT dispatch: the age of the oldest one only grows, so
        // earliest-wins re-dispatches on every pass once it has done so once.
        pick: "latest"
      })
    : null;

  const candidatePin = candidate?.digest ? pinRef(candidate.sha, candidate.digest) : null;
  const prProposesCandidate = proposesCandidate(pr, candidate?.sha ?? null, candidatePin);
  const drift = pinDrift({
    candidateSha: candidate?.sha ?? null,
    candidatePin,
    stagingPin: (await pinnedAt(STAGING_VALUES, "main", infraToken)) ?? null
  });
  const build = buildAlert(await latestReleaseRun(token), { now });
  const stagingServesCandidate = Boolean(candidate && staging?.sha === candidate.sha);
  const dispatch = shouldDispatchChecks(
    {
      prProposesCandidate,
      stagingServesCandidate,
      // A verdict on a PR proposing something else is about something else.
      checks: prProposesCandidate ? (pr?.checks ?? null) : null,
      dispatchedAt
    },
    { now }
  );

  const stagingToo = stagingGate(
    {
      candidate: candidate?.sha ?? null,
      stagingSha: staging?.sha ?? null,
      devSha: dev?.sha ?? null,
      // Unreachable is not the same as serving something else: a dev that does
      // not answer may be a dev crash-looping on this very digest.
      // Not configured reads as reachable-and-irrelevant: `devSha` is then null,
      // never equals the candidate, and the gate falls through to the grace —
      // which is the "no rung" behaviour, reached in one pass rather than two
      // hours.
      devReachable: devConfigured ? Boolean(dev) : true,
      devConfigured,
      candidateCommittedAt: candidate?.committedAt ? new Date(candidate.committedAt) : null
    },
    { now }
  );

  const gate = prodGate(
    {
      candidate: candidate?.sha ?? null,
      prodSha: prod?.sha ?? null,
      stagingSha: staging?.sha ?? null,
      checks: prProposesCandidate ? pr?.checks : null,
      servedSince: since,
      held: pr?.held ?? false,
      rollbackOpen: pr?.rollbackOpen ?? false,
      // The workflow holds the Sentry token, so it measures this and feeds it
      // back on a second `--plan` pass. `--sentry-new-issues -1` means "not
      // measured" and blocks: a gate that passes on an unmeasured signal is a
      // gate nobody is running.
      sentryNewIssues,
      drift
    },
    { now, dispatched }
  );

  return {
    at: now.toISOString(),
    candidate,
    serving: { dev, staging, prod },
    bumpPr: pr,
    // What `--set-image-tag` writes. Null when there is no candidate or no digest,
    // and the workflow refuses to pin rather than fall back to a bare tag.
    candidatePin,
    // The bump PR proposes the candidate — sha AND digest — so a verdict recorded
    // on it is about the bytes under consideration and not the ones before them.
    prProposesCandidate,
    pinDrift: drift,
    buildBroken: build.alert,
    buildWhy: build.why,
    stagingServesCandidate,
    soakStartedAt: since ? since.toISOString() : null,
    checksDispatchedAt: dispatchedAt ? dispatchedAt.toISOString() : null,
    dispatchChecks: dispatch.dispatch,
    dispatchChecksWhy: dispatch.why,
    // Every gate passed and nobody has been told yet. Under a human merge this
    // is the whole handoff: a gate a person has to notice is a gate that waits
    // for somebody to go looking.
    notifyReady: gate.promote && !readyAt,
    stuck: stuckAlert(
      { kind: gate.kind, soakStartedAt: since, alertedAt, held: pr?.held ?? false },
      { now }
    ),
    // Once, when staging first has it. `soakStartedAt` is the "not yet".
    markSoak: prProposesCandidate && stagingServesCandidate && !since,
    stagingNeedsPromote: stagingToo.promote,
    stagingGateWhy: stagingToo.why,
    // The PR has to exist and name the candidate before checks can record a
    // verdict on it, so opening it is a separate act from merging it.
    // Not when prod already serves the sha (a "main-C → main-C" PR is a repin of
    // the running build, which `prodGate` would call current forever), and not
    // while the digest has drifted from the one staging ran.
    prodNeedsProposal: Boolean(
      candidate &&
        !prProposesCandidate &&
        !pr?.rollbackOpen &&
        prod?.sha !== candidate.sha &&
        !drift
    ),
    prod: gate,
    target
  };
}

// ── Self-test ───────────────────────────────────────────────────────────────

function selfTest(): void {
  const fails = [];
  const is = (what: string, got: unknown, want: unknown) => {
    const g = JSON.stringify(got);
    const w = JSON.stringify(want);
    if (g !== w) fails.push(`${what}\n    got  ${g}\n    want ${w}`);
  };

  // The window. A Wednesday 10:00 in Ho Chi Minh is 03:00Z.
  is("Wed 10:00 local is in the window", inWindow(new Date("2026-09-23T03:00:00Z")), true);
  is("Wed 08:00 local is too early", inWindow(new Date("2026-09-23T01:00:00Z")), false);
  is("Wed 17:00 local is past it", inWindow(new Date("2026-09-23T10:00:00Z")), false);
  is("Friday is not a promote day", inWindow(new Date("2026-09-25T03:00:00Z")), false);
  is("Sunday is not either", inWindow(new Date("2026-09-27T03:00:00Z")), false);

  const now = new Date("2026-09-23T03:00:00Z");
  const soaked = new Date(now.getTime() - 45 * 60000);
  const green: ProdState = {
    candidate: "abc1234",
    prodSha: "999aaaa",
    stagingSha: "abc1234",
    checks: "passed",
    servedSince: soaked,
    held: false,
    rollbackOpen: false,
    sentryNewIssues: 0
  };

  is("every gate passed", prodGate(green, { now }).promote, true);
  is("prod already has it", prodGate({ ...green, prodSha: "abc1234" }, { now }).kind, "current");
  is(
    "a hold blocks, and says a person clears it",
    prodGate({ ...green, held: true }, { now }).kind,
    "blocked"
  );
  is(
    "an open rollback outranks a hold",
    prodGate({ ...green, held: true, rollbackOpen: true }, { now }).why,
    "a rollback PR is open; merge or close it, then promote by hand"
  );
  is(
    "staging behind the candidate waits",
    prodGate({ ...green, stagingSha: "older00" }, { now }).kind,
    "waiting"
  );
  is(
    "checks that have not reported wait",
    prodGate({ ...green, checks: null }, { now }).kind,
    "waiting"
  );
  is(
    "an unsoaked digest waits, and says how far it got",
    prodGate({ ...green, servedSince: new Date(now.getTime() - 5 * 60000) }, { now }).why,
    "soaked 5m of 30m on staging"
  );
  is(
    "a new Sentry issue blocks",
    prodGate({ ...green, sentryNewIssues: 2 }, { now }).kind,
    "blocked"
  );
  is(
    "out of hours waits",
    prodGate(green, { now: new Date("2026-09-23T14:00:00Z") }).kind,
    "waiting"
  );
  is(
    "a dispatch ignores the window",
    prodGate(green, { now: new Date("2026-09-23T14:00:00Z"), dispatched: true }).promote,
    true
  );
  is("no candidate is not an error", prodGate({ candidate: null }, { now }).kind, "waiting");
  is(
    "an unmeasured Sentry count blocks rather than passing",
    prodGate({ ...green, sentryNewIssues: -1 }, { now }).kind,
    "blocked"
  );

  // The mirror → internal mapping.
  is(
    "the origin trailer is read",
    originRevId(
      "feat: a thing (#123)\n\nCo-authored-by: x\nGitOrigin-RevId: a9413263968ff6f8ca96f87bb05f445828e328e1"
    ),
    "a9413263968ff6f8ca96f87bb05f445828e328e1"
  );
  is("a commit with no trailer maps to nothing", originRevId("feat: a thing"), null);

  // The dispatch gate. These exist because the first version of this decision
  // lived in the workflow's `if:` expressions, deadlocked, and no test could see
  // it: the proposal stamped `pending`, the gate refused to dispatch while
  // pending, and only the undispatched run could clear pending.
  const dispatching: DispatchState = {
    prProposesCandidate: true,
    stagingServesCandidate: true,
    checks: null,
    dispatchedAt: null
  };
  is("never asked, so ask", shouldDispatchChecks(dispatching, { now }).dispatch, true);
  is(
    "THE DEADLOCK: pending with no dispatch marker must re-dispatch, not wait",
    shouldDispatchChecks({ ...dispatching, checks: "pending" }, { now }).dispatch,
    true
  );
  is(
    "pending with a fresh dispatch is a run in flight, and is left alone",
    shouldDispatchChecks(
      { ...dispatching, checks: "pending", dispatchedAt: new Date(now.getTime() - 10 * 60000) },
      { now }
    ).dispatch,
    false
  );
  is(
    "pending past the stale window is re-dispatched — a dispatched run can die",
    shouldDispatchChecks(
      { ...dispatching, checks: "pending", dispatchedAt: new Date(now.getTime() - 200 * 60000) },
      { now }
    ).dispatch,
    true
  );
  is(
    "a no-verdict run is retried",
    shouldDispatchChecks({ ...dispatching, checks: "not-run" }, { now }).dispatch,
    true
  );
  for (const terminal of ["passed", "failed"] as const) {
    is(
      `${terminal} is terminal; asking again would overwrite it`,
      shouldDispatchChecks({ ...dispatching, checks: terminal }, { now }).dispatch,
      false
    );
  }
  is(
    "a PR proposing something else is not asked about this candidate",
    shouldDispatchChecks({ ...dispatching, prProposesCandidate: false }, { now }).dispatch,
    false
  );
  is(
    "staging behind the candidate is not asked — the checks confirm, they do not wait",
    shouldDispatchChecks({ ...dispatching, stagingServesCandidate: false }, { now }).dispatch,
    false
  );
  is(
    "the dispatch marker names the ref as the values file spells it",
    CHECKS_MARKER("abc1234"),
    "checks-dispatched: main-abc1234"
  );

  // dev as the first rung. It exists to catch "this image does not boot" one
  // environment earlier, and to stop holding when dev itself is the problem.
  const fresh = new Date(now.getTime() - 20 * 60000);
  const stale = new Date(now.getTime() - 300 * 60000);
  const staging0: StagingState = {
    candidate: "abc1234",
    stagingSha: "999aaaa",
    devSha: "abc1234",
    devReachable: true,
    candidateCommittedAt: fresh
  };
  is("dev has it, so staging may have it", stagingGate(staging0, { now }).promote, true);
  is(
    "staging already has it",
    stagingGate({ ...staging0, stagingSha: "abc1234" }, { now }).why,
    "staging serves it"
  );
  is(
    "dev has not got there yet — wait, that is the point of the rung",
    stagingGate({ ...staging0, devSha: "999aaaa" }, { now }).promote,
    false
  );
  is(
    "dev not answering holds staging: it may be crash-looping on this digest",
    stagingGate({ ...staging0, devSha: null, devReachable: false }, { now }).why,
    "dev is not answering; holding staging until it does"
  );
  is(
    "but a broken dev does not hold the train forever",
    stagingGate(
      { ...staging0, devSha: null, devReachable: false, candidateCommittedAt: stale },
      { now }
    ).promote,
    true
  );
  is(
    "and neither does a dev simply stuck on an older digest",
    stagingGate({ ...staging0, devSha: "999aaaa", candidateCommittedAt: stale }, { now }).promote,
    true
  );
  is(
    "a candidate with no commit date is not held on a rung it cannot be timed against",
    stagingGate({ ...staging0, devSha: "999aaaa", candidateCommittedAt: null }, { now }).promote,
    true
  );
  is("no candidate, nothing to pin", stagingGate({ candidate: null }, { now }).promote, false);
  is(
    "no dev environment configured is 'no rung', not 'rung is down' — not a 2h wait",
    stagingGate({ ...staging0, devSha: null, devConfigured: false }, { now }).why,
    "no dev environment configured; no rung to wait on"
  );

  // Telling somebody. The reconciler's own run summary is not telling somebody.
  const stuckBase: StuckState = {
    kind: "waiting",
    soakStartedAt: new Date(now.getTime() - 30 * 60000),
    alertedAt: null
  };
  is("an ordinary wait says nothing", stuckAlert(stuckBase, { now }).alert, false);
  is(
    "blocked is said at once — by construction a person has to act",
    stuckAlert({ ...stuckBase, kind: "blocked" }, { now }).alert,
    true
  );
  is(
    "waiting past the alarm is said",
    stuckAlert({ ...stuckBase, soakStartedAt: new Date(now.getTime() - 400 * 60000) }, { now })
      .alert,
    true
  );
  is(
    "having already said it, it is not said again straight away",
    stuckAlert(
      {
        ...stuckBase,
        soakStartedAt: new Date(now.getTime() - 400 * 60000),
        alertedAt: new Date(now.getTime() - 10 * 60000)
      },
      { now }
    ).alert,
    false
  );
  is(
    "but a still-stuck train says so again once the window passes",
    stuckAlert(
      {
        ...stuckBase,
        soakStartedAt: new Date(now.getTime() - 800 * 60000),
        alertedAt: new Date(now.getTime() - 400 * 60000)
      },
      { now }
    ).alert,
    true
  );
  is(
    "a digest that never reached staging is not called stuck",
    stuckAlert({ ...stuckBase, soakStartedAt: null }, { now }).alert,
    false
  );
  is(
    "prod already has it, so there is nothing to be stuck on",
    stuckAlert({ ...stuckBase, kind: "current" }, { now }).alert,
    false
  );
  is(
    "a deliberate hold does not page the person who set it",
    stuckAlert({ ...stuckBase, kind: "blocked", held: true }, { now }).alert,
    false
  );
  is(
    "...but anything else blocked still does",
    stuckAlert({ ...stuckBase, kind: "blocked", held: false }, { now }).alert,
    true
  );
  is("the ready marker names the ref", READY_MARKER("abc1234"), "ready-for-merge: main-abc1234");
  is("the stuck marker names the ref", STUCK_MARKER("abc1234"), "stuck-alert: main-abc1234");

  // The two markers want opposite ends of the same list.
  const early = new Date("2026-09-23T01:00:00Z");
  const late = new Date("2026-09-23T02:30:00Z");
  is(
    "a soak keeps its first start — a re-dispatch must not restart it",
    pickStamp([late, early], "earliest")?.toISOString(),
    early.toISOString()
  );
  is(
    "a dispatch is the LATEST one — otherwise the first re-dispatch repeats forever",
    pickStamp([early, late], "latest")?.toISOString(),
    late.toISOString()
  );
  is("no markers is not a date", pickStamp([], "latest"), null);
  is("the default is earliest", pickStamp([late, early])?.toISOString(), early.toISOString());

  // What a bump PR proposes. This is the decision the self-test did not cover,
  // and the one that silently stopped prod from ever promoting.
  is(
    "a train PR's title yields a BARE sha, matching what a candidate carries",
    proposedSha("chore(oxy-prod): bump oxy image main-999aaaa → main-abc1234"),
    "abc1234"
  );
  is(
    "a rollback title parses the same way",
    proposedSha("chore(oxy-prod): roll back oxy image main-abc1234 → main-999aaaa"),
    "999aaaa"
  );
  is(
    "a semver title proposes no sha rather than a fake one",
    proposedSha("chore(oxy-prod): bump oxy image 0.5.149 → 0.5.150"),
    null
  );
  is("a title with no arrow proposes nothing", proposedSha("chore: something else"), null);
  is("no title at all is not a crash", proposedSha(undefined), null);

  // "Not measured" has to survive every shape the flag can arrive in.
  is("an absent flag is not measured", measured(null), -1);
  is("an empty value is NOT measured-clean", measured(""), -1);
  is("a non-number is not measured", measured("none"), -1);
  is("zero is a real measurement", measured("0"), 0);
  is("a count is a count", measured("3"), 3);

  // The soak marker is a contract between this file and promote.yaml.
  is(
    "the soak marker names the ref as the values file spells it",
    SOAK_MARKER("abc1234"),
    "soak-start: main-abc1234"
  );

  // The digest pin.
  const d = `sha256:${"a".repeat(64)}`;
  is("a pin carries the readable tag and the immutable digest", pinRef("abc1234", d), `main-abc1234@${d}`);
  is("a pin parses back into its parts", parsePin(`main-abc1234@${d}`), { sha: "abc1234", digest: d });
  is("a bare main- tag still parses, with no digest", parsePin("main-abc1234"), { sha: "abc1234", digest: null });
  is("a semver pin is not a main- pin", parsePin("0.5.151"), null);
  is("an edge- tag is not a main- pin", parsePin("edge-abc1234"), null);
  for (const [what, sha, dig] of [
    ["a short digest", "abc1234", "sha256:abc"],
    ["a non-sha256 digest", "abc1234", `sha512:${"a".repeat(64)}`],
    ["a non-hex sha", "xyz1234", d]
  ] as const) {
    try {
      pinRef(sha, dig);
      fails.push(`pinRef must refuse ${what}, not write it into a values file`);
    } catch {
      /* expected */
    }
  }
  is(
    "set-image-tag writes the whole pin, digest included",
    setImageTag('app:\n  imageTag: "main-000aaaa"\n', `main-abc1234@${d}`),
    `app:\n  imageTag: "main-abc1234@${d}"\n`
  );

  // What the CLI will write: a digest pin, or nothing.
  is("a digest pin is pinnable", assertPinnable(`main-abc1234@${d}`), `main-abc1234@${d}`);
  for (const [what, tag] of [
    ["an empty tag — the chart would deploy its own AppVersion", ""],
    ["a missing tag", undefined],
    ["a bare, mutable main- tag", "main-abc1234"],
    ["a semver tag", "0.5.151"]
  ] as const) {
    try {
      assertPinnable(tag);
      fails.push(`--set-image-tag must refuse ${what}`);
    } catch {
      /* expected */
    }
  }

  // Reading a pin back out of a values file.
  is("the pinned tag is read, quotes stripped", readImageTag(`app:\n  imageTag: "main-abc1234@${d}"\n`), `main-abc1234@${d}`);
  is("an unquoted tag reads too", readImageTag("app:\n  imageTag: 0.5.151 # pinned\n"), "0.5.151");
  is("a commented-out imageTag is not the pin", readImageTag('# imageTag: "main-000aaaa"\napp:\n  image: x\n'), null);
  is("a title carrying a digest still names the bare sha", proposedSha(`chore(oxy-prod): bump oxy image main-999aaaa → main-abc1234@${d}`), "abc1234");
  is("an elided digest in a title still names the sha", proposedSha("… → main-abc1234@sha256:…"), "abc1234");
  is("a title naming main-<not a sha> proposes nothing", proposedSha("chore(oxy-prod): bump oxy image main-999aaaa → main-latest"), null);

  // The digest prod gets is the digest staging ran.
  const d2 = `sha256:${"b".repeat(64)}`;
  is(
    "staging ran the digest ghcr still has: no drift",
    pinDrift({ candidateSha: "abc1234", candidatePin: `main-abc1234@${d}`, stagingPin: `main-abc1234@${d}` }),
    null
  );
  is(
    "the tag moved after staging was pinned: drift, and it says so",
    typeof pinDrift({ candidateSha: "abc1234", candidatePin: `main-abc1234@${d2}`, stagingPin: `main-abc1234@${d}` }),
    "string"
  );
  is(
    "staging pinned by bare tag (before digest pins): nothing to compare, no drift",
    pinDrift({ candidateSha: "abc1234", candidatePin: `main-abc1234@${d2}`, stagingPin: "main-abc1234" }),
    null
  );
  is(
    "staging pins another sha: not this candidate's drift",
    pinDrift({ candidateSha: "abc1234", candidatePin: `main-abc1234@${d2}`, stagingPin: `main-999aaaa@${d}` }),
    null
  );
  is(
    "drift blocks prod — it is not a wait that clears itself",
    prodGate(
      { candidate: "abc1234", prodSha: "999aaaa", stagingSha: "abc1234", checks: "passed", drift: "moved" },
      { now: new Date("2026-09-24T03:00:00Z") }
    ).kind,
    "blocked"
  );

  // The PR proposes the candidate only if its branch pins the candidate's bytes.
  const pin = `main-abc1234@${d}`;
  is("title and branch pin both match", proposesCandidate({ proposes: "abc1234", proposesPin: pin }, "abc1234", pin), true);
  is(
    "a pre-digest PR (bare tag on the branch) is re-proposed, not merged as-is",
    proposesCandidate({ proposes: "abc1234", proposesPin: "main-abc1234" }, "abc1234", pin),
    false
  );
  is(
    "the branch pins another digest of the same sha: re-proposed",
    proposesCandidate({ proposes: "abc1234", proposesPin: `main-abc1234@${d2}` }, "abc1234", pin),
    false
  );
  is(
    "an unreadable branch pin trusts the title — a flaky read must not wipe the verdict",
    proposesCandidate({ proposes: "abc1234", proposesPin: undefined }, "abc1234", pin),
    true
  );
  is("a title for another sha is not the candidate", proposesCandidate({ proposes: "999aaaa", proposesPin: pin }, "abc1234", pin), false);
  is("no PR proposes nothing", proposesCandidate(null, "abc1234", pin), false);

  // A broken mirror build is said out loud, a bounded number of times.
  const t0 = new Date("2026-09-24T03:00:00Z");
  const run = (conclusion: string | null, minutesAgo: number) => ({
    conclusion,
    url: "https://github.com/oxy-hq/oxygen/actions/runs/1",
    at: new Date(t0.getTime() - minutesAgo * 60000).toISOString(),
    sha: "abc1234"
  });
  is("a release run that just failed is said", buildAlert(run("failure", 10), { now: t0 }).alert, true);
  is("...and not again once the window has passed", buildAlert(run("failure", 45), { now: t0 }).alert, false);
  is("a green release run is quiet", buildAlert(run("success", 5), { now: t0 }).alert, false);
  is("a cancelled run is not a broken build", buildAlert(run("cancelled", 5), { now: t0 }).alert, false);
  is("no run at all is not an alarm", buildAlert(null, { now: t0 }).alert, false);

  // The values-file edit.
  is(
    "the tag is replaced and the indent kept",
    setImageTag('app:\n  image: x\n  imageTag: "0.5.149"\n  replicas: 2\n', "main-abc1234"),
    'app:\n  image: x\n  imageTag: "main-abc1234"\n  replicas: 2\n'
  );
  is(
    "an unquoted tag is quoted on the way out",
    setImageTag("app:\n  imageTag: edge-ade5171\n", "main-abc1234"),
    'app:\n  imageTag: "main-abc1234"\n'
  );
  try {
    setImageTag("app:\n  image: x\n", "main-abc1234");
    fails.push("a values file with no imageTag must throw, not silently no-op");
  } catch {
    /* expected */
  }

  // ── The one invariant that spans both files ───────────────────────────────
  //
  // Round three established that every EFFECTING step in promote.yaml carries an
  // `inputs.target` clause, and round five found two new ones without it — because
  // the invariant lived in a review comment and a one-off grep. Derived, not
  // listed: a step that writes is one whose `run:` reaches for a write verb, so a
  // step added tomorrow is covered the day it lands.
  //
  // A step satisfies it either by naming `inputs.target` itself, or by gating on
  // `steps.merge.outputs.merged` — the merge already carried the clause, so
  // anything downstream of it inherits the scoping.
  const workflow = readFileSync(new URL("../workflows/promote.yaml", import.meta.url), "utf8");
  const WRITES = [
    "gh pr comment",
    "gh pr edit",
    "gh pr create",
    "gh pr merge",
    "gh workflow run",
    "git push"
  ];
  // A write verb inside an `echo` or a `::error::` is prose, not a write — the
  // rollback refusal names the pre-cutover command in its own message, and read
  // naively that made it look like the thing it is telling you to run.
  const commands = (chunk: string) =>
    chunk
      .split("\n")
      .filter((line) => !/^\s*(echo|printf)\b/.test(line) && !line.includes("::"))
      .join("\n");
  const steps = workflow
    .split(/\n {6}- name: /)
    .slice(1)
    .map((chunk) => ({ name: chunk.split("\n")[0]?.trim() ?? "?", chunk }));
  const effects = steps.filter(({ chunk }) =>
    WRITES.some((verb) => commands(chunk).includes(verb))
  );
  const unscoped = effects
    .filter(
      ({ chunk }) =>
        !chunk.includes("inputs.target") && !chunk.includes("steps.merge.outputs.merged")
    )
    .map(({ name }) => name);
  is(
    `every effecting step in promote.yaml is scoped by target (unscoped: ${unscoped.join(", ") || "none"})`,
    unscoped,
    []
  );
  // The derivation is only worth anything if it can see a step at all.
  is(
    `...and it found effecting steps to check, rather than matching nothing (${effects.length})`,
    effects.length >= 5,
    true
  );

  // The plan reaches the workflow through `jq … plan.json >> $GITHUB_OUTPUT`.
  // Written as a `{ jq; jq; … } < plan.json` group, the first jq drained the
  // shared stdin and every later one printed nothing — only the first output was
  // ever set, so every gate after it read empty and nothing was ever pinned. Two
  // such groups shipped, behind `|| true`. Each read has to open the file itself.
  const drained = steps
    .filter(({ chunk }) => /\}\s*<\s*plan2?\.json/.test(chunk) || /plan2?\.json[^\n]*\|\|\s*true/.test(chunk))
    .map(({ name }) => name);
  is(
    `no step feeds the plan to several jq processes through one stdin, or hides a failed read (${drained.join(", ") || "none"})`,
    drained,
    []
  );
  // Every plan reads the private infra repo — the bump PR, its markers, the
  // values files — and the workflow's GITHUB_TOKEN cannot see it. The first
  // scheduled passes after this merged all died at `GET …/pulls → 404`, because
  // locally the plan had only ever run with a personal token that reads both.
  const planSteps = steps.filter(({ chunk }) => chunk.includes("args=(--plan"));
  const tokenless = planSteps
    .filter(({ chunk }) => !chunk.includes("INFRA_TOKEN: ${{ steps.infra-token.outputs.token }}"))
    .map(({ name }) => name);
  is(
    `every --plan step passes the infra token (missing: ${tokenless.join(", ") || "none"})`,
    tokenless,
    []
  );
  is(`...and all three plan steps are found (${planSteps.length})`, planSteps.length, 3);
  // The plan reads the values files the workflow writes. Two spellings of one
  // path drift apart silently: the drift check would read a file nobody pins.
  is(
    "promote.yaml writes the prod values file this script reads",
    workflow.includes(`PROD_VALUES: ${PROD_VALUES}\n`),
    true
  );
  is(
    "promote.yaml writes the staging values file this script reads",
    workflow.includes(`STAGING_VALUES: ${STAGING_VALUES}\n`),
    true
  );
  const reads = [...workflow.matchAll(/' (plan2?\.json) >> "\$GITHUB_OUTPUT"/g)].length;
  is(`...and both plan reads are found, rather than matching nothing (${reads})`, reads, 2);

  if (fails.length) {
    console.error(`promote.ts self-test: ${fails.length} failure(s)\n\n  ${fails.join("\n  ")}\n`);
    process.exit(1);
  }
  console.log("promote.ts self-test: all gate, mapping and values-file cases pass");
}

// ── Entry ───────────────────────────────────────────────────────────────────

/** A count, or -1 when the caller did not produce one. */
export function measured(raw: string | null): number {
  const n = Number(raw);
  return raw === null || raw === "" || !Number.isFinite(n) ? -1 : n;
}

/**
 * The value after `--name`, or `fallback`.
 *
 * Returns a STRING or the fallback, never `true`: a valueless `--sha` used to
 * arrive as a boolean and be carried all the way to "pin this digest", which the
 * compiler now refuses. A flag with no value is a flag the caller got wrong.
 */
function arg(name: string): string | null;
function arg(name: string, fallback: string): string;
function arg(name: string, fallback: string | null = null): string | null {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return fallback;
  const value = process.argv[i + 1];
  return value === undefined || value.startsWith("--") ? fallback : value;
}

// Only when run as a command. Without this, importing the module to exercise one
// of its exported decisions runs the CLI block and exits — which is a module that
// cannot be poked at, and the decisions in here are exactly the ones worth poking.
// Compared through realpath: invoked via a symlinked path (macOS `/tmp` is
// `/private/tmp`), the URLs differ and `--self-test` exited 0 having run nothing.
const invokedDirectly =
  process.argv[1] &&
  import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href;

if (!invokedDirectly) {
  // imported: exports only
} else if (process.argv.includes("--self-test")) {
  selfTest();
} else if (process.argv.includes("--set-image-tag")) {
  // Two callers (staging and the prod bump) and one tested implementation. An
  // inline `node -e` in the workflow was a second copy of the regex that no test
  // covered, which is the way a values file silently stops being rewritten.
  const file = arg("set-image-tag");
  const tag = arg("tag");
  if (typeof file !== "string" || typeof tag !== "string") {
    console.error("usage: promote.ts --set-image-tag <values.yaml> --tag <imageTag>");
    process.exit(1);
  }
  const before = await readFile(file, "utf8");
  await writeFile(file, setImageTag(before, assertPinnable(tag)));
  console.log(`${file}: imageTag -> ${tag}`);
} else if (process.argv.includes("--plan")) {
  const token = process.env.GH_TOKEN;
  if (!token) {
    console.error("GH_TOKEN is required for --plan");
    process.exit(1);
  }
  // A person running --plan with their own token can read both repos with it;
  // the workflow cannot, and the self-test holds every --plan step to passing one.
  const infraToken = process.env.INFRA_TOKEN || token;
  const envsPath = arg("envs", ".github/custom-app-checks.json");
  const envs = JSON.parse(await readFile(envsPath, "utf8"));
  const result = await plan({
    target: arg("target", "all"),
    sha: arg("sha") || null,
    now: arg("now") ? new Date(arg("now") as string) : new Date(),
    token,
    infraToken,
    envs,
    // -1 is "not measured", and anything that is not a number has to land there
    // too: `Number("")` is 0, so a flag passed with an empty value would have read
    // as "measured, clean" — the one answer an unmeasured gate must never give.
    sentryNewIssues: measured(arg("sentry-new-issues")),
    dispatched: process.argv.includes("--dispatched")
  });
  const json = JSON.stringify(result, null, 2);
  const out = arg("out");
  if (out) await writeFile(out, json);
  console.log(json);
} else {
  console.error("usage: promote.ts --plan [--target …] [--sha …] | --self-test");
  process.exit(1);
}
