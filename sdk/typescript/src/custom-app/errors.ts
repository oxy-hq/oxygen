// Friendly interpretation of the errors a custom-app bundle can hit
// at startup. The bundle catches an `Error` thrown by
// `loadCustomAppManifest` or `useQuery`, hands it to
// `interpretCustomAppError`, and renders the returned struct as a
// proper error page — instead of dumping a raw exception that asks
// the developer to learn the internal contract from a stack trace.
//
// Every interpretation includes:
//   - `title`: short headline ("Manifest not found")
//   - `message`: the underlying technical message (the raw err.message)
//   - `hint`: an actionable next step ("commit public/oxy-app.json and rebuild")
//   - `docs`: a pointer to the relevant section of the architecture doc
//
// Add cases as we hit new failure modes in the wild — the catch-all
// keeps the surface safe in the meantime.

// ── API error types ─────────────────────────────────────────────────────────

/**
 * Error thrown by all custom-app hooks when an API call returns a
 * non-2xx response. Carries the structured `code` + `hint` the server
 * emits so bundle UIs can render an actionable message instead of
 * "404: { ...json... }".
 */
export class OxyApiError extends Error {
  readonly status: number;
  readonly code: string | null;
  readonly hint: string | null;
  constructor(opts: {
    status: number;
    message: string;
    code?: string | null;
    hint?: string | null;
  }) {
    const base = opts.message || `HTTP ${opts.status}`;
    const code = opts.code ? ` [${opts.code}]` : "";
    const hint = opts.hint ? `\n\n${opts.hint}` : "";
    super(`${base}${code}${hint}`);
    this.name = "OxyApiError";
    this.status = opts.status;
    this.code = opts.code ?? null;
    this.hint = opts.hint ?? null;
  }
}

/**
 * Read a non-2xx response from oxy and return an `OxyApiError`.
 * Parses the JSON envelope when present; falls back to raw text
 * (truncated to 240 chars so a runaway HTML error page doesn't
 * dominate the bundle UI).
 */
export async function apiErrorFromResponse(resp: Response): Promise<OxyApiError> {
  let body: unknown;
  let raw = "";
  try {
    raw = await resp.text();
    body = raw ? JSON.parse(raw) : undefined;
  } catch {
    // Non-JSON body — keep `raw` for the fallback path.
  }
  if (body && typeof body === "object") {
    const b = body as { message?: unknown; code?: unknown; hint?: unknown };
    return new OxyApiError({
      status: resp.status,
      message: typeof b.message === "string" ? b.message : `HTTP ${resp.status}`,
      code: typeof b.code === "string" ? b.code : null,
      hint: typeof b.hint === "string" ? b.hint : null
    });
  }
  const snippet = raw.length > 240 ? `${raw.slice(0, 237)}…` : raw;
  return new OxyApiError({
    status: resp.status,
    message: snippet || `HTTP ${resp.status}`
  });
}

// ── Bundle-startup error interpretation ─────────────────────────────────────

/**
 * Normalize a rejected request into the `Error` a hook reports.
 *
 * An `AbortError` reaching a hook's `.catch` is by construction an abort the
 * hook did NOT cause — its own teardown is marked separately — so it is a
 * fetcher's request timeout, a dropped socket, or a navigation. The platform's
 * wording for those ("The user aborted a request.") is wrong on its face by
 * the time a bundle renders it, so rephrase and keep the original as `cause`.
 *
 * The name is used only to WORD an error already being reported. Whether to
 * report at all is the caller's teardown flag, never the name — that
 * conflation is what left hooks loading forever.
 */
export function asReportableError(err: unknown): Error {
  // Read the name off the ORIGINAL value: a `DOMException` is not an `Error`
  // in every realm (it is not under jsdom), and normalizing first would fold
  // the name into a message string and lose it.
  const name =
    typeof err === "object" && err !== null && "name" in err
      ? String((err as { name: unknown }).name)
      : "";
  const e = err instanceof Error ? err : new Error(String(err));
  if (name !== "AbortError" && e.name !== "AbortError") return e;
  return Object.assign(new Error("request was interrupted (the connection dropped or timed out)"), {
    cause: e
  });
}

export interface CustomAppErrorReport {
  title: string;
  message: string;
  hint: string;
  docs?: string;
}

const ARCH_DOC = "internal-docs/customer-apps.md";

