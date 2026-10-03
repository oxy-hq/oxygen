/**
 * The `params` a `useQuery` SQL template names, as they travel to the server.
 *
 * A template refers to a param two ways:
 *
 * - `{{ params.X | sqlquote }}` — a string is written as a SQL string literal,
 *   a number or boolean as itself, a nullish value as `NULL`.
 * - `{{ params.X }}` — the value as it is, with no quoting. For a number, or an
 *   identifier the caller has validated. The caller is responsible for it.
 *
 * The **server** substitutes them, not this module. How a quote or a backslash
 * is written inside a `'…'` literal depends on the engine that reads it —
 * DuckDB and Postgres read `''` and nothing else, ClickHouse, MySQL, Snowflake
 * and Redshift also read a backslash as an escape, and BigQuery reads a
 * backslash and does not read `''` at all — and only the server knows which
 * engine the query's `database` is. The client used to double `'` and send the
 * finished SQL, so a value holding a backslash broke the query, or changed what
 * it asked, on every engine but the first two.
 *
 * Not a security boundary either way: the query endpoint runs whatever
 * read-only SQL a signed-in member sends it. This is about the value a user
 * typed reaching the warehouse as that value.
 */

/** A value a `useQuery` template can bind. */
export type QueryParam = string | number | boolean | null | undefined;

/** A param as JSON carries it: `undefined` and a non-finite number are `null`. */
export type SentQueryParam = string | number | boolean | null;

const PLACEHOLDER = /\{\{\s*params\.([a-zA-Z0-9_]+)(?:\s*\|\s*sqlquote)?\s*\}\}/g;

/**
 * The params `sql` names, ready to send beside it — or `undefined` when `sql`
 * has no placeholder, so a plain query's request is the same as it always was.
 *
 * Only the names the template uses are sent, in sorted order, so the result is
 * a stable cache key. A name the caller did not supply is sent as `null`, which
 * the server writes as `NULL`.
 */
export function paramsToSend(
  sql: string,
  params: Record<string, QueryParam>
): Record<string, SentQueryParam> | undefined {
  const names = new Set<string>();
  for (const match of sql.matchAll(PLACEHOLDER)) {
    if (match[1]) names.add(match[1]);
  }
  if (names.size === 0) return undefined;

  // Own entries only, so a template naming `constructor` reads no inherited member.
  const supplied = new Map(Object.entries(params));
  const sent: Record<string, SentQueryParam> = {};
  for (const name of [...names].sort()) {
    sent[name] = sendable(supplied.get(name));
  }
  return sent;
}

function sendable(value: QueryParam): SentQueryParam {
  if (value === undefined || value === null) return null;
  // JSON has no NaN or Infinity; say `null` here rather than let the cache key
  // and the request body disagree about it.
  if (typeof value === "number" && !Number.isFinite(value)) return null;
  return value;
}
