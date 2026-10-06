/**
 * `oxyc routes`, `oxyc schema`, `oxyc openapi` — how a caller with no checkout
 * finds out what it may ask for.
 *
 * These are the commands the tool exists for. An agent debugging a customer's
 * data has the binary and a token and nothing else: no repo, no doc site, no
 * running server it can browse. Everything it needs to construct a correct
 * request has to be reachable from here.
 */

import {
  type Catalog,
  type CatalogRoute,
  loadCatalog,
  loadOpenApi,
  searchRoutes
} from "../api/catalog.js";
import type { Context } from "../context/resolve.js";
import { heading, table, wrap } from "../ui/render.js";
import { out, stdoutIsTty } from "../ui/tty.js";
import { CliError, ExitCode } from "../util/errors.js";

export interface RoutesFlags {
  json?: boolean;
  refresh?: boolean;
  /** Include `ide-only` and `worker-only` routes, which most callers cannot use. */
  all?: boolean;
}

/**
 * List the endpoints this deployment mounts.
 *
 * UNFILTERED OUTPUT IS A TABLE; a filter buys the prose. That is not a
 * cosmetic choice — the full surface is ~600 routes, and printing each one's
 * description unfiltered produces something no human scrolls through and no
 * agent should be made to read. Narrowing is what makes the prose affordable,
 * which is also why the error for "no matches" points back at the filter.
 */
export async function runRoutes(
  ctx: Context,
  filter: string | undefined,
  flags: RoutesFlags
): Promise<void> {
  const catalog = await loadCatalog({
    target: ctx.target(),
    bearer: await ctx.maybeBearer(),
    refresh: flags.refresh
  });

  let matches = searchRoutes(catalog, filter);
  if (!flags.all) {
    // `ide-only` and `worker-only` routes are mounted on one instance of the
    // fleet. They are real, but a caller hitting the load balancer will get a
    // 421 or a forward, so listing them alongside the rest as if they were
    // equally reachable is misleading. `--all` is the escape hatch.
    matches = matches.filter((r) => r.role === "fleet-ok");
  }

  if (flags.json) {
    // A JSON ARRAY — one document, not one object per line. Both readings are
    // defensible and `--help` used to claim the other one; an array is what
    // `oxyc list --json` and `oxyc openapi` already emit, and what a `| jq`
    // written against this expects.
    //
    // Indented for a human, compact for a pipe, exactly as `runOpenApi` below
    // decides it. Unfiltered this is ~670 objects of eight fields, where the
    // indentation alone is tens of KB of context an agent pays for and no
    // human is reading.
    process.stdout.write(`${JSON.stringify(matches, null, stdoutIsTty() ? 2 : 0)}\n`);
    return;
  }

  if (matches.length === 0) {
    throw new CliError(`no route matches ${JSON.stringify(filter ?? "")}`, {
      code: ExitCode.NOT_FOUND,
      hint: "oxyc routes            — the full list\noxyc routes --all      — including ide-only and worker-only mounts"
    });
  }

  // A filter narrow enough to read in full gets the prose; a broad one gets
  // the compact table. 40 is where a screenful stops being a screenful.
  //
  // NEITHER ARM REFUSES, and `oxy_routes` mirrors that (`mcp.ts`): a broad
  // match degrades to method, path and credential rather than dead-ending a
  // caller who filtered in good faith. `credential` survives there for the same
  // reason `renderCompact` keeps it in each surface heading below — it is what
  // separates the bearer mount from the API-key one. The MCP threshold is
  // higher because a context window is not a screen.
  const verbose = Boolean(filter) && matches.length <= 40;
  process.stdout.write(
    verbose ? renderDetailed(catalog, matches) : renderCompact(catalog, matches)
  );
  process.stdout.write("\n");

  if (!filter) {
    process.stderr.write(
      `\n${out.dim(`${matches.length} routes. Narrow with \`oxyc routes <filter>\` to see what each one does.`)}\n`
    );
  }
}

/** The compact grouped table: method and path, grouped by surface. */
function renderCompact(catalog: Catalog, matches: CatalogRoute[]): string {
  const sections: string[] = [];
  for (const surface of surfacesOf(catalog, matches)) {
    const group = matches.filter((r) => r.surface === surface.id);
    if (group.length === 0) continue;
    sections.push(heading(`${surface.label} — ${surface.credential}`));
    sections.push(
      table(group, [
        { header: "METHOD", value: (r) => r.method },
        { header: "PATH", value: (r) => r.path }
      ])
    );
  }
  return sections.join("\n");
}

/** Method, path, what it does, and the mount comment that explains why. */
function renderDetailed(catalog: Catalog, matches: CatalogRoute[]): string {
  const lines: string[] = [];
  for (const surface of surfacesOf(catalog, matches)) {
    const group = matches.filter((r) => r.surface === surface.id);
    if (group.length === 0) continue;
    lines.push(heading(`${surface.label} — ${surface.credential}`));
    for (const route of group) {
      lines.push(`  ${out.bold(route.method.padEnd(7))} ${route.path}`);
      for (const line of wrap(route.description, 84)) lines.push(`          ${line}`);
      // Marked as a note, because a mount comment is not always *about* its
      // mount — it may be explaining the route above it, or one since removed.
      wrap(route.note, 78).forEach((line, i) => {
        lines.push(`          ${i === 0 ? "note: " : "      "}${line}`);
      });
      if (route.path_parameters.length > 0) {
        lines.push(`          ${out.dim(`params: ${route.path_parameters.join(", ")}`)}`);
      }
    }
  }
  return lines.join("\n");
}

/** Surfaces in the catalog's own display order, falling back to first-seen. */
function surfacesOf(catalog: Catalog, matches: CatalogRoute[]) {
  if (catalog.surfaces.length > 0) return catalog.surfaces;
  const seen: string[] = [];
  for (const route of matches) if (!seen.includes(route.surface)) seen.push(route.surface);
  return seen.map((id) => ({ id, label: id, credential: "" }));
}

/**
 * `oxyc schema <path>` — the request and response shapes for one endpoint.
 *
 * This is the piece the Rust CLI never had: `--openapi` printed the whole
 * document and left the caller to find their operation in it. An agent
 * building a request body needs one operation's schema, and dumping 47 of them
 * to find it costs more context than the request it is trying to make.
 */
export async function runSchema(
  ctx: Context,
  rawPath: string,
  method: string | undefined
): Promise<void> {
  const doc = (await loadOpenApi({ target: ctx.target(), bearer: await ctx.maybeBearer() })) as {
    paths?: Record<string, Record<string, unknown>>;
    components?: unknown;
  };

  const paths = doc.paths ?? {};

  // Exact match on the comparable form first, then a substring on the literal
  // segments — an agent that types `threads` should get the thread endpoints
  // rather than an error about a path that is almost right.
  const wanted = comparablePath(rawPath);
  const exact = Object.entries(paths).filter(([p]) => comparablePath(p) === wanted);
  const candidates =
    exact.length > 0
      ? exact
      : Object.entries(paths).filter(([p]) =>
          literalSegments(p).includes(literalSegments(rawPath))
        );

  if (candidates.length === 0) {
    throw new CliError(`no documented schema for ${rawPath}`, {
      code: ExitCode.NOT_FOUND,
      hint:
        "the OpenAPI document covers a curated subset of the surface.\n" +
        `     \`oxyc routes ${rawPath}\` shows whether the endpoint exists at all.`
    });
  }

  const selected: Record<string, unknown> = {};
  for (const [path, operations] of candidates) {
    const filtered = method
      ? Object.fromEntries(
          Object.entries(operations).filter(([verb]) => verb.toLowerCase() === method.toLowerCase())
        )
      : operations;
    if (Object.keys(filtered).length > 0) selected[path] = filtered;
  }

  if (Object.keys(selected).length === 0) {
    throw new CliError(`no ${method?.toUpperCase()} operation documented for ${rawPath}`, {
      code: ExitCode.NOT_FOUND,
      hint: `oxyc schema ${rawPath}   — without --method, to see which verbs are documented`
    });
  }

  // `components` rides along because schemas are `$ref`-heavy and a body
  // description full of unresolvable references is not a description.
  process.stdout.write(
    `${JSON.stringify({ paths: selected, components: doc.components }, null, 2)}\n`
  );
}

