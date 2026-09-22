/**
 * Lexical scans behind `oxyc validate`'s data-placement checks: Airhouse
 * migration SQL, and a function's calls to `ctx.secrets.set`.
 *
 * NOT PARSERS, AND NOT THE AUTHORITY. The server parses an Airhouse migration
 * before it applies it and refuses what it cannot place; these answer earlier,
 * for the common mistakes. So every scan is CONSERVATIVE: comments and string
 * literals are masked before a keyword is matched, a target that cannot be read
 * is skipped rather than guessed at, and where a scan and the server disagree,
 * the server is right.
 */

export interface ScanHit {
  /** 1-based, in the original text — masking preserves every offset. */
  line: number;
  level: "error" | "warning";
  message: string;
}

/** Every character but a newline as a space, so offsets and line numbers survive. */
function blank(text: string): string {
  return text.replace(/[^\n]/g, " ");
}

/** 1-based line of `offset` in `text`; shared with `function-lint.ts`. */
export function lineAt(text: string, offset: number): number {
  let line = 1;
  for (let i = 0; i < offset; i++) if (text.charCodeAt(i) === 10) line++;
  return line;
}

function stickyAt(re: RegExp, text: string, at: number): RegExpExecArray | null {
  re.lastIndex = at;
  return re.exec(text);
}

/** Just past a block comment opened at `open`; SQL nests them, JavaScript does not. */
function closeBlockComment(text: string, open: number, nested: boolean): number {
  let depth = 1;
  let i = open + 2;
  while (i < text.length) {
    if (nested && text.startsWith("/*", i)) {
      depth++;
      i += 2;
    } else if (text.startsWith("*/", i)) {
      depth--;
      i += 2;
      if (depth === 0) return i;
    } else {
      i++;
    }
  }
  return text.length;
}

// ── SQL ──────────────────────────────────────────────────────────────────────

interface MaskedSql {
  /** Comments and string literals blanked, quoted identifiers kept: targets are read here. */
  code: string;
  /** `code` with quoted-identifier CONTENTS blanked too: keywords are matched here. */
  bare: string;
}

/** Just past the quote closing the one at `open`; a doubled quote is an escape. */
function closeDoubled(text: string, open: number, quote: string): number {
  for (let i = open + 1; i < text.length; i++) {
    if (text[i] !== quote) continue;
    if (text[i + 1] === quote) {
      i++;
      continue;
    }
    return i + 1;
  }
  return text.length;
}

function dollarTagAt(text: string, at: number): string | undefined {
  return stickyAt(/\$(?:[A-Za-z_][A-Za-z0-9_]*)?\$/y, text, at)?.[0];
}

/** One masked token: where it ends, and what replaces it in each view. */
function sqlToken(sql: string, i: number): { end: number; code: string; bare: string } | undefined {
  const two = sql.slice(i, i + 2);
  let end: number | undefined;
  if (two === "--") {
    const newline = sql.indexOf("\n", i);
    end = newline < 0 ? sql.length : newline;
  } else if (two === "/*") {
    end = closeBlockComment(sql, i, true);
  } else if (sql[i] === "'") {
    end = closeDoubled(sql, i, "'");
  } else if (sql[i] === "$" && dollarTagAt(sql, i)) {
    const tag = dollarTagAt(sql, i) as string;
    const close = sql.indexOf(tag, i + tag.length);
    end = close < 0 ? sql.length : close + tag.length;
  } else if (sql[i] === '"') {
    // Kept whole for reading a target; only the INSIDE is blanked for keywords,
    // so a prefix's trailing `\s+` still stops at the opening quote.
    const close = closeDoubled(sql, i, '"');
    const inner = sql.slice(i + 1, Math.max(i + 1, close - 1));
    const tail = sql.slice(i + 1 + inner.length, close);
    return { end: close, code: sql.slice(i, close), bare: `"${blank(inner)}${tail}` };
  }
  if (end === undefined) return undefined;
  const masked = blank(sql.slice(i, end));
  return { end, code: masked, bare: masked };
}

