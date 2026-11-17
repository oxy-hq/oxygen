// One review round: run the passes, verify, filter, and fold the result into the state.

import * as config from "../config.ts";
import { discussion, threadReplies } from "./feedback.ts";
import type { Dropped } from "./findings.ts";
import { applyFilters, dropKnown, verdictFor } from "./findings.ts";
import type { Loaded } from "./load.ts";
import type { Scheduled, Verified } from "./passes.ts";
import { applyVerification, Budget, collectCandidates, schedulePasses } from "./passes.ts";
import { contextBlock, generalPrompt, lensPrompt, recheckPrompt, verifyPrompt } from "./prompts.ts";
import type { SummaryView } from "./render.ts";
import { judgementOf, roadmapLines } from "./roadmap.ts";
import { FINDINGS_SCHEMA, GENERAL_SCHEMA, RECHECK_SCHEMA, VERIFY_SCHEMA } from "./schemas.ts";
import { addFindings, applyRecheck, isClosed, openFindings } from "./state.ts";
import type {
  Candidate,
  Finding,
  GeneralOutput,
  IssueComment,
  LensPick,
  PassResult,
  PassSpec,
  RecheckOutput,
  ReviewState,
  ReviewStatus,
  RoadmapJudgement,
  Round,
  RunPass,
  Skipped,
  VerifyOutput,
  VerifyVerdict
} from "./types.ts";

/** What a round reads; the comments are optional so a test can leave them out. */
export type RoundInput = Pick<
  Loaded,
  "meta" | "head" | "mergeBase" | "plan" | "prev" | "reviewComments" | "since"
> & {
  comments?: IssueComment[];
  /** internal-docs/platform-roadmap.md at the checkout, for the general pass's roadmap answer. */
  roadmapDoc?: string | null;
};

export interface Debug extends Verified {
  candidates: Candidate[];
  dropped: Dropped[];
  known: Dropped[];
}

export interface RoundOutcome {
  state: ReviewState;
  view: SummaryView;
  added: Finding[];
  closedNow: Finding[];
  results?: PassResult[];
  debug?: Debug;
}

interface Prompts {
  general: () => string;
  lens: (lens: LensPick) => string;
  recheck: () => string;
  verify: (candidates: readonly Candidate[]) => string;
}

interface Exec {
  runPass: RunPass;
  cwd: string;
  graph: boolean;
}

interface VerifyRun {
  verdicts: VerifyVerdict[] | null;
  result: PassResult<VerifyOutput> | null;
  skipped?: string | null;
}

interface Sifted {
  findings: Finding[];
  notReviewed: string[];
  kept: Candidate[];
  possible: Candidate[];
  verify: VerifyRun;
  debug: Debug;
}

const cents = (n: number): number => Math.round(n * 100) / 100;

function passSpec(item: Scheduled, prompts: Prompts): PassSpec {
  if (item.kind === "general")
    return {
      name: "general",
      prompt: prompts.general(),
      schema: GENERAL_SCHEMA,
      effort: config.EFFORT.general,
      taskTokens: config.TASK_TOKENS.general
    };
  if (item.kind === "recheck")
    return {
      name: "recheck",
      prompt: prompts.recheck(),
      schema: RECHECK_SCHEMA,
      effort: config.EFFORT.recheck,
      taskTokens: config.TASK_TOKENS.recheck
    };
  return {
    name: item.lens.id,
    prompt: prompts.lens(item.lens),
    schema: FINDINGS_SCHEMA,
    effort: config.EFFORT.lens,
    taskTokens: config.TASK_TOKENS.lens
  };
}

interface Gate {
  opened: Promise<void>;
  open: () => void;
  /** Start the timer that opens it anyway: a pass that is slow to answer holds no one long. */
  arm: () => void;
  close: () => void;
}

