/**
 * JSON Schema from TypeScript type declarations — the SIMPLE CASE, without a
 * compiler.
 *
 * `oxyc publish` writes a function's `inputSchema` / `outputSchema` from its
 * types (`publish/function-schemas.ts`), and app-event payloads are meant to be
 * produced the same way later; this module is the part both share. It reads
 * source TEXT: no type checker, no `typescript` dependency, which a compiled
 * `oxyc` binary could not carry with its lib files anyway. What it understands
 * is a deliberate subset, and anything outside it is an `UnsupportedType`
 * naming the construct and its line — never a guess:
 *
 * - `interface` (with `extends` of another declared interface) and `type`
 *   aliases, declared in the file or imported from a relative file by name;
 * - `string`, `number`, `boolean`, `null`, `unknown` / `any` (anything),
 *   `object`, `Date` (an RFC 3339 string, as JSON carries one);
 * - string, number and boolean literals, and unions of them (`enum`);
 * - unions (`T | null`, `A | B`), with `| undefined` read as optional;
 * - `T[]`, `Array<T>`, `ReadonlyArray<T>`, `readonly T[]`, `Readonly<T>`;
 * - object literal types, optional (`?`) and `readonly` members, a string
 *   index signature (`[key: string]: T`), `Record<string, T>` and
 *   `Record<"a" | "b", T>`;
 * - JSDoc: the text is the `description`; the tags `@minimum`, `@maximum`,
 *   `@exclusiveMinimum`, `@exclusiveMaximum`, `@multipleOf`, `@minLength`,
 *   `@maxLength`, `@minItems`, `@maxItems`, `@pattern`, `@format`, `@default`,
 *   `@deprecated` and `@asType integer` become the keywords they name — the
 *   spelling `ts-json-schema-generator` reads in `manifest-types.ts`.
 *
 * Not understood: generics, intersections, tuples, `enum` declarations,
 * template literal types, mapped / conditional / utility types other than the
 * ones above, recursive types, and anything from a package import. Objects are
 * closed (`additionalProperties: false`), as the manifest's own schema is.
 *
 * The output is the subset `@oxy-hq/sdk/manifest`'s `checkFunctionSchema`
 * accepts: no `$ref`, every declaration inlined.
 */

import { dirname, join, normalize } from "node:path";
import { maskJs } from "../publish/placement-scan.js";

/** A JSON Schema object, as this module writes one. */
export type JsonSchema = Record<string, unknown>;

/** The construct is outside the subset; `line` is 1-based in `file`. */
export class UnsupportedType extends Error {
  constructor(
    message: string,
    readonly file: string,
    readonly line: number
  ) {
    super(message);
    this.name = "UnsupportedType";
  }
}

export const SCHEMA_DIALECT = "https://json-schema.org/draft/2020-12/schema";

// ── Modules: one source file's declarations and imports ─────────────────────

/** A top-level `interface` or `type` in one module. */
export interface TypeDeclaration {
  name: string;
  kind: "interface" | "alias";
  exported: boolean;
  /** Offset of the body: an interface's `{`, an alias's first type token. */
  start: number;
  /** Interface `extends` names, as written. */
  heritage: string[];
  generic: boolean;
  doc?: JsDoc;
  line: number;
}

/** One source file, read for its types. */
export interface TypeModule {
  path: string;
  source: string;
  /** `source` with comments, strings and regexes blanked, offsets kept. */
  masked: string;
  declarations: Map<string, TypeDeclaration>;
  /** Local binding → the module it names and the name exported there. */
  imports: Map<string, { specifier: string; name: string }>;
}

/** Reads a file by absolute path; `undefined` when it cannot or must not. */
export type ReadSource = (path: string) => string | undefined;

/** Depth of `{` nesting at every offset of `masked`. */
function braceDepths(masked: string): Int32Array {
  const depths = new Int32Array(masked.length + 1);
  let depth = 0;
  for (let i = 0; i < masked.length; i++) {
    depths[i] = depth;
    const c = masked[i];
    if (c === "{") depth++;
    else if (c === "}") depth = Math.max(0, depth - 1);
  }
  depths[masked.length] = depth;
  return depths;
}

