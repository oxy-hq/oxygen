/**
 * Helpers for schema-qualified table identifiers in the Database sidebar.
 *
 * The airhouse connector reports tables as `"schema"."table"` (both identifiers
 * double-quoted, internal `"` doubled) so every reference is runnable across
 * schemas. Other connectors report a bare table name with no schema. These
 * helpers split that qualified name for grouping/display and re-quote an
 * identifier for DDL.
 */

/** Split a possibly schema-qualified table name into `{ schema, label }`.
 *  Returns `schema: null` (and the whole string as `label`) for bare names. */
export function parseQualifiedName(name: string): { schema: string | null; label: string } {
  const m = name.match(/^"((?:[^"]|"")*)"\."((?:[^"]|"")*)"$/);
  if (m) {
    const unquote = (s: string) => s.replace(/""/g, '"');
    return { schema: unquote(m[1]), label: unquote(m[2]) };
  }
  return { schema: null, label: name };
}