/**
 * A path reduced to what two spellings of the same endpoint have in common.
 *
 * TWO MISMATCHES, both of which made the obvious implementation useless
 * against the real document:
 *
 *  1. **No `/api` prefix.** OpenAPI carries it in `servers`, so the document
 *     says `/{workspace_id}/agents` while every other surface of this CLI —
 *     and every caller — says `/api/{workspace_id}/agents`. Normalising the
 *     request path *up* to `/api/...` guaranteed a miss on every lookup.
 *  2. **Different placeholder names.** The document says `{workspace_id}`;
 *     this CLI's own placeholder is `{workspace}`, and `oxyc schema
 *     {workspace}/threads` is exactly what someone copies from `oxyc api`.
 *
 * So both sides are reduced: the `/api` prefix dropped, and every `{...}`
 * collapsed to `{}`. The names inside braces carry no information a lookup
 * needs — position does.
 */
export function comparablePath(path: string): string {
  return path
    .trim()
    .replace(/^\/+/, "/")
    .replace(/^(?!\/)/, "/")
    .replace(/^\/api(?=\/|$)/, "")
    .replace(/\{[^}]*\}/g, "{}")
    .replace(/\/+$/, "")
    .toLowerCase();
}

/** The literal (non-placeholder) part of a path, for a fuzzy substring match. */
export function literalSegments(path: string): string {
  return comparablePath(path).replace(/\{\}/g, "");
}

/** `oxyc openapi` — the whole document, for piping into jq. */
export async function runOpenApi(ctx: Context): Promise<void> {
  const doc = await loadOpenApi({ target: ctx.target(), bearer: await ctx.maybeBearer() });
  process.stdout.write(`${JSON.stringify(doc, null, stdoutIsTty() ? 2 : 0)}\n`);
}