/** 1-based line of `offset`. */
export function lineOf(text: string, offset: number): number {
  let line = 1;
  for (let i = 0; i < offset && i < text.length; i++) if (text[i] === "\n") line++;
  return line;
}

/** Offset just past the bracket matching the one at `open`, in masked text. */
export function matchBracket(masked: string, open: number): number | undefined {
  const pairs: Record<string, string> = { "{": "}", "(": ")", "[": "]", "<": ">" };
  const close = pairs[masked[open] ?? ""];
  if (!close) return undefined;
  const opener = masked[open] as string;
  let depth = 0;
  for (let i = open; i < masked.length; i++) {
    const c = masked[i];
    if (c === opener) depth++;
    else if (c === close) {
      // `=>` is not a closing angle.
      if (close === ">" && masked[i - 1] === "=") continue;
      depth--;
      if (depth === 0) return i + 1;
    }
  }
  return undefined;
}

/** The JSDoc block ending just before `offset` (whitespace and `export` aside). */
function docBefore(source: string, offset: number): JsDoc | undefined {
  let i = offset - 1;
  while (i >= 0 && /\s/.test(source[i] as string)) i--;
  if (source.slice(i - 1, i + 1) !== "*/") return undefined;
  const open = source.lastIndexOf("/**", i);
  if (open < 0) return undefined;
  return parseJsDoc(source.slice(open, i + 1));
}

/** Parse one file's top-level type declarations and named relative imports. */
export function readTypeModule(path: string, source: string): TypeModule {
  const masked = maskJs(source);
  const depths = braceDepths(masked);
  const declarations = new Map<string, TypeDeclaration>();
  const imports = new Map<string, { specifier: string; name: string }>();

  const decl = /(^|[;\s}])(export\s+)?(?:declare\s+)?(interface|type)\s+([A-Za-z_$][\w$]*)\s*(<)?/g;
  for (let m = decl.exec(masked); m !== null; m = decl.exec(masked)) {
    const at = m.index + (m[1]?.length ?? 0);
    if (depths[at] !== 0) continue;
    const kind = m[3] === "interface" ? "interface" : "alias";
    const name = m[4] as string;
    let cursor = m.index + m[0].length;
    const generic = m[5] !== undefined;
    if (generic) {
      const end = matchBracket(masked, cursor - 1);
      if (end === undefined) continue;
      cursor = end;
    }
    let start: number;
    let heritage: string[] = [];
    if (kind === "interface") {
      const open = masked.indexOf("{", cursor);
      if (open < 0) continue;
      const between = masked.slice(cursor, open).trim();
      const ext = /^extends\s+([\s\S]+)$/.exec(between);
      if (between !== "" && !ext) continue;
      if (ext) {
        heritage = (ext[1] as string)
          .split(",")
          .map((part) => part.trim())
          .filter(Boolean);
      }
      start = open;
    } else {
      const eq = /^\s*=/.exec(masked.slice(cursor));
      if (!eq) continue;
      start = cursor + eq[0].length;
    }
    // A later declaration of the same name (interface merging) is not
    // modelled; the first one is the one read.
    if (declarations.has(name)) continue;
    declarations.set(name, {
      name,
      kind,
      exported: m[2] !== undefined,
      start,
      heritage,
      generic,
      doc: docBefore(source, at),
      line: lineOf(source, at)
    });
  }

  // `from` and no further: the specifier is a string, blanked in `masked`, so
  // the quote is found in `source`.
  const imp = /(^|[;\s])import\s+(?:type\s+)?\{([^}]*)\}\s*from/g;
  for (let m = imp.exec(masked); m !== null; m = imp.exec(masked)) {
    if (depths[m.index] !== 0) continue;
    let quoteAt = m.index + m[0].length;
    while (/\s/.test(source[quoteAt] ?? "")) quoteAt++;
    const quote = source[quoteAt];
    if (quote !== '"' && quote !== "'") continue;
    const close = source.indexOf(quote, quoteAt + 1);
    if (close < 0) continue;
    const specifier = source.slice(quoteAt + 1, close);
    if (!specifier.startsWith(".")) continue;
    for (const part of (m[2] as string).split(",")) {
      const named = /^\s*(?:type\s+)?([A-Za-z_$][\w$]*)(?:\s+as\s+([A-Za-z_$][\w$]*))?\s*$/.exec(
        part
      );
      if (!named) continue;
      imports.set(named[2] ?? (named[1] as string), { specifier, name: named[1] as string });
    }
  }

  return { path, source, masked, declarations, imports };
}

