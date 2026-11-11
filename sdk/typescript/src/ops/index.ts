// `@oxy-hq/sdk/ops` — the operating graph, applied inside an Oxy Function.
//
// The platform states where a caller may act (`ctx.user.reach`, derived from
// their assignments); this is
// how a function applies it. Library code, versioned with the SDK, and
// replaceable: an app that needs a tighter rule wraps `reachOf` and keeps the
// rest. It may tighten, never widen — the reach the platform hands in is the
// ceiling.
//
// Lifted from Store Ops's `functions/access.ts` (customer-apps #138) minus its
// roster SQL: the roster is the platform's now, so the decision arrives on
// `ctx.user` and nothing here reads a table.
//
// Two entry points. `requireReach` for a writer that names a location: the
// 403 to return, or null. `predicate` for a reader: a SQL expression over the
// statement's own location column, bound through the statement's parameter
// list, so the scope lives in the WHERE — before the LIMIT, so a page is a
// page of the right set.

import type { OxyReach } from "../custom-app/function-context";

/** What `ctx.user.reach` carries — one shape, declared once on the context type. */
export type Reach = OxyReach;

/** The slice of `ctx` this module reads — a test hands in exactly this. */
export interface ReachCtx {
  user: {
    /**
     * Present on every invocation since SDK 2.12 / the operating graph. Typed
     * optional here so a function can be written before its server is
     * upgraded: an absent reach is the fail-closed answer, never the office.
     */
    reach?: Reach | null;
    appRole?: string;
    kind?: string;
  };
}

const NOWHERE: Reach = { everywhere: false, via: null, locations: [] };

/**
 * The caller's reach. Synchronous — the platform decided it before the
 * function ran — but `await reachOf(ctx)` still works, so code written against
 * the app-side version keeps compiling.
 *
 * Absent reach (an older server) lands on nowhere. Not "system reaches
 * everywhere, everyone else nowhere": a system invocation on an older server
 * carries no reach either, and the one thing this module must never do is
 * invent a wider answer than the platform gave.
 */
export function reachOf(ctx: ReachCtx): Reach {
  const r = ctx.user.reach;
  if (!r || typeof r !== "object" || !Array.isArray(r.locations)) return NOWHERE;
  return { everywhere: r.everywhere === true, via: r.via ?? null, locations: [...r.locations] };
}

export function reaches(reach: Reach, locationId: string): boolean {
  return reach.everywhere || reach.locations.includes(locationId);
}

/**
 * For a writer naming a location: the 403 to return, or null to proceed.
 *
 * The message names the location and not the roster, and the code is stable:
 * the client tells "you are not rostered here" from "no such location" (a 404
 * the writers answer BEFORE this) and from "admin only" (a 403 with another
 * code) without parsing English.
 */
export function requireReach(ctx: ReachCtx, locationId: string): Response | null {
  if (reaches(reachOf(ctx), locationId)) return null;
  return Response.json(
    { error: `you are not rostered at ${locationId}`, code: "OutOfReach", locationId },
    { status: 403 }
  );
}

/**
 * For a reader: a SQL expression that is true for rows in reach, over the
 * given location column, binding through `params`.
 *
 * `TRUE` for a caller who reaches everywhere, so the statement reads the same
 * either way. Otherwise the locations go in as ONE comma-joined text parameter
 * unpacked by `string_to_array` — a location id is a uuid or a slug and can
 * never contain the separator. An empty list becomes an empty array, so a
 * caller assigned nowhere matches nothing rather than everything: the two
 * states are different values, not one sentinel doing double duty.
 */
export function predicate(reach: Reach, column: string, params: unknown[]): string {
  if (reach.everywhere) return "TRUE";
  params.push(reach.locations.join(","));
  // `::text` on the column: `string_to_array` yields `text[]`, and Postgres
  // will not compare a `uuid` column to it on its own (`uuid = text` has no
  // operator). The platform's location ids ARE uuids, so a tenant storing
  // them in the natural column type would otherwise get a reader that never
  // runs. A text column is unaffected.
  return `${column}::text = ANY(string_to_array($${params.length}::text, ','))`;
}

/**
 * The app-admin gate, decided in one place. `appRole` is derived by the
 * platform from app-admin standing, and is the ONE field on `ctx.user` a function
 * may gate on; `orgRole` and `teams` are there to explain, not to decide.
 */
export function isAdmin(ctx: Pick<ReachCtx, "user">): boolean {
  return (ctx.user.appRole ?? "") === "admin";
}

export function adminOnly(what: string): Response {
  return Response.json(
    { error: `${what} needs app-admin standing`, code: "AdminOnly" },
    { status: 403 }
  );
}
