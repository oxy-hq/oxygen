/**
 * Workspace previews: the wire signals, read and written outside React.
 *
 *  - `x-oxy-preview-revision: <revision_id>` — sent on every request while a
 *    page is pinned to a preview (see `PreviewPinProvider`).
 *  - `x-oxy-preview: <branch>@<revision_id>` — stamped by the server on any
 *    response it served from a preview. The preview bar reads it to confirm the
 *    page really came from the pinned revision, rather than trusting the URL.
 *    Kept as a tiny pub/sub surfaced through `useSyncExternalStore`, the same
 *    shape as `ideHealth.ts` — neither server state nor a Zustand store.
 *  - `409 {"code":"preview_read_only","message":…}` — anything that runs or
 *    changes something while pinned. Its message is written for the person,
 *    so it is shown as-is instead of a generic failure.
 */

import { isAxiosError } from "axios";

/** The query parameter that pins a page to a preview: `?preview=<revision_id>`. */
export const PREVIEW_PARAM = "preview";

/** The response header the server stamps on anything it served from a preview. */
export const PREVIEW_HEADER = "x-oxy-preview";

/**
 * The request header that makes a request a preview request: the immutable
 * staging revision the page is pinned to. Sent on every API request while
 * pinned, beside the `?branch=<label>` the call site already sends.
 *
 * `?branch=` alone cannot tell a preview apart from the IDE, which sends the
 * very same `?branch=feat/x` for its editable working copy — so this header,
 * never the query string, is what the server keys on, and a page without a
 * pin (the IDE included) never sends it.
 */
const PREVIEW_REVISION_HEADER = "x-oxy-preview-revision";

/**
 * Whether a `?preview=` value can be a revision id. It comes straight from a
 * URL anyone can edit, and it is sent as a header: a value outside printable
 * ASCII makes the browser throw on EVERY request. Revision ids are opaque
 * server tokens (UUID-shaped), so anything else is simply not a pin.
 */
export function isRevisionToken(value: string): boolean {
  return /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(value);
}

/**
 * The revision the current workspace page is pinned to, published by
 * `PreviewPinProvider` for the request layer (axios interceptor, SSE call
 * sites), which runs outside React. Null whenever no page is pinned.
 */
let activeRevision: string | null = null;

export function setActivePreviewRevision(revisionId: string | null): void {
  activeRevision = revisionId;
}

export function getActivePreviewRevision(): string | null {
  return activeRevision;
}

/** Headers to spread into a hand-built request (`fetch`, SSE): empty when live. */
export function previewRequestHeaders(): Record<string, string> {
  return activeRevision ? { [PREVIEW_REVISION_HEADER]: activeRevision } : {};
}

const PREVIEW_READ_ONLY_CODE = "preview_read_only";

/**
 * Only shown if the server ever sends a `preview_read_only` body with no
 * `message` — every real refusal names the specific action instead (e.g.
 * "Creating a schedule isn't available in a preview…"). "Read-only" here
 * means the workspace and its data, not Oxy's own tables: starting a thread
 * or an analytics run still writes an ordinary Oxy row and is allowed.
 */
const FALLBACK_READ_ONLY_MESSAGE =
  "This is a preview — it can't change the workspace or its data. Exit preview to run or change things.";

interface ServedPreview {
  branch: string;
  revisionId: string;
}

/**
 * `<branch>@<revision_id>` split on the LAST `@`: git allows `@` inside a
 * branch name (only `@{` is reserved), while a revision id never carries one.
 */
function parsePreviewHeader(value: string | null | undefined): ServedPreview | null {
  if (!value) return null;
  const at = value.lastIndexOf("@");
  if (at <= 0 || at === value.length - 1) return null;
  return { branch: value.slice(0, at), revisionId: value.slice(at + 1) };
}

/** Every revision the server has confirmed serving, with the branch it named. */
type ServedState = Readonly<Record<string, string>>;

let served: ServedState = {};
const listeners = new Set<() => void>();

export function reportPreviewServed(headerValue: string | null | undefined): void {
  const parsed = parsePreviewHeader(headerValue);
  if (!parsed) return;
  if (served[parsed.revisionId] === parsed.branch) return;
  served = { ...served, [parsed.revisionId]: parsed.branch };
  for (const listener of listeners) listener();
}