/**
 * The modules a schema may draw on: the entry, and the relative files it (and
 * they) import, read on demand through `read` and cached.
 */
export class TypeProgram {
  private readonly modules = new Map<string, TypeModule | null>();

  constructor(private readonly read: ReadSource) {}

  /** A module by absolute path, or `undefined` when it cannot be read. */
  module(path: string): TypeModule | undefined {
    const key = normalize(path);
    if (!this.modules.has(key)) {
      const source = this.read(key);
      this.modules.set(key, source === undefined ? null : readTypeModule(key, source));
    }
    return this.modules.get(key) ?? undefined;
  }

  /** The declaration `name` refers to in `from`: its own, or one it imports. */
  resolve(
    from: TypeModule,
    name: string,
    seen = new Set<string>()
  ): { decl: TypeDeclaration; module: TypeModule } | undefined {
    const own = from.declarations.get(name);
    if (own) return { decl: own, module: from };
    const imported = from.imports.get(name);
    if (!imported) return undefined;
    const target = this.importTarget(from.path, imported.specifier);
    if (!target) return undefined;
    const key = `${target.path}#${imported.name}`;
    if (seen.has(key)) return undefined;
    seen.add(key);
    return this.resolve(target, imported.name, seen);
  }

  /** `./types`, `./types.js`, `./types/index` … as TypeScript resolves them. */
  private importTarget(fromPath: string, specifier: string): TypeModule | undefined {
    const base = join(dirname(fromPath), specifier);
    const stem = base.replace(/\.(m|c)?js$/, "");
    for (const candidate of [
      base,
      `${stem}.ts`,
      `${stem}.tsx`,
      `${stem}.d.ts`,
      `${stem}.mts`,
      join(stem, "index.ts")
    ]) {
      if (!/\.(m|c)?tsx?$/.test(candidate)) continue;
      const module = this.module(candidate);
      if (module) return module;
    }
    return undefined;
  }
}

// ── JSDoc ───────────────────────────────────────────────────────────────────

export interface JsDoc {
  description?: string;
  tags: Array<{ name: string; text: string }>;
}

/** `/** … *\/` → its prose and its `@tag text` lines. */
export function parseJsDoc(comment: string): JsDoc {
  const body = comment
    .replace(/^\/\*\*/, "")
    .replace(/\*\/$/, "")
    .split("\n")
    .map((line) => line.replace(/^\s*\*?\s?/, ""));
  const prose: string[] = [];
  const tags: Array<{ name: string; text: string }> = [];
  for (const line of body) {
    const tag = /^@([A-Za-z]+)\s*(.*)$/.exec(line.trim());
    if (tag) tags.push({ name: tag[1] as string, text: (tag[2] as string).trim() });
    else if (tags.length > 0) {
      const last = tags[tags.length - 1] as { name: string; text: string };
      last.text = `${last.text} ${line.trim()}`.trim();
    } else prose.push(line);
  }
  const description = prose
    .join("\n")
    .split(/\n\s*\n/)
    .map((para) => para.replace(/\s+/g, " ").trim())
    .filter(Boolean)
    .join("\n\n");
  return { description: description || undefined, tags };
}

const NUMERIC_TAGS = new Set([
  "minimum",
  "maximum",
  "exclusiveMinimum",
  "exclusiveMaximum",
  "multipleOf",
  "minLength",
  "maxLength",
  "minItems",
  "maxItems"
]);