/** Interpret a thrown error as a structured report for UI display. */
export function interpretCustomAppError(err: unknown): CustomAppErrorReport {
  const message = err instanceof Error ? err.message : String(err);

  // Order matters — earlier matches take priority. Use specific
  // substrings that the loader / fetcher actually emit so this stays
  // grep-discoverable from both ends.

  if (/Failed to load oxy-app\.json.*HTTP 404/.test(message)) {
    return {
      title: "Manifest not found",
      message,
      hint:
        "The bundle is being served, but oxy-app.json was not. Check that " +
        "public/oxy-app.json is committed in the custom-app repo and " +
        "that the build copied it into the static output. If you're using " +
        "Next.js, anything under public/ is auto-copied to out/.",
      docs: ARCH_DOC
    };
  }

  if (/Failed to load oxy-app\.json/.test(message)) {
    return {
      title: "Manifest could not be loaded",
      message,
      hint:
        "Network error fetching the manifest. Confirm the bundle is being " +
        "served from a path that matches OXY_APP_BASE_PATH at " +
        "build time — a mismatch causes assets and the manifest to 404.",
      docs: ARCH_DOC
    };
  }

  if (/schemaVersion/i.test(message)) {
    return {
      title: "Manifest schema mismatch",
      message,
      hint:
        "This bundle was built against a different version of the " +
        "data-product contract than the SDK it ships. Rebuild the bundle " +
        "with a compatible @oxy-hq/sdk version.",
      docs: ARCH_DOC
    };
  }

  // Query proxy responses. The `${status}: ${body}` shape comes from
  // useQuery — body is the server's `{ "message": "..." }` JSON for
  // structured errors, or raw text for unstructured ones.

  if (/^401:/m.test(message)) {
    return {
      title: "Session expired",
      message,
      hint: "Reload the page to re-authenticate via oxy's session cookie.",
      docs: ARCH_DOC
    };
  }

  if (/^403:.*origin not allowed/im.test(message)) {
    return {
      title: "Request origin not allowed",
      message,
      hint:
        "The bundle's host isn't in oxy's OXY_ALLOWED_ORIGINS. " +
        "Production: add the bundle's serving origin to the env var. " +
        "Local dev: oxy auto-allows http://localhost:5173 and :5174.",
      docs: ARCH_DOC
    };
  }

  if (/^403:.*not a member/im.test(message)) {
    return {
      title: "Access denied",
      message,
      hint:
        "Your account isn't a member of the org that owns this project. " +
        "Ask an org owner to add you.",
      docs: ARCH_DOC
    };
  }

  if (/^403:.*SELECT.*WITH/im.test(message)) {
    return {
      title: "Query rejected — read-only endpoint",
      message,
      hint:
        "This proxy only runs SELECT or WITH queries. Mutations " +
        "(INSERT/UPDATE/DELETE/DROP) are not allowed from custom-app " +
        "bundles.",
      docs: ARCH_DOC
    };
  }

  if (/^403:/m.test(message)) {
    return {
      title: "Access denied",
      message,
      hint: "The request was rejected by the server. Check the oxy server " + "logs for details.",
      docs: ARCH_DOC
    };
  }

  if (/^404:/m.test(message) && /project/i.test(message)) {
    return {
      title: "Project not found",
      message,
      hint:
        "The projectId in oxy-app.json doesn't match any registered " +
        "project. Confirm the manifest's projectId is a real UUID for " +
        "this deployment.",
      docs: ARCH_DOC
    };
  }

  if (/^400:.*sql.*must be non-empty/im.test(message)) {
    return {
      title: "Empty SQL",
      message,
      hint:
        "useQuery was called with an empty or whitespace-only `sql`. " +
        "Pass a real query, or set `enabled: false` to skip the call.",
      docs: ARCH_DOC
    };
  }

  if (/^400:/m.test(message) && /query failed/i.test(message)) {
    return {
      title: "Query failed",
      message,
      hint:
        "The SQL ran but the warehouse rejected it. Full error in the " +
        "oxy server logs (look for the projects::query span).",
      docs: ARCH_DOC
    };
  }

  if (/^502:/m.test(message)) {
    return {
      title: "Warehouse unreachable",
      message,
      hint:
        "Oxy couldn't reach the configured database. Check connector " +
        "config + warehouse health.",
      docs: ARCH_DOC
    };
  }

  // "Unexpected token '<', '<!doctype '..." — the bundle asked for JSON
  // at a path that oxy resolved to its SPA-fallback HTML.
  //
  // In v2 the single most likely cause is: the built bundle is stale.
  // It was built against an older SDK whose useQuery / fetchers pointed
  // at endpoints that no longer exist (e.g. the deleted /products/...
  // route), so the request resolved to the SPA fallback and returned
  // index.html where the bundle expected a JSON body.
  if (/Unexpected token '<'|<!doctype/i.test(message)) {
    return {
      title: "Fetched HTML where JSON was expected",
      message,
      hint:
        "Most likely the built bundle is stale — built against an old " +
        "SDK whose endpoints no longer exist on the server. Rebuild the " +
        "bundle (vite build) with @oxy-hq/sdk@^2.0.0 and reload. If " +
        "the bundle is current, check that OXY_APP_BASE_PATH matches " +
        "the path the custom-app row is served at.",
      docs: ARCH_DOC
    };
  }

  // Catch-all — surfaces the raw message but with a generic next step.
  return {
    title: "Unexpected error loading the dashboard",
    message,
    hint:
      "Check the browser console for the full stack trace, and the oxy " +
      "server logs for the corresponding request.",
    docs: ARCH_DOC
  };
}
