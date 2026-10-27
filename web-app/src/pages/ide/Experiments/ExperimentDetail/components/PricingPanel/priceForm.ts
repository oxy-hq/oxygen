import { isCalendarDate } from "@/pages/ide/Experiments/dates";
import {
  type Grain,
  isSwitchMoment,
  periodStartRule,
  periodWord,
  switchDate
} from "@/pages/ide/Experiments/grain";
import type { Candidate, PriceRequest } from "@/types/experiment";

/** A request prices 1 to this many candidates (index C5), counted after the α preset is expanded. */
export const MAX_CANDIDATES = 6;
/** The server's candidate bounds (`candidate.rs`): a request over them is refused after the round trip. */
export const MAX_WAVES = 52;
export const MAX_SPACING_DAYS = 366;
export const MAX_PERIOD_DAYS = 56;
export const MAX_PAIRS = 26;
export const STANDARD_ALPHA = 0.05;
/** The directional preset: a looser α registered BEFORE the switch (spec, "Directional experiments"). */
export const DIRECTIONAL_ALPHA = 0.2;

export type CandidateShape = "waves" | "switchback";
/** `both` prices the same design twice, at 0.05 and at 0.2, so the trade is read off two numbers. */
export type AlphaChoice = "0.05" | "0.2" | "both";

/** One form row = one design. Every input is the raw text the person typed. */
export interface CandidateFormValues {
  shape: CandidateShape;
  alpha: AlphaChoice;
  // waves
  waveSizes: string;
  firstSwitchOn: string;
  spacingDays: string;
  blocks: string;
  /** Pair units by baseline and treat one of each pair; wave sizes and blocks are then set by the server. */
  matchPairs: boolean;
  assignBy: string;
  // switchback
  firstPeriodOn: string;
  periodDays: string;
  pairs: string;
}

export interface PriceFormValues {
  candidates: CandidateFormValues[];
}

export interface PriceFormContext {
  /** The earliest calendar date it is anywhere on Earth right now (`earliestTodayOnEarth`). */
  earliestToday: string;
  /**
   * `design.washout_days`, when the page was given it. The list item does not
   * carry it yet (Contract issue 25), so today nothing passes it and the check is dormant.
   */
  washoutDays?: number | null;
  /** `design.grain`: every length counts these, and every first date must start one. Absent = day. */
  grain?: Grain;
}

const many = (ctx: PriceFormContext) => periodWord(ctx.grain ?? "day", { plural: true });

export type CandidateField = Exclude<keyof CandidateFormValues, "shape" | "alpha">;
export type CandidateErrors = Partial<Record<CandidateField, string>>;

export interface PriceFormResult {
  /** Parallel to `values.candidates`; `{}` for a row with nothing wrong. */
  candidateErrors: CandidateErrors[];
  /** About the set, not a row: none, too many, or (after expansion) over the limit. */
  formError: string | null;
  request: PriceRequest | null;
}

/**
 * Fewest pairs that can reach `alpha` at all: a sign-flip test over n pairs has
 * 2^n arrangements, so its smallest two-sided p-value is 2/2^n (6 pairs for 0.05,
 * 4 for 0.2). The engine refuses fewer; the form says so before a job is queued.
 */
export function minPairs(alpha: number): number {
  let n = 1;
  while (2 / 2 ** n > alpha) n += 1;
  return n;
}

/** A new row starts in the draft's own shape: a switchback draft prices switchbacks. */
export function draftShape(design: { switchback: unknown | null }): CandidateShape {
  return design.switchback ? "switchback" : "waves";
}

export function newCandidate(shape: CandidateShape = "waves"): CandidateFormValues {
  return {
    shape,
    alpha: "0.05",
    waveSizes: "",
    firstSwitchOn: "",
    spacingDays: "14",
    blocks: "",
    matchPairs: false,
    assignBy: "",
    firstPeriodOn: "",
    periodDays: "7",
    pairs: "6"
  };
}

export function alphasOf(choice: AlphaChoice): number[] {
  return choice === "both" ? [STANDARD_ALPHA, DIRECTIONAL_ALPHA] : [Number(choice)];
}

/** Candidates a set of rows would price: a row at both alphas is two. */
export function expandedCount(rows: Pick<CandidateFormValues, "alpha">[]): number {
  return rows.reduce((total, row) => total + alphasOf(row.alpha).length, 0);
}

// ---- field parsers ------------------------------------------------------------

type Parsed<T> = { value: T; error: null } | { value: null; error: string };
const good = <T>(value: T): Parsed<T> => ({ value, error: null });
const bad = (error: string): Parsed<never> => ({ value: null, error });

const WHOLE_NUMBER = /^\d{1,6}$/;
const DIMENSION_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const EMPTY_ENTRY =
  "Wave sizes has an empty entry. Separate sizes with single commas, for example 2, 3.";