export function maskSql(sql: string): MaskedSql {
  const code: string[] = [];
  const bare: string[] = [];
  let plain = 0;
  let i = 0;
  while (i < sql.length) {
    const token = sqlToken(sql, i);
    if (!token) {
      i++;
      continue;
    }
    code.push(sql.slice(plain, i), token.code);
    bare.push(sql.slice(plain, i), token.bare);
    i = token.end;
    plain = token.end;
  }
  code.push(sql.slice(plain));
  bare.push(sql.slice(plain));
  return { code: code.join(""), bare: bare.join("") };
}

const DUCKLAKE =
  "DuckLake has no primary keys, unique constraints, indexes or foreign keys, and a table " +
  "carrying one fails and leaves the app's Airhouse writer inert";

/** Leftmost-first, so `CREATE UNIQUE INDEX` is one hit rather than that and `UNIQUE`. */
const UNSUPPORTED =
  /\bCREATE\s+(?:UNIQUE\s+)?INDEX\b|\bPRIMARY\s+KEY\b|\bUNIQUE\b|\bFOREIGN\s+KEY\b|\bREFERENCES\b/gi;

function unsupportedConstraints(sql: string, bare: string): ScanHit[] {
  const hits: ScanHit[] = [];
  const seen = new Set<string>();
  for (const m of bare.matchAll(UNSUPPORTED)) {
    const written = m[0].replace(/\s+/g, " ").toUpperCase();
    const line = lineAt(sql, m.index ?? 0);
    // `FOREIGN KEY (a) REFERENCES t (b)` is one constraint spelled with two keywords.
    const key = `${line}:${written === "REFERENCES" ? "FOREIGN KEY" : written}`;
    if (seen.has(key)) continue;
    seen.add(key);
    hits.push({
      line,
      level: "error",
      message: `\`${written}\` is not supported on Airhouse: ${DUCKLAKE}`
    });
  }
  return hits;
}

/** A statement whose target must be `app_<writer>.<name>`. */
interface Targeted {
  label: string;
  re: RegExp;
  /** `DROP TABLE a, b` names several. */
  list?: boolean;
}

const TARGETED: Targeted[] = [
  {
    label: "CREATE TABLE",
    re: /\bCREATE\s+(?:OR\s+REPLACE\s+)?(?:(?:TEMP|TEMPORARY)\s+)?TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?/gi
  },
  {
    label: "CREATE VIEW",
    re: /\bCREATE\s+(?:OR\s+REPLACE\s+)?(?:(?:TEMP|TEMPORARY)\s+)?VIEW\s+(?:IF\s+NOT\s+EXISTS\s+)?/gi
  },
  { label: "ALTER TABLE", re: /\bALTER\s+TABLE\s+(?:IF\s+EXISTS\s+)?/gi },
  { label: "DROP TABLE", re: /\bDROP\s+TABLE\s+(?:IF\s+EXISTS\s+)?/gi, list: true },
  { label: "DROP VIEW", re: /\bDROP\s+VIEW\s+(?:IF\s+EXISTS\s+)?/gi, list: true },
  { label: "INSERT INTO", re: /\bINSERT\s+(?:OR\s+(?:REPLACE|IGNORE)\s+)?INTO\s+/gi },
  // Not `ON CONFLICT … DO UPDATE` or an FK's `ON UPDATE`, neither of which has a target.
  { label: "UPDATE", re: /(?<!\b(?:DO|ON)\s+)\bUPDATE\s+/gi },
  { label: "DELETE FROM", re: /\bDELETE\s+FROM\s+/gi },
  { label: "COMMENT ON", re: /\bCOMMENT\s+ON\s+(?:TABLE|VIEW|COLUMN)\s+/gi }
];

const PART = /"(?:[^"]|"")*"|[A-Za-z_][A-Za-z0-9_$]*/y;
const DOT = /\s*\.\s*/y;
const COMMA = /\s*,\s*/y;

interface Name {
  /** As written, quotes included — what a suggestion should reuse. */
  raw: string[];
  /**
   * Lower-cased, quoted or not: DuckDB matches identifiers case-insensitively
   * either way, so `"APP_X".t` names the app's schema and must not be refused.
   */
  parts: string[];
  end: number;
}

function readName(code: string, at: number): Name | undefined {
  const raw: string[] = [];
  const parts: string[] = [];
  let i = at;
  for (;;) {
    const part = stickyAt(PART, code, i);
    if (!part) return undefined;
    const token = part[0];
    raw.push(token);
    parts.push(
      (token.startsWith('"') ? token.slice(1, -1).replaceAll('""', '"') : token).toLowerCase()
    );
    i = PART.lastIndex;
    if (!stickyAt(DOT, code, i)) break;
    i = DOT.lastIndex;
  }
  return { raw, parts, end: i };
}

