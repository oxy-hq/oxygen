/**
 * The routes a runner drives a build's promote checks through, under
 * `/api/customer-apps/{app}/builds/{build}/checks`:
 *
 *   POST claim              take the attempt, of staging or of production;
 *                           the platform writes `validation` and starts the
 *                           build's check functions (a staging attempt)
 *   POST functions/settle   ask whether those functions have a verdict yet
 *   GET  stylesheets        the build's `index.html` and CSS, for the guard
 *   POST report             write the verdict of a check the runner owns
 *   PUT  files/{name}       keep one file of the attempt's evidence on the
 *                           platform: a picture, a page's text, the CSS digest
 *
 * Every refusal answers `{ "code": …, "message": … }`. A claim is idempotent,
 * a report leaves a row that already has a verdict as it is, and a file is
 * stored under its name, so each call here is safe to repeat: one that failed
 * for a reason that may pass (the network, a 5xx, a 429) is sent again, a few
 * times, with a growing wait.
 */

import { type ApiResponse, parseJson, request } from "../api/request.js";
import { type Creds, ensureOk } from "../apps/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import type { StyleSource } from "./design-system.js";
import type { Outcome } from "./outcome.js";

/** The kinds a runner computes and reports; the platform and its model own the rest. */
export type RunnerKind = "loads" | "design_system" | "smoke" | "changes" | "production_smoke";

/** Where an attempt's checks run: the build on staging, or the build production now serves. */
export type CheckEnvironment = "staging" | "production";

/** What one attempt may keep, as the claim says. */
export interface Limits {
  /** The most screens opened. */
  screens: number;
  /** The largest file kept, in bytes. */
  file_bytes: number;
  /** The most files kept for the attempt. */
  files: number;
}

/** What a claim that names no limits gets. */
export const DEFAULT_LIMITS: Limits = { screens: 30, file_bytes: 2_000_000, files: 200 };

/**
 * The most errors a baseline lists for one screen and viewport: what the
 * `changes` row keeps of production's errors, and what the platform keeps of
 * that (`production/baseline.rs` `MAX_ERRORS`). A list this long may have
 * been cut: what is not in it was not necessarily not there.
 */
export const BASELINE_MAX_ERRORS = 10;

/** One screen at one viewport, as production showed it before the move. */
export interface BaselineScreen {
  path: string;
  viewport: string;
  ready: boolean;
  errors: string[];
}

/** What the newest `changes` run of the build saw on production before the move. */
export interface Baseline {
  at: string | null;
  screens: BaselineScreen[];
}

/** Who is running the attempt, as the console shows it. */
export interface Runner {
  label: string;
  url: string | null;
}

/** `POST …/checks/claim`, 200. */
export interface Claim {
  app: { id: string; org_slug: string; slug: string; workspace_id?: string | null };
  /** `id` is the build's row id, which its page reports as its build; `build_id` the published id. */
  build: { id: string; build_id: string; sdk_version: string | null };
  environment: string;
  attempt: number;
  serving: boolean;
  url: string | null;
  sdk_floor: string;
  /** The principal the platform resolved the credential to, when it says. */
  identity?: string | null;
  /** The kinds still without a verdict after the claim. */
  open: string[];
  /** The build's declared screens (app-relative paths, `""` the root); null or absent: discover them. */
  screens?: string[] | null;
  /**
   * What production serves now (a staging attempt); null when nothing, or this same build.
   * `id` is the build's row id, which is what a served page reports as its build
   * (`window.__OXY_APP__.buildId`); `build_id` is the id it was published under.
   */
  production?: { id: string; build_id: string } | null;
  /** A production attempt: what production showed before the move; null when nothing is known. */
  baseline?: Baseline | null;
  limits?: Partial<Limits> | null;
}

/** The claim's limits, each a positive whole number or the default. */
export function limitsOf(claim: Pick<Claim, "limits">): Limits {
  const given: Partial<Record<keyof Limits, unknown>> = claim.limits ?? {};
  const pick = (name: keyof Limits) => {
    const value = given[name];
    return typeof value === "number" && Number.isInteger(value) && value > 0
      ? value
      : DEFAULT_LIMITS[name];
  };
  return { screens: pick("screens"), file_bytes: pick("file_bytes"), files: pick("files") };
}

