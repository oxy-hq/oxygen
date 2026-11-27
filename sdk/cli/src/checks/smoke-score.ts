/**
 * The verdicts over every screen of a build: `smoke` (staging) and
 * `production_smoke` (production, after a promote).
 *
 * `smoke` holds each screen at each viewport to the rule `loads` holds the
 * root to (`loads-score.ts`): the document loaded as the app, on the
 * environment and build being checked, sent `oxy-app-ready`, and threw
 * nothing. Failed requests and the requests the check refused (a write, or
 * anything to another origin) are warnings.
 * Its detail also names the screens of the app the opened ones link to that
 * the list does not hold (`screens_unlisted`): they were not opened, which a
 * verdict over "every screen" should not leave unsaid.
 *
 * `production_smoke` does not ask whether production is perfect. It asks what
 * the move BROKE: an error the screen did not have before the move, or a
 * screen that was ready before and is not now. "Before" is the baseline, what
 * the build's `changes` run saw on production, ONE ENTRY PER SCREEN AND
 * VIEWPORT. What a screen shows can be called new only against its own
 * entry. Where there is none (no baseline at all: a promoting publish, a
 * build promoted before its `changes` ran; or a baseline that does not list
 * the screen: its pass ran out of time, or production links to other screens
 * now) nothing can be called new: the errors shown there are listed as
 * warnings, and a check with no new error but some of those is `skipped`,
 * never `failed`, so an error production already had neither pages anyone
 * nor rolls production back.
 *
 * AN ENTRY'S LIST OF ERRORS MAY HAVE BEEN CUT. A baseline keeps at most ten
 * errors a screen, and a load can throw more. So against a list that is full,
 * an uncaught error it does not hold is not called new either: it is a
 * warning. What a cut cannot hide still counts: a document that failed (the
 * first of a list, never cut), and a screen that was ready and is not now.
 *
 * A detail lists many page loads, and the platform refuses one past 256 KB,
 * so the lists under each load are cut until the whole detail fits.
 */

import { type Observed, problemOf, verdict, viewportDetail, warningsOf } from "./loads-score.js";
import { type LoadsTarget, VIEWPORTS } from "./loads-target.js";
import { cap, type Finding, finding, MAX_MESSAGE_CHARS, type Outcome, outcome } from "./outcome.js";
import { BASELINE_MAX_ERRORS, type Baseline, type BaselineScreen } from "./promote-api.js";
import { screenLabel } from "./screens.js";
import type { ScreensPass, ScreenVisit } from "./screens-browser.js";

/** The largest detail reported: under the 256 KB the platform stores. */
const MAX_DETAIL_BYTES = 200_000;
/** The most page errors, failed requests and refused requests of each kind listed per load, tried in this order. */
const KEEP_STEPS = [20, 5, 2, 0] as const;
/** The most new errors listed. */
const MAX_NEW_ERRORS = 50;

interface Screen {
  path: string;
  visits: ScreenVisit[];
}

/** The pass's visits by screen, in the list's order, desk before phone. */
function byScreen(pass: ScreensPass): Screen[] {
  const order = VIEWPORTS.map((viewport) => viewport.name);
  return pass.list.paths.map((path) => ({
    path,
    visits: pass.visits
      .filter((visit) => visit.path === path)
      .sort((a, b) => order.indexOf(a.viewport.name) - order.indexOf(b.viewport.name))
  }));
}

function where(visit: ScreenVisit): string {
  return `${screenLabel(visit.path)} at ${visit.viewport.name}`;
}

/** `14 screens`, `1 screen`. */
function screens(count: number): string {
  return `${count} screen${count === 1 ? "" : "s"}`;
}

/** What every screen check's detail says about the list, and each screen's loads. */
function listDetail(pass: ScreensPass, all: Screen[], keep: number): Record<string, unknown> {
  const detail: Record<string, unknown> = {
    discovered: pass.list.discovered,
    screens_omitted: pass.list.omitted,
    screens: all
      .filter((screen) => screen.visits.length > 0)
      .map((screen) => ({
        path: screen.path,
        viewports: screen.visits.map((visit) =>
          viewportDetail(visit.viewport, visit.observed, keep)
        )
      }))
  };
  const unopened = all.filter((screen) => screen.visits.length < VIEWPORTS.length).length;
  if (unopened > 0) detail.screens_unvisited = unopened;
  if (keep < KEEP_STEPS[0]) detail.lists_capped_at = keep;
  return detail;
}