function parseWaveSizes(raw: string): Parsed<number[]> {
  const text = raw.trim();
  if (text === "") return bad("Enter at least one wave size, for example 2, 3.");
  const sizes: number[] = [];
  for (const token of text.split(",").map((t) => t.trim())) {
    if (token === "") return bad(EMPTY_ENTRY);
    if (!WHOLE_NUMBER.test(token)) return bad(`"${token}" is not a whole number of 2 or more.`);
    const size = Number(token);
    if (size === 0) return bad("A wave size of 0 places no units. Use 2 or more.");
    if (size === 1) return bad("Wave sizes must be 2 or more; 1 is too small.");
    sizes.push(size);
  }
  if (sizes.length > MAX_WAVES) return bad(`At most ${MAX_WAVES} waves, got ${sizes.length}.`);
  return good(sizes);
}

function parseWhole(
  raw: string,
  min: number,
  empty: string,
  invalid: string,
  over?: { max: number; message: string }
): Parsed<number> {
  const text = raw.trim();
  if (text === "") return bad(empty);
  if (!WHOLE_NUMBER.test(text) || Number(text) < min) return bad(invalid);
  if (over && Number(text) > over.max) return bad(over.message);
  return good(Number(text));
}

function parseDate(raw: string, ctx: PriceFormContext, empty: string): Parsed<string> {
  const value = raw.trim();
  if (value === "") return bad(empty);
  const grain = ctx.grain ?? "day";
  const day = grain === "hour" ? switchDate(value) : value;
  if (!isCalendarDate(day)) {
    return bad(
      grain === "hour"
        ? "Enter a real date and hour as YYYY-MM-DDTHH:00."
        : "Enter a real calendar date as YYYY-MM-DD."
    );
  }
  // ISO dates order lexicographically. Only a date past in EVERY zone is refused
  // here; the exact check, in the design's own timezone, is the server's.
  if (day < ctx.earliestToday) {
    return bad(
      `${day} is already past everywhere (the earliest date on Earth today is ${ctx.earliestToday}). Choose a later date.`
    );
  }
  if (!isSwitchMoment(value, grain)) return bad(periodStartRule(grain));
  return good(value);
}

function parseBlocks(raw: string): Parsed<number | undefined> {
  const text = raw.trim();
  if (text === "") return good(undefined);
  if (!WHOLE_NUMBER.test(text) || Number(text) < 2) {
    return bad("Blocks must be a whole number of 2 or more, or empty for an unblocked draw.");
  }
  return good(Number(text));
}

function parseAssignBy(raw: string): Parsed<string | undefined> {
  const text = raw.trim();
  if (text === "") return good(undefined);
  if (!DIMENSION_NAME.test(text)) {
    return bad("A dimension name uses letters, digits and underscores, and starts with a letter.");
  }
  return good(text);
}

// ---- one row -------------------------------------------------------------------

/** What a row yields: its errors, and — only when it has none — a builder that takes the α. */
interface Checked {
  errors: CandidateErrors;
  build: ((alpha: number) => Candidate) | null;
}

function checkWaves(v: CandidateFormValues, ctx: PriceFormContext): Checked {
  if (v.matchPairs) return checkPairs(v, ctx);
  const sizes = parseWaveSizes(v.waveSizes);
  const date = parseDate(v.firstSwitchOn, ctx, "Choose the first switch date.");
  const spacing = parseWhole(
    v.spacingDays,
    1,
    `Enter the spacing between waves in ${many(ctx)}.`,
    `Spacing must be a whole number of ${many(ctx)}, 1 or more.`,
    // The server checks spacing only when there is more than one wave.
    (sizes.value?.length ?? 0) > 1
      ? {
          max: MAX_SPACING_DAYS,
          message: `Spacing must be between 1 and ${MAX_SPACING_DAYS} ${many(ctx)} when there is more than one wave.`
        }
      : undefined
  );
  const blocks = parseBlocks(v.blocks);
  const assignBy = parseAssignBy(v.assignBy);

  const errors: CandidateErrors = {};
  if (sizes.error) errors.waveSizes = sizes.error;
  if (date.error) errors.firstSwitchOn = date.error;
  if (spacing.error) errors.spacingDays = spacing.error;
  if (blocks.error) errors.blocks = blocks.error;
  if (assignBy.error) errors.assignBy = assignBy.error;
  if (Object.keys(errors).length > 0) return { errors, build: null };
  if (sizes.value === null || date.value === null || spacing.value === null) {
    return { errors, build: null };
  }

  const wave_sizes = sizes.value;
  const first_switch_on = date.value;
  const spacing_days = spacing.value;
  const optional = {
    ...(blocks.value ? { blocks: blocks.value } : {}),
    ...(assignBy.value ? { assign_by: assignBy.value } : {})
  };
  return {
    errors,
    build: (alpha) => ({
      shape: "waves",
      wave_sizes,
      first_switch_on,
      spacing_days,
      ...optional,
      alpha
    })
  };
}

