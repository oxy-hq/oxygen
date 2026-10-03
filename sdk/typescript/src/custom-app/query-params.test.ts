// What `useQuery` sends for a SQL template's `{{ params.X }}` placeholders.
//
// The client does not write a value into the SQL any more — the server does,
// by the rule of the engine that reads it. So these tests are about what is
// sent, and that a value is sent as the characters it holds.

import { describe, expect, it } from "vitest";
import { paramsToSend } from "./query-params";

describe("paramsToSend", () => {
  it("sends nothing for SQL with no placeholder", () => {
    expect(paramsToSend("SELECT 1", { unused: "val" })).toBeUndefined();
    // Jinja the template does not own is not a placeholder.
    expect(paramsToSend("SELECT '{{ controls.x }}'", { x: "val" })).toBeUndefined();
  });

  it("sends the value of each param the template names", () => {
    const sql =
      "SELECT * FROM t WHERE store = {{ params.store | sqlquote }} AND year = {{ params.year }}";
    expect(paramsToSend(sql, { store: "NY", year: 2024, unused: "x" })).toEqual({
      store: "NY",
      year: 2024
    });
  });

  it("reads both placeholder forms, whatever their spacing", () => {
    for (const sql of [
      "{{params.a}}",
      "{{ params.a }}",
      "{{params.a|sqlquote}}",
      "{{  params.a  |  sqlquote  }}"
    ]) {
      expect(paramsToSend(sql, { a: true })).toEqual({ a: true });
    }
  });

  // The server cannot quote for an engine a value it never saw: a backslash,
  // a quote, the two together and a trailing backslash each broke out of the
  // `''`-doubled literal the client used to write on some engine.
  it("sends a string as its characters, unescaped and unquoted", () => {
    const sql = "SELECT * FROM t WHERE name = {{ params.name | sqlquote }}";
    for (const name of ["O'Brien", "a\\b", "x\\' OR 1=1 -- ", "C:\\", "{{ params.other }}"]) {
      expect(paramsToSend(sql, { name })).toEqual({ name });
    }
  });

  it("sends null for a nullish or missing param, which the server writes as NULL", () => {
    const sql = "SELECT {{ params.a | sqlquote }}, {{ params.b }}, {{ params.c }}";
    expect(paramsToSend(sql, { a: null, b: undefined })).toEqual({ a: null, b: null, c: null });
  });

  it("sends null for a number JSON cannot carry", () => {
    const sql = "SELECT {{ params.a }}, {{ params.b }}";
    expect(paramsToSend(sql, { a: Number.NaN, b: Number.POSITIVE_INFINITY })).toEqual({
      a: null,
      b: null
    });
  });

  it("does not read an inherited member for a param named like one", () => {
    expect(paramsToSend("SELECT {{ params.constructor }}", {})).toEqual({ constructor: null });
  });

  it("is the same key whatever order the template or the caller names params in", () => {
    const a = paramsToSend("{{ params.b }} {{ params.a }}", { a: 1, b: 2 });
    const b = paramsToSend("{{ params.a }} {{ params.b }} {{ params.a }}", { b: 2, a: 1 });
    expect(JSON.stringify(a)).toBe(JSON.stringify(b));
    expect(JSON.stringify(a)).toBe('{"a":1,"b":2}');
  });
});