/** The screens the pass saw linked and did not open, as `smoke` names them; nothing when there are none. */
function unlistedDetail(pass: ScreensPass): Record<string, unknown> {
  const { paths, total } = pass.unlisted;
  return total > 0 ? { screens_unlisted: paths, screens_unlisted_total: total } : {};
}

/** The first outcome whose detail fits what the platform stores; the leanest when none does. */
function fitted(make: (keep: number) => Outcome): Outcome {
  const [first, ...leaner] = KEEP_STEPS;
  let made = make(first);
  for (const keep of leaner) {
    if (Buffer.byteLength(JSON.stringify(made.detail), "utf8") <= MAX_DETAIL_BYTES) break;
    made = make(keep);
  }
  return made;
}

/** The verdict when no screen failed but some were never opened; `undefined` when all were. */
function unopenedVerdict(all: Screen[]): { summary: string; finding: Finding } | undefined {
  const opened = all.filter((screen) => screen.visits.length === VIEWPORTS.length).length;
  if (opened === all.length) return undefined;
  const summary = `${opened} of ${screens(all.length)} opened before the time was up`;
  return { summary, finding: finding("error", `${summary}: the rest were not checked`) };
}

/** `smoke`: every screen at every viewport renders and throws nothing. */
export function scoreSmoke(target: LoadsTarget, pass: ScreensPass): Outcome {
  const all = byScreen(pass);
  const errors: Finding[] = [];
  const warnings: Finding[] = [];
  let failing = 0;
  for (const screen of all) {
    const before = errors.length;
    for (const visit of screen.visits) {
      const why = verdict(target, visit.observed);
      if (why !== undefined) errors.push(finding("error", `${where(visit)}: ${why}`));
      warnings.push(...warningsOf(where(visit), visit.observed));
    }
    if (errors.length > before) failing += 1;
  }
  const unopened = unopenedVerdict(all);
  // Errors first: the cut at fifty findings takes warnings before it takes one of them.
  const findings = [...errors, ...(unopened ? [unopened.finding] : []), ...warnings];
  const made = (keep: number) => {
    const detail = { ...listDetail(pass, all, keep), ...unlistedDetail(pass) };
    if (failing > 0) {
      const verb = failing === 1 ? "has" : "have";
      return outcome(
        "failed",
        `${failing} of ${screens(all.length)} ${verb} errors`,
        findings,
        detail
      );
    }
    if (unopened) return outcome("error", unopened.summary, findings, detail);
    return outcome("passed", `${screens(all.length)}, 0 errors`, findings, detail);
  };
  return fitted(made);
}

/** The errors a screen shows now, as a baseline lists them: why the document failed, then what it threw. */
export function errorsOf(observed: Observed): string[] {
  return [...(observed.navigationError ? [observed.navigationError] : []), ...observed.pageErrors];
}

/** Everything wrong with a load, as a list of errors: what it threw, then why it is not the app. */
function everythingWrong(target: LoadsTarget, observed: Observed): string[] {
  const problem = problemOf(target, observed);
  // A thrown error and a failed document are in the list already.
  const listed = !problem || problem.kind === "page_error" || problem.kind === "navigation";
  return listed ? errorsOf(observed) : [...errorsOf(observed), problem.why];
}

/** One load held against its own baseline entry. */
interface Held {
  /** What it shows that the entry did not: the move's doing. */
  fresh: string[];
  /** What it throws that a FULL list does not hold: there before the cut, or not. */
  cut: string[];
}

/** What this load shows that `before`, its own baseline entry, did not. */
function heldAgainst(target: LoadsTarget, observed: Observed, before: BaselineScreen): Held {
  const unlisted = (error: string) => !before.errors.includes(error);
  // A failed document is the first error of a list: a cut, which drops the last, keeps it.
  const failed = observed.navigationError;
  const fresh = failed !== undefined && unlisted(failed) ? [failed] : [];
  const thrown = observed.pageErrors.filter(unlisted);
  const full = before.errors.length >= BASELINE_MAX_ERRORS;
  const held: Held = full ? { fresh, cut: thrown } : { fresh: [...fresh, ...thrown], cut: [] };
  const problem = problemOf(target, observed);
  // A thrown error and a failed document are in the lists above already.
  if (!problem || problem.kind === "page_error" || problem.kind === "navigation") return held;
  // Not ready now is the move's doing only when it was ready before.
  if (problem.kind === "not_ready" && !before.ready) return held;
  held.fresh.push(problem.why);
  return held;
}

type ScreenError = { path: string; viewport: string; error: string };