/** The claim's baseline with every field checked for its type; `undefined` when there is none. */
export function baselineOf(claim: Pick<Claim, "baseline">): Baseline | undefined {
  const given = claim.baseline;
  if (!given || typeof given !== "object" || !Array.isArray(given.screens)) return undefined;
  const screens: BaselineScreen[] = [];
  for (const row of given.screens as unknown[]) {
    const screen = row as Partial<BaselineScreen> | null;
    if (typeof screen?.path !== "string" || typeof screen.viewport !== "string") continue;
    screens.push({
      path: screen.path,
      viewport: screen.viewport,
      ready: screen.ready === true,
      errors: Array.isArray(screen.errors)
        ? screen.errors.filter((e): e is string => typeof e === "string")
        : []
    });
  }
  return { at: typeof given.at === "string" ? given.at : null, screens };
}

/** `POST …/checks/functions/settle`, 200. */
export interface Settle {
  done: boolean;
  status: string;
  summary: string | null;
}

/** `POST …/checks/report`, 200. */
export interface ReportAnswer {
  state: string;
  open: string[];
}

export interface PromoteApi {
  creds: Creds;
  appId: string;
  /** The build's row id or its `build_id`, as the caller named it. */
  build: string;
  /** Which attempt of the build: staging's, or production's after a promote. */
  environment: CheckEnvironment;
  /** The wait before each repeat of a call that failed; its length is how many repeats. */
  backoffMs: readonly number[];
  sleep: (ms: number) => Promise<void>;
}

/** The waits between repeats of a failed call: three repeats. */
export const DEFAULT_BACKOFF_MS: readonly number[] = [1_000, 2_000, 4_000];

function base(api: PromoteApi): string {
  return `/api/customer-apps/${api.appId}/builds/${encodeURIComponent(api.build)}/checks`;
}

/** A status worth sending the same request again for. */
function mayPass(status: number): boolean {
  return status >= 500 || status === 408 || status === 429;
}

/** A body that is not JSON: a file's bytes and what they are. */
interface RawBody {
  bytes: Uint8Array;
  contentType: string;
}

type Method = "GET" | "POST" | "PUT";

/** What a call that is not plain JSON changes. */
interface CallOptions {
  /** A file's bytes, sent in place of the JSON body. */
  raw?: RawBody;
  /** The wait before each repeat; `api.backoffMs` when absent. */
  backoffMs?: readonly number[];
}

/** The request itself, once: `body` as JSON, unless the call carries a file's bytes. */
function send(
  api: PromoteApi,
  method: Method,
  path: string,
  body: unknown,
  raw: RawBody | undefined
): Promise<ApiResponse> {
  return request({
    target: api.creds.target,
    path: `${base(api)}${path}`,
    method,
    body: raw ? raw.bytes : body === undefined ? undefined : JSON.stringify(body),
    bearer: api.creds.bearer,
    headers: raw ? { ...api.creds.headers, "Content-Type": raw.contentType } : api.creds.headers
  });
}

/**
 * One call, repeated while it fails for a reason that may pass. Returns the
 * last response whatever its status; throws only when the deployment never
 * answered.
 */
async function call(
  api: PromoteApi,
  method: Method,
  path: string,
  body?: unknown,
  options: CallOptions = {}
): Promise<ApiResponse> {
  const backoffMs = options.backoffMs ?? api.backoffMs;
  for (let attempt = 0; ; attempt += 1) {
    const wait = backoffMs[attempt];
    try {
      const response = await send(api, method, path, body, options.raw);
      if (!mayPass(response.status) || wait === undefined) return response;
    } catch (cause) {
      if (wait === undefined) throw cause;
    }
    await api.sleep(wait);
  }
}

/** The body as an object, or a failure naming the route that did not answer one. */
function objectOf(response: ApiResponse, what: string): Record<string, unknown> {
  const parsed = parseJson(response.body);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new CliError(`${what} did not answer a JSON object`, {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined,
      hint: "the deployment may predate runner-driven build checks — `oxyc routes customer-apps` lists what it mounts"
    });
  }
  return parsed as Record<string, unknown>;
}

/** The refusal's own code (`attempt_closed`), when the body carries one. */
function refusalCode(response: ApiResponse): string | undefined {
  const parsed = parseJson(response.body) as { code?: unknown } | undefined;
  return typeof parsed?.code === "string" ? parsed.code : undefined;
}

