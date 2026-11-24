/**
 * Does a `consistency_prompt` compile as a template?
 *
 * The Rust check (`validate_consistency_prompt`) hands the string to
 * `minijinja::Environment::add_template_owned`, which parses it. There is no
 * minijinja in TypeScript, so this is a STRUCTURAL approximation of that
 * parser: it finds what a prompt author actually gets wrong — a delimiter left
 * open, a block tag with no end tag, an end tag that closes the wrong block, an
 * empty `{{ }}` — and says nothing about the expression grammar inside a tag.
 * `{{ 1 + }}` is a minijinja error this does not catch.
 *
 * Pinned against the Rust unit-test cases for the same function
 * (`validate_consistency_prompt_tests`, beside the Rust check)
 * and the parity corpus.
 */

/** Block tags and the tags that close them. `set` opens a block only without `=`. */
const BLOCKS: Record<string, string> = {
  for: "endfor",
  if: "endif",
  macro: "endmacro",
  call: "endcall",
  filter: "endfilter",
  block: "endblock",
  with: "endwith",
  autoescape: "endautoescape",
  raw: "endraw",
  set: "endset"
};

/** Tags valid only inside an open block of the named kind. */
const INNER: Record<string, string[]> = {
  elif: ["if"],
  else: ["if", "for"],
  break: ["for"],
  continue: ["for"]
};

const CLOSERS = new Set(Object.values(BLOCKS));

const UNCLOSED: Record<string, string> = {
  "{": "unexpected end of input, expected end of variable block",
  "%": "unexpected end of input, expected end of block",
  "#": "unexpected end of comment"
};

/** One `{{ }}`, `{% %}` or `{# #}`, trimmed of whitespace control. */
interface Tag {
  kind: string;
  body: string;
  /** Where scanning resumes. */
  next: number;
}

/** The next tag at or after `from`, an error string, or `undefined` at the end. */
function nextTag(source: string, from: number): Tag | string | undefined {
  const start = source.slice(from).search(/\{[{%#]/);
  if (start === -1) return undefined;
  const at = from + start;
  const kind = source[at + 1] ?? "";
  const close = kind === "{" ? "}}" : `${kind}}`;
  const end = source.indexOf(close, at + 2);
  if (end === -1) return UNCLOSED[kind];
  const body = source
    .slice(at + 2, end)
    .replace(/^-|-$/g, "")
    .trim();
  return { kind, body, next: end + close.length };
}

/** Past `{% endraw %}`: everything up to it is text, delimiters included. */
function skipRaw(source: string, from: number): number | undefined {
  const endraw = /\{%-?\s*endraw\s*-?%\}/g;
  endraw.lastIndex = from;
  const m = endraw.exec(source);
  return m ? m.index + m[0].length : undefined;
}

/** Apply one `{% … %}` statement to the open-block stack; an error, or nothing. */
function statement(tag: string, body: string, open: string[]): string | undefined {
  if (tag === "") return "unexpected end of block";
  if (tag === "set" && body.includes("=")) return undefined;
  if (tag in BLOCKS) {
    open.push(tag);
    return undefined;
  }
  if (CLOSERS.has(tag)) {
    const top = open.pop();
    return top !== undefined && BLOCKS[top] === tag ? undefined : `unexpected \`${tag}\``;
  }
  const within = INNER[tag];
  const top = open[open.length - 1];
  if (within && (top === undefined || !within.includes(top))) return `unexpected \`${tag}\``;
  // Anything else (`include`, `import`, `extends`, …) is a statement on its
  // own; its arguments are expression grammar this does not check.
  return undefined;
}

/** Handle one tag: where scanning resumes, or the error it is. */
function step(tag: Tag, source: string, open: string[]): number | string {
  if (tag.kind === "{" && tag.body === "") return "unexpected end of variable block";
  if (tag.kind !== "%") return tag.next;
  const name = tag.body.split(/\s+/)[0] ?? "";
  if (name === "raw") return skipRaw(source, tag.next) ?? "unexpected end of raw block";
  return statement(name, tag.body, open) ?? tag.next;
}

/** The minijinja-style message for the first syntax error, or `undefined`. */
export function templateSyntaxError(source: string): string | undefined {
  const open: string[] = [];
  let i = 0;
  for (let tag = nextTag(source, i); tag !== undefined; tag = nextTag(source, i)) {
    const next = typeof tag === "string" ? tag : step(tag, source, open);
    if (typeof next === "string") return next;
    i = next;
  }
  const unclosed = open.pop();
  return unclosed
    ? `unexpected end of input, expected end of block (\`${unclosed}\` is never closed)`
    : undefined;
}