/** Matched pairs: one switch date, the waves and blocks the server derives from the unit count. */
function checkPairs(v: CandidateFormValues, ctx: PriceFormContext): Checked {
  const date = parseDate(v.firstSwitchOn, ctx, "Choose the first switch date.");
  const assignBy = parseAssignBy(v.assignBy);
  const errors: CandidateErrors = {};
  if (date.error) errors.firstSwitchOn = date.error;
  if (assignBy.error) errors.assignBy = assignBy.error;
  if (Object.keys(errors).length > 0 || date.value === null) return { errors, build: null };
  const first_switch_on = date.value;
  const optional = assignBy.value ? { assign_by: assignBy.value } : {};
  return {
    errors,
    build: (alpha) => ({
      shape: "waves",
      pairs: true,
      wave_sizes: [],
      first_switch_on,
      spacing_days: 14,
      ...optional,
      alpha
    })
  };
}

function checkSwitchback(v: CandidateFormValues, ctx: PriceFormContext): Checked {
  const first = parseDate(v.firstPeriodOn, ctx, "Choose the first period's start date.");
  const period = parseWhole(
    v.periodDays,
    1,
    `Enter the period length in ${many(ctx)}.`,
    `Period length must be a whole number of ${many(ctx)}, 1 or more.`,
    {
      max: MAX_PERIOD_DAYS,
      message: `Period length must be between 1 and ${MAX_PERIOD_DAYS} ${many(ctx)}.`
    }
  );
  const pairs = parseWhole(
    v.pairs,
    1,
    "Enter the number of on/off pairs.",
    "Pairs must be a whole number of 1 or more.",
    { max: MAX_PAIRS, message: `Pairs must be between 1 and ${MAX_PAIRS}.` }
  );

  const errors: CandidateErrors = {};
  if (first.error) errors.firstPeriodOn = first.error;
  if (period.error) errors.periodDays = period.error;
  if (pairs.error) errors.pairs = pairs.error;
  else if (pairs.value !== null) {
    // At both alphas the stricter one decides: a row that cannot price one is refused.
    const alpha = Math.min(...alphasOf(v.alpha));
    const least = minPairs(alpha);
    if (pairs.value < least) {
      errors.pairs = `At α ${alpha} a switchback needs at least ${least} pairs; ${pairs.value} can never reach it.`;
    }
  }
  // The washout is a key in the FILE, so the client can check it only when told it.
  if (period.value !== null && ctx.washoutDays != null && period.value <= ctx.washoutDays) {
    errors.periodDays = `A period must be longer than the ${ctx.washoutDays}-${periodWord(ctx.grain ?? "day")} washout.`;
  }
  if (Object.keys(errors).length > 0) return { errors, build: null };
  if (first.value === null || period.value === null || pairs.value === null) {
    return { errors, build: null };
  }

  const first_period_on = first.value;
  const period_days = period.value;
  const pairCount = pairs.value;
  return {
    errors,
    build: (alpha) => ({
      shape: "switchback",
      first_period_on,
      period_days,
      pairs: pairCount,
      alpha
    })
  };
}

// ---- the set -------------------------------------------------------------------

function setError(rows: CandidateFormValues[]): string | null {
  const count = expandedCount(rows);
  if (count === 0) return "Add at least one candidate to price.";
  if (count <= MAX_CANDIDATES) return null;
  const twice = rows.some((r) => r.alpha === "both")
    ? " (a row priced at both alphas counts twice)"
    : "";
  return `A request prices 1 to ${MAX_CANDIDATES} candidates; this one asks for ${count}${twice}.`;
}

/**
 * Checks the form before anything is queued and puts each message next to its
 * own field. The server re-checks what it can see and the engine refuses what it
 * cannot place — this catches what a person can see is wrong without a round
 * trip, including a wave of one unit, which the server would otherwise only
 * refuse after a queued job. Only the shape a row has chosen is read: the other
 * shape's inputs keep whatever was typed and never block a request.
 */
export function validatePriceForm(values: PriceFormValues, ctx: PriceFormContext): PriceFormResult {
  const rows = values.candidates;
  const checked = rows.map((row) =>
    row.shape === "waves" ? checkWaves(row, ctx) : checkSwitchback(row, ctx)
  );
  const candidateErrors = checked.map((c) => c.errors);
  const formError = setError(rows);
  if (formError || checked.some((c) => c.build === null)) {
    return { candidateErrors, formError, request: null };
  }
  const candidates = checked.flatMap((c, i) =>
    c.build ? alphasOf(rows[i].alpha).map(c.build) : []
  );
  return { candidateErrors, formError: null, request: { candidates } };
}