function misplaced(label: string, name: Name, schema: string, column: boolean): string | undefined {
  // A column is `schema.table.column`; everything else is `schema.name`.
  const want = column ? 3 : 2;
  if (name.parts.length === want && name.parts[0] === schema) return undefined;
  const written = `${label} ${name.raw.join(".")}`;
  const suggestion = `${schema}.${name.raw.slice(-(want - 1)).join(".")}`;
  const what = name.parts.length < want ? "is unqualified" : "is outside the app's schema";
  return (
    `\`${written}\` ${what} — write \`${suggestion}\`: the server refuses an Airhouse ` +
    `migration target that is not qualified with \`${schema}\``
  );
}

function misplacedTargets(sql: string, masked: MaskedSql, schema: string): ScanHit[] {
  const hits: ScanHit[] = [];
  for (const target of TARGETED) {
    for (const m of masked.bare.matchAll(target.re)) {
      const column = /\bCOLUMN\s+$/i.test(m[0]);
      let at = (m.index ?? 0) + m[0].length;
      for (;;) {
        // A target this cannot read — a placeholder, a subquery — is the server's to judge.
        const name = readName(masked.code, at);
        if (!name) break;
        const problem = misplaced(target.label, name, schema, column);
        if (problem) hits.push({ line: lineAt(sql, at), level: "error", message: problem });
        if (!target.list || !stickyAt(COMMA, masked.code, name.end)) break;
        at = COMMA.lastIndex;
      }
    }
  }
  return hits;
}

/**
 * Every placement problem in one Airhouse migration.
 *
 * `schema` is `app_<writer>`, or `undefined` when the slug could not derive one
 * — the target check then has nothing to compare against and is skipped; the
 * caller reports why.
 */