/** Write a JSDoc's description and keyword tags onto `schema`. */
function applyDoc(schema: JsonSchema, doc: JsDoc | undefined): JsonSchema {
  if (!doc) return schema;
  if (doc.description) schema.description = doc.description;
  for (const { name, text } of doc.tags) {
    if (NUMERIC_TAGS.has(name)) {
      const n = Number(text);
      if (text !== "" && Number.isFinite(n)) schema[name] = n;
    } else if (name === "pattern" || name === "format") {
      if (text) schema[name] = text;
    } else if (name === "default") {
      try {
        schema.default = JSON.parse(text);
      } catch {
        schema.default = text;
      }
    } else if (name === "deprecated") {
      schema.deprecated = true;
    } else if (name === "asType" && text === "integer") {
      if (schema.type === "number") schema.type = "integer";
      else if (Array.isArray(schema.type)) {
        schema.type = schema.type.map((t) => (t === "number" ? "integer" : t));
      }
    }
  }
  return schema;
}

// ── Tokens ──────────────────────────────────────────────────────────────────

interface Token {
  kind: "ident" | "string" | "number" | "punct" | "template" | "eof";
  value: string;
  pos: number;
  /** The JSDoc block right before this token, if any. */
  doc?: JsDoc;
}

const PUNCT = [
  "=>",
  "...",
  "{",
  "}",
  "(",
  ")",
  "[",
  "]",
  "<",
  ">",
  ";",
  ",",
  ":",
  "?",
  "|",
  "&",
  "=",
  ".",
  "-"
];

/** A lazy tokenizer over type syntax, from one offset of `source`. */
class Lexer {
  private ahead: Token | undefined;

  constructor(
    private readonly source: string,
    private pos: number
  ) {}

  peek(): Token {
    this.ahead ??= this.scan();
    return this.ahead;
  }

  next(): Token {
    const token = this.peek();
    this.ahead = undefined;
    return token;
  }

  private scan(): Token {
    const s = this.source;
    let doc: JsDoc | undefined;
    for (;;) {
      while (this.pos < s.length && /\s/.test(s[this.pos] as string)) this.pos++;
      if (s.startsWith("//", this.pos)) {
        const nl = s.indexOf("\n", this.pos);
        this.pos = nl < 0 ? s.length : nl;
        doc = undefined;
        continue;
      }
      if (s.startsWith("/*", this.pos)) {
        const close = s.indexOf("*/", this.pos + 2);
        const end = close < 0 ? s.length : close + 2;
        const text = s.slice(this.pos, end);
        doc = text.startsWith("/**") ? parseJsDoc(text) : undefined;
        this.pos = end;
        continue;
      }
      break;
    }
    const pos = this.pos;
    const c = s[pos];
    if (c === undefined) return { kind: "eof", value: "", pos, doc };
    if (c === '"' || c === "'") {
      let i = pos + 1;
      let value = "";
      while (i < s.length && s[i] !== c) {
        if (s[i] === "\\") {
          const esc = s[i + 1] ?? "";
          value += ({ n: "\n", t: "\t", r: "\r" } as Record<string, string>)[esc] ?? esc;
          i += 2;
        } else value += s[i++];
      }
      this.pos = i + 1;
      return { kind: "string", value, pos, doc };
    }
    if (c === "`") {
      const close = s.indexOf("`", pos + 1);
      this.pos = close < 0 ? s.length : close + 1;
      return { kind: "template", value: s.slice(pos, this.pos), pos, doc };
    }
    const num = /^(?:0x[0-9a-fA-F]+|\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(
      s.slice(pos, pos + 64)
    );
    if (num) {
      this.pos = pos + num[0].length;
      return { kind: "number", value: num[0].replace(/_/g, ""), pos, doc };
    }
    const id = /^[A-Za-z_$][\w$]*/.exec(s.slice(pos, pos + 256));
    if (id) {
      this.pos = pos + id[0].length;
      return { kind: "ident", value: id[0], pos, doc };
    }
    const punct = PUNCT.find((p) => s.startsWith(p, pos));
    this.pos = pos + (punct?.length ?? 1);
    return { kind: "punct", value: punct ?? c, pos, doc };
  }
}

// ── The type syntax this module reads ───────────────────────────────────────

/** A parsed type expression. */
export type TypeNode =
  | { k: "keyword"; name: string; pos: number }
  | { k: "literal"; value: string | number | boolean; pos: number }
  | { k: "ref"; name: string; args: TypeNode[]; pos: number }
  | { k: "array"; item: TypeNode; pos: number }
  | { k: "object"; members: TypeMember[]; index?: TypeNode; pos: number }
  | { k: "union"; members: TypeNode[]; pos: number };

export interface TypeMember {
  name: string;
  optional: boolean;
  type: TypeNode;
  doc?: JsDoc;
}

const KEYWORDS = new Set([
  "string",
  "number",
  "boolean",
  "null",
  "undefined",
  "unknown",
  "any",
  "object",
  "never",
  "void",
  "bigint",
  "symbol"
]);

/** Parses one type expression; stops at the first token that cannot continue it. */
class TypeParser {
  private readonly lexer: Lexer;