/** A gate that opens once: on `open()`, or `ms` after `arm()`. `close` stops its timer. */
export function gate(ms: number): Gate {
  let open = () => {};
  const opened = new Promise<void>((resolve) => {
    open = resolve;
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  return {
    opened,
    open,
    arm: () => {
      timer ??= setTimeout(open, ms);
    },
    close: () => clearTimeout(timer)
  };
}

/**
 * Which pass each pass waits for before it starts: the first pass of its kind. A request
 * reads a cached prefix only once an earlier request's response has started, so passes
 * started together each paid to write the same prefix. Only passes of one kind can share
 * one: a pass's output schema is a tool definition, and tools open the prompt, so the
 * general, lens, recheck and verify passes differ from the first token (measured on
 * #4168: lens and verify first calls each wrote ~30K tokens and read none). In practice
 * this means a round's second and third lens wait for its first.
 */
export function waitsFor<T>(run: readonly T[], kindOf: (item: T) => string): Map<T, T> {
  const out = new Map<T, T>();
  const leaders = new Map<string, T>();
  for (const item of run) {
    const leader = leaders.get(kindOf(item));
    if (leader === undefined) leaders.set(kindOf(item), item);
    else out.set(item, leader);
  }
  return out;
}

/** Run the scheduled passes, each starting once the pass it waits for has begun answering. */
async function runScheduled(
  run: readonly Scheduled[],
  budget: Budget,
  prompts: Prompts,
  exec: Exec
): Promise<PassResult[]> {
  const items = run.map((item) => ({ item, spec: passSpec(item, prompts) }));
  // The schema follows the kind; effort is in the key in case two kinds ever share one.
  const waits = waitsFor(items, (i) => `${i.item.kind}:${i.spec.effort}`);
  const gates = new Map(
    [...new Set(waits.values())].map((i) => [i, gate(config.STAGGER_MAX_WAIT_MS)])
  );
  try {
    return await Promise.all(
      items.map(async (entry) => {
        const before = waits.get(entry);
        if (before) await gates.get(before)?.opened;
        const mine = gates.get(entry);
        mine?.arm();
        try {
          const result = await exec.runPass<unknown>({
            ...entry.spec,
            ...(mine ? { onFirstResponse: mine.open } : {}),
            model: config.MODELS.finder,
            budgetUsd: entry.item.cap,
            cwd: exec.cwd,
            graph: exec.graph
          });
          budget.settle(entry.item.cap, result.cost);
          return result;
        } finally {
          // A pass that ended without ever answering must not hold the ones behind it.
          mine?.open();
        }
      })
    );
  } finally {
    for (const g of gates.values()) g.close();
  }
}

async function verify(
  candidates: readonly Candidate[],
  budget: Budget,
  prompts: Prompts,
  exec: Exec
): Promise<VerifyRun> {
  if (!candidates.length) return { verdicts: [], result: null };
  if (!budget.tryReserve(config.BUDGET.verify))
    return { verdicts: null, result: null, skipped: "no budget left for the verifier" };
  const result = await exec.runPass<VerifyOutput>({
    name: "verify",
    prompt: prompts.verify(candidates),
    schema: VERIFY_SCHEMA,
    effort: config.EFFORT.verify,
    taskTokens: config.TASK_TOKENS.verify,
    model: config.MODELS.verify,
    budgetUsd: config.BUDGET.verify,
    cwd: exec.cwd,
    graph: exec.graph
  });
  budget.settle(config.BUDGET.verify, result.cost);
  return {
    verdicts: result.output?.results ?? null,
    result,
    skipped: result.ok ? null : `the verifier did not finish (${result.subtype})`
  };
}

function promptsFor(load: RoundInput, repo: string, prior: readonly Finding[]): Prompts {
  const comments = load.comments ?? [];
  const context = contextBlock({
    repo,
    meta: load.meta,
    plan: load.plan,
    mergeBase: load.mergeBase,
    head: load.head,
    prior,
    since: load.since,
    bodyChars: config.PR_BODY_CHARS,
    conversation: discussion(comments, config.REVIEWER_LOGINS)
  });
  const max = config.MAX_FINDINGS_PER_PASS;
  const lines = roadmapLines(load.roadmapDoc);
  return {
    general: () =>
      generalPrompt({ context, lenses: load.plan.lenses, maxFindings: max, roadmapLines: lines }),
    lens: (lens) => lensPrompt({ lens, context, maxFindings: max }),
    recheck: () =>
      recheckPrompt({
        findings: prior,
        replies: threadReplies({
          reviewComments: load.reviewComments,
          issueComments: comments,
          findings: prior,
          reviewerLogins: config.REVIEWER_LOGINS
        }),
        context,
        since: load.since,
        mergeBase: load.mergeBase,
        head: load.head
      }),
    verify: (candidates) =>
      verifyPrompt({
        candidates,
        context,
        mergeBase: load.mergeBase,
        head: load.head,
        maxFindings: max
      })
  };
}

/** Passes' answers → findings to add, plus everything that was set aside and why. */
async function sift(
  load: RoundInput,
  results: readonly PassResult[],
  budget: Budget,
  prompts: Prompts,
  exec: Exec
): Promise<Sifted> {
  const recheck = results.find((r) => r.name === "recheck") as
    | PassResult<RecheckOutput>
    | undefined;
  const findings = applyRecheck(load.prev.findings, recheck?.output?.results ?? [], load.head);
  const { candidates, notReviewed } = collectCandidates(results);
  const { kept: fresh, known } = dropKnown(
    candidates,
    findings.filter((f) => !isClosed(f.status))
  );
  const v = await verify(fresh, budget, prompts, exec);
  const verified = applyVerification(fresh, v.verdicts);
  const { kept, dropped } = applyFilters(verified.confirmed, config);
  return {
    findings,
    notReviewed,
    kept,
    possible: verified.possible,
    verify: v,
    debug: { ...verified, candidates, dropped, known }
  };
}

function statusOf(results: readonly PassResult[]): ReviewStatus {
  const general = results.find((r) => r.name === "general");
  if (!general?.ok) return "incomplete";
  return results.every((r) => r.ok) ? "complete" : "partial";
}

interface RoundContext {
  load: RoundInput;
  state: ReviewState;
  added: Finding[];
  closedNow: Finding[];
  all: PassResult[];
  sifted: Sifted;
  skipped: Skipped[];
  budget: Budget;
  started: number;
}

function roundRecord({
  load,
  state,
  added,
  closedNow,
  all,
  sifted,
  budget,
  started
}: RoundContext): Round {
  return {
    sha: load.head.slice(0, 9),
    at: new Date().toISOString(),
    mode: load.plan.mode,
    status: state.status,
    verdict: state.verdict,
    cost: cents(budget.spent),
    ms: Date.now() - started,
    lines: load.plan.size.lines,
    lenses: load.plan.lenses.map((l) => l.id),
    added: added.map((f) => f.id),
    closed: closedNow.map((f) => f.id),
    candidates: sifted.debug.candidates.length,
    rejected: sifted.debug.rejected.length,
    filtered: sifted.debug.dropped.length + sifted.debug.known.length,
    passes: Object.fromEntries(
      all.map((r) => [
        r.name,
        {
          ok: r.ok,
          subtype: r.subtype,
          cost: cents(r.cost),
          ms: r.ms,
          turns: r.turns,
          tools: r.tools,
          ...(r.usage ? { usage: r.usage } : {}),
          ...(r.first ? { first: r.first } : {})
        }
      ])
    )
  };
}

function viewOf({
  load,
  state,
  closedNow,
  all,
  sifted,
  skipped,
  budget,
  started
}: RoundContext): SummaryView {
  const general = all.find((r) => r.name === "general");
  const why = general?.error ? `: ${general.error.slice(0, 200)}` : "";
  return {
    mode: load.plan.mode,
    since: load.since,
    lenses: load.plan.lenses,
    skippedLenses: [
      ...load.plan.droppedLenses,
      ...skipped,
      ...(sifted.verify.skipped ? [{ id: "verify", reason: sifted.verify.skipped }] : [])
    ],
    failedPasses: all.filter((r) => !r.ok),
    notReviewed: sifted.notReviewed,
    ignored: load.plan.ignored,
    large: load.plan.large,
    size: load.plan.size,
    largeLimit: config.LARGE_PR_LINES,
    dropped: [...sifted.debug.dropped, ...sifted.debug.rejected, ...sifted.debug.known],
    closedNow,
    cost: budget.spent,
    ms: Date.now() - started,
    reason:
      state.status === "incomplete"
        ? `The general pass did not finish (\`${general?.subtype ?? "no_result"}\`${why}).`
        : null
  };
}

/**
 * This round's roadmap judgement. It is for this head only: a stale one must not outlive
 * a pass that did not finish. An incremental pass reads the latest changes, so it can
 * find behaviour an earlier push lacked but never unsee what an earlier push added: a
 * `changed` already on the PR stays until a full review says otherwise.
 */
function roadmapJudgement(
  load: RoundInput,
  results: readonly PassResult[]
): RoadmapJudgement | undefined {
  const general = results.find((r) => r.name === "general") as
    | PassResult<GeneralOutput>
    | undefined;
  const fresh = judgementOf(
    general?.ok ? general.output?.roadmap : undefined,
    new Set(roadmapLines(load.roadmapDoc).map((l) => l.id))
  );
  const prior = load.prev.roadmap;
  const incremental = load.plan.mode === "incremental";
  if (incremental && fresh?.behaviour === "unchanged" && prior?.behaviour === "changed")
    return prior;
  return fresh;
}

/** Run a round and return the next state plus what the summary needs to say. */
export async function runRound({
  load,
  repo,
  runPass,
  cwd,
  graph
}: { load: RoundInput; repo: string } & Exec): Promise<RoundOutcome> {
  const started = Date.now();
  const exec: Exec = { runPass, cwd, graph };
  const prior = openFindings(load.prev);
  const budget = new Budget(config.BUDGET.total, config.BUDGET.overshoot);
  // Every push rechecks what is open, incremental or not: after a force-push a fix
  // must still be able to close a finding.
  const { run, skipped } = schedulePasses({
    lenses: load.plan.lenses,
    needRecheck: prior.length > 0,
    budget,
    caps: config.BUDGET
  });
  const prompts = promptsFor(load, repo, prior);
  const results = await runScheduled(run, budget, prompts, exec);
  const sifted = await sift(load, results, budget, prompts, exec);

  const all: PassResult[] = sifted.verify.result ? [...results, sifted.verify.result] : results;
  const added = addFindings({ ...load.prev, findings: sifted.findings }, sifted.kept, load.head);
  const state: ReviewState = {
    ...load.prev,
    head: load.head,
    base: load.mergeBase,
    status: statusOf(all),
    findings: added.findings,
    next: added.next,
    possible: sifted.possible.map(({ title, file, line, severity, category, why }) => ({
      title,
      file,
      line,
      severity,
      category,
      why: why ?? ""
    }))
  };
  state.verdict = verdictFor(openFindings(state));
  const judgement = roadmapJudgement(load, results);
  if (judgement) state.roadmap = judgement;
  else delete state.roadmap;
  const wasClosed = new Set(load.prev.findings.filter((f) => isClosed(f.status)).map((f) => f.id));
  const closedNow = state.findings.filter((f) => isClosed(f.status) && !wasClosed.has(f.id));
  const ctx: RoundContext = {
    load,
    state,
    added: added.added,
    closedNow,
    all,
    sifted,
    skipped,
    budget,
    started
  };
  state.rounds = [...load.prev.rounds, roundRecord(ctx)];
  return {
    state,
    view: viewOf(ctx),
    added: added.added,
    closedNow,
    results: all,
    debug: sifted.debug
  };
}