export function subscribePreviewServed(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Whether the server has said it served a response from `revisionId`. */
export function isPreviewRevisionServed(revisionId: string): boolean {
  return revisionId in served;
}

/** Test seam: forget every confirmation. */
export function resetPreviewServed(): void {
  served = {};
  for (const listener of listeners) listener();
}

/**
 * `409 {"code":"cannot_compile"}` from create or refresh: the branch has
 * uncommitted edits in its worktree, or the workspace has no checkout. The
 * server's message says which and what to do, so it is shown where the person
 * asked (the row, the New preview field, the IDE button) — never as a generic
 * failure, and never as the read-only preview toast, which it is not.
 */
const CANNOT_COMPILE_CODE = "cannot_compile";

/** `404 {"code":"preview_not_found"}`: refresh of a branch never previewed. */
const PREVIEW_NOT_FOUND_CODE = "preview_not_found";

function apiErrorBody(
  error: unknown,
  status: number
): { code?: unknown; message?: unknown } | null {
  if (!isAxiosError(error) || error.response?.status !== status) return null;
  const data = error.response.data;
  return data && typeof data === "object" ? (data as { code?: unknown; message?: unknown }) : null;
}

/** The server's message for a `cannot_compile` refusal, or `null` for any other error. */
export function cannotCompileMessage(error: unknown): string | null {
  const body = apiErrorBody(error, 409);
  if (body?.code !== CANNOT_COMPILE_CODE) return null;
  return typeof body.message === "string" && body.message.trim()
    ? body.message
    : "This branch can't be compiled right now.";
}

export function isPreviewNotFound(error: unknown): boolean {
  return apiErrorBody(error, 404)?.code === PREVIEW_NOT_FOUND_CODE;
}

/** True when `error` is the `409` a preview answers to anything that runs or changes. */
export function isPreviewReadOnlyError(error: unknown): boolean {
  if (!isAxiosError(error)) return false;
  const data = error.response?.data as { code?: unknown } | undefined;
  return error.response?.status === 409 && data?.code === PREVIEW_READ_ONLY_CODE;
}

/** The server's own words for a preview refusal, with a fallback if it sent none. */
function previewReadOnlyMessage(body: unknown): string {
  const message = (body as { message?: unknown } | null | undefined)?.message;
  return typeof message === "string" && message.trim() ? message : FALLBACK_READ_ONLY_MESSAGE;
}

/** The body of a `409 preview_read_only`, or `null` for any other response. */
export function readPreviewReadOnlyBody(status: number | undefined, body: unknown): string | null {
  if (status !== 409) return null;
  const code = (body as { code?: unknown } | null | undefined)?.code;
  return code === PREVIEW_READ_ONLY_CODE ? previewReadOnlyMessage(body) : null;
}

/**
 * Held procedure runs (`OXY_PREVIEW_RUNS`): every route under
 * `…/previews/runs` answers this when the feature is off — not a per-request
 * failure, so the whole Runs panel should say so rather than showing a form
 * that will only ever refuse.
 */
const PREVIEW_RUNS_DISABLED_CODE = "preview_runs_disabled";

/** `409 {"code":"preview_not_ready"}`: no ready staging revision to run against yet. */
const PREVIEW_NOT_READY_CODE = "preview_not_ready";

/** `404 {"code":"ref_not_in_revision"}`: no automation definition at `ref` in the staging revision. */
const REF_NOT_IN_REVISION_CODE = "ref_not_in_revision";

export function isPreviewRunsDisabled(error: unknown): boolean {
  return apiErrorBody(error, 404)?.code === PREVIEW_RUNS_DISABLED_CODE;
}

/**
 * The server's fixed refusals to `POST …/previews/runs`, worded for the
 * "Dry-run a procedure" form. `null` for any other error — including
 * `400 bad_request`, which the UI can't provoke since it only ever sends
 * `kind: "procedure"` — so the caller falls back to a generic message.
 */
export function startRunErrorMessage(error: unknown): string | null {
  if (isPreviewRunsDisabled(error)) return "Preview runs aren't enabled on this deployment.";

  const notReady = apiErrorBody(error, 409);
  if (notReady?.code === PREVIEW_NOT_READY_CODE) return "The branch is still compiling.";

  const notFound = apiErrorBody(error, 404);
  if (notFound?.code === REF_NOT_IN_REVISION_CODE) {
    return "No procedure at that path on this branch.";
  }
  if (notFound?.code === PREVIEW_NOT_FOUND_CODE) {
    return "No preview exists for that branch yet — create one first.";
  }
  return null;
}

// ── Airway samples (S11) ─────────────────────────────────────────────────

/** `422 {"code":"sample_refused"}`: the source can't be sampled at all. */
const SAMPLE_REFUSED_CODE = "sample_refused";

/**
 * `422 {"code":"sample_unsupported"}` (fix round 2026-09-30): the pipeline
 * itself can never be sampled — Airway writes its own metadata tables in
 * `main`, which a preview can't write. Distinct from `sample_refused`, which
 * is about the *destination* (inline, wrong database, unconfined Airhouse);
 * this is about the pipeline's own bookkeeping.
 */
const SAMPLE_UNSUPPORTED_CODE = "sample_unsupported";

/** `409 {"code":"sandbox_required"}`: no `/sources` row for a rotate-on-use pipeline. */
const SANDBOX_REQUIRED_CODE = "sandbox_required";

const WINDOW_REQUIRED_CODE = "window_required";
const WINDOW_TOO_LONG_CODE = "window_too_long";
const WINDOW_NOT_SUPPORTED_CODE = "window_not_supported";
const RESOURCES_REQUIRED_CODE = "resources_required";
const UNKNOWN_RESOURCE_CODE = "unknown_resource";

/**
 * The server's fixed refusals to `POST …/previews/runs` with
 * `kind: "airway_sample"`, worded for the "Sample an Airway pipeline" form —
 * a sibling of `startRunErrorMessage` above, not a superset of it: the two
 * forms never show each other's copy, so `ref_not_in_revision` etc. get their
 * own pipeline-flavored wording here rather than sharing the procedure one.
 * Checked in the contract's own refusal order.
 */
export function sampleRunErrorMessage(error: unknown): string | null {
  if (isPreviewRunsDisabled(error)) return "Preview runs aren't enabled on this deployment.";

  const notReady = apiErrorBody(error, 409);
  if (notReady?.code === PREVIEW_NOT_READY_CODE) return "The branch is still compiling.";
  if (notReady?.code === SANDBOX_REQUIRED_CODE) {
    return "Register a sandbox source for this pipeline first.";
  }

  const notFound = apiErrorBody(error, 404);
  if (notFound?.code === REF_NOT_IN_REVISION_CODE) {
    return "No Airway pipeline at that path on this branch.";
  }
  if (notFound?.code === PREVIEW_NOT_FOUND_CODE) {
    return "No preview exists for that branch yet — create one first.";
  }

  const refused = apiErrorBody(error, 422);
  if (refused?.code === SAMPLE_REFUSED_CODE) {
    return "This source can't be sampled (replication slot or quota-limited).";
  }
  if (refused?.code === SAMPLE_UNSUPPORTED_CODE) {
    return "This pipeline can't be sampled yet: Airway writes its own metadata tables in main, which a preview can't write.";
  }

  const badRequest = apiErrorBody(error, 400);
  if (badRequest?.code === WINDOW_REQUIRED_CODE) {
    return "Enter both a start and an end for the window, start before end.";
  }
  if (badRequest?.code === WINDOW_TOO_LONG_CODE) {
    return "The window can't be longer than 31 days.";
  }
  if (badRequest?.code === WINDOW_NOT_SUPPORTED_CODE) {
    return "This source has no date window — clear the window and try again.";
  }
  if (badRequest?.code === RESOURCES_REQUIRED_CODE) {
    return "This source has more than one resource — choose which ones to sample.";
  }
  if (badRequest?.code === UNKNOWN_RESOURCE_CODE) {
    return "Unknown resource — check the name against what this source advertises.";
  }

  return null;
}

// ── Sandbox sources (S11) ────────────────────────────────────────────────

const PRODUCTION_VAR_CODE = "production_var";
const PRODUCTION_REALM_CODE = "production_realm";
const ROTATING_VAR_TAKEN_CODE = "rotating_var_taken";

/**
 * Fix round (2026-09-30): a var name containing `/`, an `apps/…` var, or a
 * var a custom app declares — none of those are sandbox-only, so a sample
 * run against them could touch a production or app-owned secret. Named to
 * match the sibling codes above; confirm the exact wire spelling against the
 * backend's contract update once it lands.
 */
const RESERVED_VAR_CODE = "reserved_var";

/** `409 {"code":"production_var"}`: a var a production QuickBooks pipeline names. */
export function productionVarMessage(error: unknown): string | null {
  const body = apiErrorBody(error, 409);
  if (body?.code !== PRODUCTION_VAR_CODE) return null;
  return "That variable is already used by a production QuickBooks pipeline — sandbox sources need their own secrets.";
}

/** `409 {"code":"production_realm"}`: a realm id a production QuickBooks pipeline names. */
export function productionRealmMessage(error: unknown): string | null {
  const body = apiErrorBody(error, 409);
  if (body?.code !== PRODUCTION_REALM_CODE) return null;
  return "That realm id is already used by a production QuickBooks pipeline — use a different sandbox company.";
}

/** `409 {"code":"rotating_var_taken"}`: another pipeline's sandbox already rotates that var. */
export function rotatingVarTakenMessage(error: unknown): string | null {
  const body = apiErrorBody(error, 409);
  if (body?.code !== ROTATING_VAR_TAKEN_CODE) return null;
  return "Another pipeline's sandbox source already rotates that variable — each one needs its own.";
}

/**
 * `409 {"code":"reserved_var"}`: a var name containing `/`, an `apps/…` var,
 * or a var a custom app declares — see `RESERVED_VAR_CODE`.
 */
export function reservedVarMessage(error: unknown): string | null {
  const body = apiErrorBody(error, 409);
  if (body?.code !== RESERVED_VAR_CODE) return null;
  return "Use a sandbox-only secret name, not a production or custom-app secret.";
}

/**
 * Any of the four named `PUT …/previews/sources` conflicts — used by
 * `useUpsertPreviewSource` to tell "the form already showed this" apart from
 * an error that still needs a generic toast. `400 bad_request` (malformed
 * overrides, wrong environment, a `preview:` pipeline name) is not among
 * them: the form validates those itself before it can reach the server.
 */
export function saveSourceErrorMessage(error: unknown): string | null {
  return (
    productionRealmMessage(error) ??
    productionVarMessage(error) ??
    rotatingVarTakenMessage(error) ??
    reservedVarMessage(error)
  );
}