/** What production shows now, held against the baseline. */
interface Compared {
  /** What a screen shows that its baseline entry did not: the move's doing. */
  fresh: ScreenError[];
  /** What nothing can be said about, each with why (`not in the baseline`, a full list). */
  untold: (ScreenError & { why: string })[];
  warnings: Finding[];
}

/** Why an error a full list does not hold is not called new. */
const FULL_LIST = "the baseline's list of errors is full";

/** Each visit held against its own baseline entry; one with no entry has nothing called new. */
function errorsAgainst(
  target: LoadsTarget,
  all: Screen[],
  baseline: Baseline | undefined
): Compared {
  const before = new Map<string, BaselineScreen>();
  for (const screen of baseline?.screens ?? []) {
    before.set(`${screen.path}\n${screen.viewport}`, screen);
  }
  const found: Compared = { fresh: [], untold: [], warnings: [] };
  for (const visit of all.flatMap((screen) => screen.visits)) {
    const at = { path: visit.path, viewport: visit.viewport.name };
    const error = (text: string) => ({ ...at, error: cap(text, MAX_MESSAGE_CHARS) });
    const was = before.get(`${visit.path}\n${visit.viewport.name}`);
    if (was) {
      const held = heldAgainst(target, visit.observed, was);
      found.fresh.push(...held.fresh.map(error));
      for (const text of held.cut) found.untold.push({ ...error(text), why: FULL_LIST });
    } else {
      const why = baseline ? "not in the baseline" : "no baseline";
      for (const text of everythingWrong(target, visit.observed)) {
        found.untold.push({ ...error(text), why });
      }
    }
    found.warnings.push(...warningsOf(where(visit), visit.observed));
  }
  return found;
}

/** `3 new errors on 2 of 14 screens`, or without `new` where none can be called new. */
function errorsLine(errors: ScreenError[], all: Screen[], adjective: string): string {
  const broken = new Set(errors.map((e) => e.path)).size;
  const noun = errors.length === 1 ? "error" : "errors";
  return `${errors.length} ${adjective}${noun} on ${broken} of ${screens(all.length)}`;
}

/** `production_smoke`: what production shows now that it did not show before the move. */
export function scoreProductionSmoke(
  target: LoadsTarget,
  pass: ScreensPass,
  baseline: Baseline | undefined
): Outcome {
  const all = byScreen(pass);
  // A baseline that saw no screen can tell nothing apart, like none at all.
  const known = baseline !== undefined && baseline.screens.length > 0;
  const { fresh, untold, warnings } = errorsAgainst(target, all, known ? baseline : undefined);
  const unopened = unopenedVerdict(all);
  const label = (e: ScreenError) => `${screenLabel(e.path)} at ${e.viewport}: ${e.error}`;
  const findings = [
    ...fresh.map((e) => finding("error", label(e))),
    ...(unopened ? [unopened.finding] : []),
    ...untold.map((e) => finding("warning", `${label(e)} (${e.why}: not called new)`)),
    ...warnings
  ];
  const made = (keep: number) => {
    const detail: Record<string, unknown> = {
      baseline_at: known ? (baseline?.at ?? null) : null,
      ...listDetail(pass, all, keep),
      new_errors: fresh.slice(0, MAX_NEW_ERRORS)
    };
    if (fresh.length > MAX_NEW_ERRORS) detail.new_errors_omitted = fresh.length - MAX_NEW_ERRORS;
    return productionVerdict({ all, fresh, untold, known, unopened }, findings, detail);
  };
  return fitted(made);
}

/** The verdict over what was found: only an error its screen's baseline entry did not have fails the check. */
function productionVerdict(
  found: {
    all: Screen[];
    fresh: ScreenError[];
    untold: ScreenError[];
    known: boolean;
    unopened: { summary: string } | undefined;
  },
  findings: Finding[],
  detail: Record<string, unknown>
): Outcome {
  const { all, fresh, untold, known, unopened } = found;
  if (fresh.length > 0) return outcome("failed", errorsLine(fresh, all, "new "), findings, detail);
  if (unopened) return outcome("error", unopened.summary, findings, detail);
  if (untold.length > 0) {
    const why = known
      ? " the baseline from before the move cannot tell apart: not called new"
      : ", and no baseline from before the move to tell new ones apart";
    return outcome("skipped", `${errorsLine(untold, all, "")}${why}`, findings, detail);
  }
  return outcome("passed", `${screens(all.length)}, no new errors`, findings, detail);
}