/**
 * Claim the attempt (`null`: the newest). `"closed"` when every check of it
 * already has a verdict: there is nothing left to run.
 */
export async function claimAttempt(
  api: PromoteApi,
  attempt: number | null,
  runner: Runner
): Promise<Claim | "closed"> {
  const response = await call(api, "POST", "/claim", {
    attempt,
    runner,
    environment: api.environment
  });
  if (response.status === 409 && refusalCode(response) === "attempt_closed") return "closed";
  try {
    ensureOk(response, api.creds);
  } catch (cause) {
    if (!(cause instanceof CliError) || cause.code !== ExitCode.NOT_FOUND) throw cause;
    throw new CliError(cause.message, {
      code: cause.code,
      detail: cause.detail,
      hint: `no build "${api.build}" of this app${attempt === null ? "" : `, or no attempt ${attempt} of it`}, or this credential may not check it: the platform's checker and staff who can open the app's staging can`
    });
  }
  const claim = objectOf(response, "POST …/checks/claim") as Partial<Claim>;
  if (
    typeof claim.attempt !== "number" ||
    !Array.isArray(claim.open) ||
    typeof claim.build?.id !== "string" ||
    typeof claim.build?.build_id !== "string"
  ) {
    throw new CliError("POST …/checks/claim did not answer a claim", {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined
    });
  }
  return claim as Claim;
}

/** Ask whether the build's check functions have a verdict yet. */
export async function settleFunctions(api: PromoteApi, attempt: number): Promise<Settle> {
  const response = await call(api, "POST", "/functions/settle", { attempt });
  ensureOk(response, api.creds);
  const settle = objectOf(response, "POST …/checks/functions/settle");
  return {
    done: settle.done === true,
    status: typeof settle.status === "string" ? settle.status : "running",
    summary: typeof settle.summary === "string" ? settle.summary : null
  };
}

/** The build's `index.html` and every stylesheet under `assets/`. */
export async function fetchStylesheets(api: PromoteApi): Promise<StyleSource[]> {
  const response = await call(api, "GET", "/stylesheets");
  ensureOk(response, api.creds);
  const sources = objectOf(response, "GET …/checks/stylesheets").sources;
  if (!Array.isArray(sources)) {
    throw new CliError("GET …/checks/stylesheets did not answer the build's sources", {
      code: ExitCode.FAILURE
    });
  }
  return sources.filter(
    (source): source is StyleSource =>
      typeof source === "object" &&
      source !== null &&
      typeof (source as StyleSource).file === "string" &&
      typeof (source as StyleSource).css === "string"
  );
}

/** Write one check's verdict. */
export async function reportCheck(
  api: PromoteApi,
  attempt: number,
  kind: RunnerKind,
  verdict: Outcome
): Promise<ReportAnswer> {
  const response = await call(api, "POST", "/report", {
    attempt,
    environment: api.environment,
    checks: [{ kind, status: verdict.status, summary: verdict.summary, detail: verdict.detail }]
  });
  ensureOk(response, api.creds);
  const answer = objectOf(response, "POST …/checks/report");
  return {
    state: typeof answer.state === "string" ? answer.state : "running",
    open: Array.isArray(answer.open)
      ? answer.open.filter((k): k is string => typeof k === "string")
      : []
  };
}

/** What a kept file may be. */
export type FileType = "image/png" | "application/json";

/**
 * Keep one file of the attempt's evidence (`PUT …/files/{name}`). Throws the
 * platform's refusal, with its code (`file_too_large`, `too_many_files`,
 * `attempt_closed`, `invalid_name`). Repeated once when it failed for a reason
 * that may pass: a run uploads many, and none is worth a long wait.
 */
export async function putFile(
  api: PromoteApi,
  attempt: number,
  name: string,
  contentType: FileType,
  bytes: Uint8Array
): Promise<void> {
  const query = `attempt=${attempt}&environment=${api.environment}`;
  const response = await call(
    api,
    "PUT",
    `/files/${encodeURIComponent(name)}?${query}`,
    undefined,
    {
      raw: { bytes, contentType },
      backoffMs: api.backoffMs.slice(0, 1)
    }
  );
  ensureOk(response, api.creds);
}