  constructor(
    private readonly module: TypeModule,
    start: number
  ) {
    this.lexer = new Lexer(module.source, start);
  }

  fail(message: string, pos: number): never {
    throw new UnsupportedType(message, this.module.path, lineOf(this.module.source, pos));
  }

  /** Where the next token starts. */
  get position(): number {
    return this.lexer.peek().pos;
  }

  private isPunct(value: string): boolean {
    const t = this.lexer.peek();
    return t.kind === "punct" && t.value === value;
  }

  private expect(value: string): Token {
    const t = this.lexer.next();
    if (t.kind !== "punct" || t.value !== value) {
      this.fail(`expected \`${value}\`, found \`${t.value || "end of file"}\``, t.pos);
    }
    return t;
  }

  type(): TypeNode {
    const pos = this.position;
    if (this.isPunct("|")) this.lexer.next();
    const members = [this.intersection()];
    while (this.isPunct("|")) {
      this.lexer.next();
      members.push(this.intersection());
    }
    return members.length === 1 ? (members[0] as TypeNode) : { k: "union", members, pos };
  }

  private intersection(): TypeNode {
    const node = this.postfix();
    if (this.isPunct("&")) {
      this.fail(
        "intersection types (`A & B`) are not supported — declare one object type",
        this.position
      );
    }
    return node;
  }

  private postfix(): TypeNode {
    let node = this.primary();
    while (this.isPunct("[")) {
      const pos = this.lexer.next().pos;
      if (!this.isPunct("]")) this.fail("indexed access types (`T[K]`) are not supported", pos);
      this.lexer.next();
      node = { k: "array", item: node, pos };
    }
    return node;
  }

  private primary(): TypeNode {
    const t = this.lexer.next();
    const pos = t.pos;
    if (t.kind === "punct") {
      if (t.value === "(") {
        const inner = this.type();
        this.expect(")");
        if (this.isPunct("=>")) this.fail("function types are not JSON", pos);
        return inner;
      }
      if (t.value === "{") return this.objectBody(pos);
      if (t.value === "[") this.fail("tuple types are not supported — use an array (`T[]`)", pos);
      if (t.value === "-") {
        const n = this.lexer.next();
        if (n.kind !== "number") this.fail("expected a number after `-`", n.pos);
        return { k: "literal", value: -Number(n.value), pos };
      }
      this.fail(`expected a type, found \`${t.value}\``, pos);
    }
    if (t.kind === "string") return { k: "literal", value: t.value, pos };
    if (t.kind === "number") return { k: "literal", value: Number(t.value), pos };
    if (t.kind === "template") this.fail("template literal types are not supported", pos);
    if (t.kind === "eof") this.fail("expected a type, found the end of the file", pos);
    if (t.value === "true" || t.value === "false") {
      return { k: "literal", value: t.value === "true", pos };
    }
    if (t.value === "readonly") return this.postfix();
    if (["typeof", "keyof", "infer", "unique"].includes(t.value)) {
      this.fail(`\`${t.value}\` types are not supported`, pos);
    }
    if (KEYWORDS.has(t.value)) return { k: "keyword", name: t.value, pos };
    let name = t.value;
    while (this.isPunct(".")) {
      this.lexer.next();
      name += `.${this.lexer.next().value}`;
    }
    const args: TypeNode[] = [];
    if (this.isPunct("<")) {
      this.lexer.next();
      args.push(this.type());
      while (this.isPunct(",")) {
        this.lexer.next();
        args.push(this.type());
      }
      this.expect(">");
    }
    return { k: "ref", name, args, pos };
  }