export function scanAirhouseMigration(sql: string, schema: string | undefined): ScanHit[] {
  const masked = maskSql(sql);
  const hits = unsupportedConstraints(sql, masked.bare);
  for (const m of masked.bare.matchAll(/\bCREATE\s+SCHEMA\b/gi)) {
    hits.push({
      line: lineAt(sql, m.index ?? 0),
      level: "warning",
      message:
        `\`CREATE SCHEMA\` is unnecessary — the platform creates ` +
        `${schema ? `\`${schema}\`` : "the app's schema"} before it applies migrations`
    });
  }
  if (schema !== undefined) hits.push(...misplacedTargets(sql, masked, schema));
  return hits.sort((a, b) => a.line - b.line);
}

// ── JavaScript / TypeScript ──────────────────────────────────────────────────

/** Just past a quote closing the one at `open`; a backslash escapes. */
function closeEscaped(text: string, open: number, quote: string): number {
  for (let i = open + 1; i < text.length; i++) {
    if (text[i] === "\\") {
      i++;
      continue;
    }
    if (text[i] === quote) return i + 1;
    // An unterminated '…' or "…" ends at the line; a template literal does not.
    if (quote !== "`" && text[i] === "\n") return i;
  }
  return text.length;
}

/** A `/` starts a regex literal only where a value may begin, not after one. */
function regexCanStart(text: string, at: number): boolean {
  let j = at - 1;
  while (j >= 0 && /\s/.test(text[j] as string)) j--;
  return j < 0 || "(,=:[!&|?{};".includes(text[j] as string);
}

function closeRegex(text: string, open: number): number {
  let inClass = false;
  for (let i = open + 1; i < text.length; i++) {
    const c = text[i];
    if (c === "\\") {
      i++;
    } else if (c === "\n") {
      return open + 1;
    } else if (inClass) {
      if (c === "]") inClass = false;
    } else if (c === "[") {
      inClass = true;
    } else if (c === "/") {
      return i + 1;
    }
  }
  return open + 1;
}

/**
 * Comments, strings, template literals and regex literals blanked.
 *
 * APPROXIMATE: a template literal is masked whole, `${…}` included, so a call
 * written inside one is not seen. That errs toward silence, which is the side
 * a warning should err on.
 */
export function maskJs(source: string): string {
  const out: string[] = [];
  let plain = 0;
  let i = 0;
  while (i < source.length) {
    const c = source[i] as string;
    const two = source.slice(i, i + 2);
    let end: number | undefined;
    if (two === "//") {
      const newline = source.indexOf("\n", i);
      end = newline < 0 ? source.length : newline;
    } else if (two === "/*") {
      end = closeBlockComment(source, i, false);
    } else if (c === "'" || c === '"' || c === "`") {
      end = closeEscaped(source, i, c);
    } else if (c === "/" && regexCanStart(source, i)) {
      end = closeRegex(source, i);
    }
    if (end === undefined) {
      i++;
      continue;
    }
    out.push(source.slice(plain, i), blank(source.slice(i, end)));
    i = end;
    plain = end;
  }
  out.push(source.slice(plain));
  return out.join("");
}

/** Top-level argument ranges of a call whose `(` ends just before `from`. */
export function splitArgs(
  code: string,
  from: number
): Array<{ start: number; end: number }> | undefined {
  const args: Array<{ start: number; end: number }> = [];
  let depth = 0;
  let start = from;
  for (let i = from; i < code.length; i++) {
    const c = code[i];
    if (c === "(" || c === "[" || c === "{") {
      depth++;
    } else if (c === ")" || c === "]" || c === "}") {
      if (depth === 0) {
        args.push({ start, end: i });
        return args;
      }
      depth--;
    } else if (c === "," && depth === 0) {
      args.push({ start, end: i });
      start = i + 1;
    }
  }
  return undefined;
}

/** The value of a plain string literal, or `undefined` for anything computed. */
export function literalString(text: string): string | undefined {
  const quoted = /^(['"])((?:\\.|(?!\1)[^\\])*)\1$/s.exec(text);
  if (quoted) return quoted[2];
  if (/^`[^`]*`$/.test(text) && !text.includes("${")) return text.slice(1, -1);
  return undefined;
}

const SECRETS_SET = /\bctx\s*\??\.\s*secrets\s*\??\.\s*set\s*\(/g;
/** A rotated token written back is what `secrets.write` is for. */
const CREDENTIAL_KEY = /(token|secret|key|password)/i;
const STATE_WORD =
  /^(state|cursor|checkpoint|progress|last|count|counter|cache|offset|watermark)$/i;

/**
 * Whether a key names state, by whole words — `SYNC_CURSOR`, `row-count`,
 * `lastSyncAt` — so `QB_ACCOUNT_ID` is not read as a `count`.
 */
function namesState(key: string): boolean {
  return key
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .split(/[^A-Za-z0-9]+/)
    .some((word) => STATE_WORD.test(word));
}
const STRINGIFY = /\bJSON\s*\??\.\s*stringify\b/;

export interface SecretsStateHit {
  line: number;
  /** `ctx.secrets.set(<key as written>, …)` */
  call: string;
  /** What gave it away, worded to follow "stores". */
  why: string;
}

/**
 * `ctx.secrets.set` calls that look like state rather than a credential.
 *
 * ONE FILE — the function's entry. A helper module it imports is not followed,
 * so a call made there is not seen.
 */
export function secretsUsedAsState(source: string): SecretsStateHit[] {
  const code = maskJs(source);
  const hits: SecretsStateHit[] = [];
  for (const m of code.matchAll(SECRETS_SET)) {
    const args = splitArgs(code, (m.index ?? 0) + m[0].length);
    const keyRange = args?.[0];
    // Unterminated: the bundler reports that, and there is nothing to read here.
    if (!keyRange) continue;
    const keyText = source.slice(keyRange.start, keyRange.end).trim();
    const key = literalString(keyText);
    if (key !== undefined && CREDENTIAL_KEY.test(key)) continue;

    const valueRange = args?.[1];
    const value = valueRange ? code.slice(valueRange.start, valueRange.end) : "";
    const why = STRINGIFY.test(value)
      ? "a `JSON.stringify(…)` value"
      : key !== undefined && namesState(key)
        ? `a key that names state (\`${key}\`)`
        : undefined;
    if (why === undefined) continue;
    hits.push({
      line: lineAt(source, m.index ?? 0),
      call: `ctx.secrets.set(${keyText}, …)`,
      why
    });
  }
  return hits;
}