  /** `{ a: T; b?: U; [key: string]: V }`, the `{` already read. */
  objectBody(pos: number): TypeNode {
    const members: TypeMember[] = [];
    let index: TypeNode | undefined;
    for (;;) {
      const t = this.lexer.next();
      if (t.kind === "punct" && t.value === "}") break;
      if (t.kind === "punct" && (t.value === ";" || t.value === ",")) continue;
      if (t.kind === "eof") this.fail("unterminated object type", pos);
      let key = t;
      if (key.kind === "ident" && key.value === "readonly") {
        const after = this.lexer.peek();
        if (!(after.kind === "punct" && [":", "?", "("].includes(after.value))) {
          key = this.lexer.next();
          key.doc ??= t.doc;
        }
      }
      if (key.kind === "punct" && key.value === "[") {
        this.lexer.next(); // the index parameter's name
        this.expect(":");
        const keyType = this.lexer.next();
        if (keyType.value !== "string") {
          this.fail("only `[key: string]` index signatures are supported", keyType.pos);
        }
        this.expect("]");
        this.expect(":");
        index = this.type();
        continue;
      }
      if (key.kind !== "ident" && key.kind !== "string" && key.kind !== "number") {
        this.fail(`expected a member name, found \`${key.value}\``, key.pos);
      }
      let optional = false;
      if (this.isPunct("?")) {
        this.lexer.next();
        optional = true;
      }
      if (this.isPunct("(") || this.isPunct("<")) {
        this.fail(`method \`${key.value}\` is not JSON — declare data members only`, key.pos);
      }
      this.expect(":");
      members.push({ name: key.value, optional, type: this.type(), doc: key.doc });
    }
    return { k: "object", members, index, pos };
  }
}

/** Parse the type expression starting at `offset` of `module`. */
export function parseTypeAt(module: TypeModule, offset: number): TypeNode {
  return parseTypeSpan(module, offset).node;
}

/** Like {@link parseTypeAt}, and where the next token after the type starts. */
export function parseTypeSpan(module: TypeModule, offset: number): { node: TypeNode; end: number } {
  const parser = new TypeParser(module, offset);
  const node = parser.type();
  return { node, end: parser.position };
}

// ── To JSON Schema ──────────────────────────────────────────────────────────

/** What a conversion reads from: the program and the module a node lives in. */
interface Scope {
  program: TypeProgram;
  module: TypeModule;
  /** Declarations being expanded, to refuse a recursive type by name. */
  expanding: string[];
}

function fail(scope: Scope, message: string, pos: number): never {
  throw new UnsupportedType(message, scope.module.path, lineOf(scope.module.source, pos));
}

const isLiteralSchema = (s: JsonSchema): boolean =>
  "const" in s && Object.keys(s).every((k) => k === "const" || k === "type");

const isPlainType = (s: JsonSchema): boolean =>
  Object.keys(s).length === 1 && (typeof s.type === "string" || Array.isArray(s.type));

const typeList = (s: JsonSchema): string[] =>
  Array.isArray(s.type) ? (s.type as string[]) : [s.type as string];

function unique<T>(values: T[]): T[] {
  return values.filter((v, i) => values.indexOf(v) === i);
}

/** One type name, or a list of several. */
const oneOrMany = (types: string[]): string | string[] =>
  types.length === 1 ? (types[0] as string) : types;

/** `undefined` members dropped; whether there were any. */
function withoutUndefined(node: TypeNode): { node: TypeNode | undefined; optional: boolean } {
  if (node.k === "keyword" && node.name === "undefined") return { node: undefined, optional: true };
  if (node.k !== "union") return { node, optional: false };
  const kept = node.members.filter((m) => !(m.k === "keyword" && m.name === "undefined"));
  if (kept.length === node.members.length) return { node, optional: false };
  if (kept.length === 0) return { node: undefined, optional: true };
  return {
    node: kept.length === 1 ? kept[0] : { ...node, members: kept },
    optional: true
  };
}

function convert(node: TypeNode, scope: Scope): JsonSchema {
  switch (node.k) {
    case "keyword":
      switch (node.name) {
        case "string":
        case "number":
        case "boolean":
        case "null":
        case "object":
          return { type: node.name };
        case "unknown":
        case "any":
          return {};
        default:
          return fail(scope, `\`${node.name}\` has no JSON form`, node.pos);
      }
    case "literal":
      return { type: typeof node.value, const: node.value };
    case "array":
      return { type: "array", items: convert(node.item, scope) };
    case "object":
      return objectSchema(node.members, node.index, scope);
    case "union":
      return unionSchema(node, scope);
    case "ref":
      return refSchema(node, scope);
  }
}

function objectSchema(
  members: TypeMember[],
  index: TypeNode | undefined,
  scope: Scope
): JsonSchema {
  const properties: Record<string, JsonSchema> = {};
  const required: string[] = [];
  for (const member of members) {
    const { node, optional } = withoutUndefined(member.type);
    if (!node) fail(scope, `member \`${member.name}\` is only \`undefined\``, member.type.pos);
    properties[member.name] = applyDoc(convert(node, scope), member.doc);
    if (!member.optional && !optional) required.push(member.name);
  }
  const schema: JsonSchema = { type: "object", properties };
  if (required.length > 0) schema.required = required;
  schema.additionalProperties = index ? convert(index, scope) : false;
  return schema;
}

function unionSchema(node: Extract<TypeNode, { k: "union" }>, scope: Scope): JsonSchema {
  const flat: TypeNode[] = [];
  const flatten = (n: TypeNode): void => {
    if (n.k === "union") n.members.forEach(flatten);
    else flat.push(n);
  };
  flatten(node);
  const { node: kept } = withoutUndefined({ ...node, members: flat });
  if (!kept) return fail(scope, "a union of only `undefined` has no JSON form", node.pos);
  if (kept.k !== "union") return convert(kept, scope);

  const schemas = kept.members.map((m) => convert(m, scope));
  if (schemas.some((s) => Object.keys(s).length === 0)) return {};

  // Literals (and `null`) only: one enum.
  const isNull = (s: JsonSchema) => isPlainType(s) && s.type === "null";
  if (schemas.every((s) => isLiteralSchema(s) || isNull(s))) {
    const values = unique(schemas.map((s) => (isNull(s) ? null : s.const)));
    const types = unique(schemas.map((s) => s.type as string));
    // `true | false` is `boolean`.
    if (types.length === 1 && types[0] === "boolean" && values.length === 2)
      return { type: "boolean" };
    return { type: oneOrMany(types), enum: values };
  }
  // Bare types only (`string | number | null`): one type list.
  if (schemas.every(isPlainType)) return { type: oneOrMany(unique(schemas.flatMap(typeList))) };
  return { anyOf: schemas };
}

const BUILTIN_DOC = "is not declared in this file or a relative import it names";

function refSchema(node: Extract<TypeNode, { k: "ref" }>, scope: Scope): JsonSchema {
  const arity = (n: number) => {
    if (node.args.length !== n) {
      fail(scope, `\`${node.name}\` takes ${n} type argument${n === 1 ? "" : "s"}`, node.pos);
    }
  };
  switch (node.name) {
    case "Array":
    case "ReadonlyArray":
      arity(1);
      return { type: "array", items: convert(node.args[0] as TypeNode, scope) };
    case "Readonly":
      arity(1);
      return convert(node.args[0] as TypeNode, scope);
    case "Date":
      return { type: "string", format: "date-time" };
    case "Record": {
      arity(2);
      const [key, value] = node.args as [TypeNode, TypeNode];
      const keySchema = convert(key, scope);
      const valueSchema = convert(value, scope);
      if (isPlainType(keySchema) && keySchema.type === "string") {
        return { type: "object", additionalProperties: valueSchema };
      }
      if (Array.isArray(keySchema.enum) && keySchema.type === "string") {
        const names = keySchema.enum as string[];
        const properties = Object.fromEntries(names.map((n) => [n, { ...valueSchema }]));
        return { type: "object", properties, required: names, additionalProperties: false };
      }
      if (isLiteralSchema(keySchema) && typeof keySchema.const === "string") {
        const name = keySchema.const;
        return {
          type: "object",
          properties: { [name]: valueSchema },
          required: [name],
          additionalProperties: false
        };
      }
      return fail(scope, "`Record` keys must be `string` or string literals", node.pos);
    }
  }
  if (node.args.length > 0) {
    return fail(scope, `generic type \`${node.name}<…>\` is not supported`, node.pos);
  }
  const found = scope.program.resolve(scope.module, node.name);
  if (!found) return fail(scope, `\`${node.name}\` ${BUILTIN_DOC}`, node.pos);
  return declarationSchema(found.decl, { ...scope, module: found.module });
}

/** A declaration, inlined: its body converted, its JSDoc as the description. */
function declarationSchema(decl: TypeDeclaration, scope: Scope): JsonSchema {
  const key = `${scope.module.path}#${decl.name}`;
  if (scope.expanding.includes(key)) {
    return fail(
      scope,
      `\`${decl.name}\` refers to itself; recursive types are not supported`,
      decl.start
    );
  }
  if (decl.generic)
    return fail(scope, `generic declaration \`${decl.name}<…>\` is not supported`, decl.start);
  const inner: Scope = { ...scope, expanding: [...scope.expanding, key] };
  let schema: JsonSchema;
  if (decl.kind === "alias") {
    schema = convert(parseTypeAt(scope.module, decl.start), inner);
  } else {
    const parser = new TypeParser(scope.module, decl.start + 1);
    const body = parser.objectBody(decl.start);
    if (body.k !== "object") return fail(scope, "expected an interface body", decl.start);
    schema = objectSchema(body.members, body.index, inner);
    for (const parent of decl.heritage) {
      if (!/^[A-Za-z_$][\w$]*$/.test(parent)) {
        return fail(scope, `\`extends ${parent}\` is not supported`, decl.start);
      }
      const base = refSchema({ k: "ref", name: parent, args: [], pos: decl.start }, inner);
      if (base.type !== "object" || typeof base.properties !== "object") {
        return fail(scope, `\`${parent}\` is not an object type`, decl.start);
      }
      schema.properties = { ...(base.properties as object), ...(schema.properties as object) };
      const required = unique([
        ...((base.required as string[] | undefined) ?? []),
        ...((schema.required as string[] | undefined) ?? [])
      ]);
      if (required.length > 0) schema.required = required;
    }
  }
  if (decl.doc?.description && schema.description === undefined) {
    schema = { description: decl.doc.description, ...schema };
  }
  return schema;
}

/** The schema of a declared type, by name, as seen from `module`. */
export function schemaForDeclaration(
  program: TypeProgram,
  module: TypeModule,
  name: string
): JsonSchema {
  const found = program.resolve(module, name);
  if (!found) {
    throw new UnsupportedType(`\`${name}\` ${BUILTIN_DOC}`, module.path, 1);
  }
  return withDialect(
    declarationSchema(found.decl, { program, module: found.module, expanding: [] })
  );
}

/** The schema of a parsed type expression in `module`. */
export function schemaForType(
  program: TypeProgram,
  module: TypeModule,
  node: TypeNode
): JsonSchema {
  return withDialect(convert(node, { program, module, expanding: [] }));
}

/** `$schema` first, so a reader of the manifest knows the dialect. */
function withDialect(schema: JsonSchema): JsonSchema {
  return { $schema: SCHEMA_DIALECT, ...schema };
}
