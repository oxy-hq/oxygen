#!/usr/bin/env node
// What a shell command is about to tell `gh pr create`, for .claude/hooks/guard-pr.sh.
// Reads the command on stdin and prints JSON: `{ found: false }` when the command opens
// no PR, else the title, base, repo, and the body (or the file it will be read from).
//
// A small shell reader rather than regexes over the whole command: an earlier version
// took `git commit -F msg && gh pr create …`'s `-F` as the PR body, gated a `--fill` on
// it, and read a `gh pr create` mentioned inside a heredoc as a real one. The reader
// splits the command into simple commands with quoting, `$(…)`, heredocs and
// redirections understood well enough to find the one `gh pr create` and its own flags.
// Anything it cannot resolve (an unquoted variable, a `--fill`) is `dynamic`, and the
// hook fails open on it.

export interface PrCreate {
  found: true;
  title: string | null;
  base: string | null;
  repo: string | null;
  /** The body text when the command carries it (a quoted string, a heredoc, a file written earlier in the same command). */
  body: string | null;
  /** A `--body-file` the command reads from disk and does not write itself. */
  bodyFile: string | null;
  /** The last `cd <dir>` before the `gh pr create`, relative to where the command started. */
  cwd: string | null;
  /** A `cd` before the `gh pr create` the text does not settle (`cd "$(just wt-claim x)"`, `cd ~/x`, `cd -`). */
  cwdUnresolved: boolean;
  /** The body could not be read from the text (`--fill`, a variable, an unparsed form). */
  dynamic: boolean;
}

export type Parsed = PrCreate | { found: false };

/** One word of a simple command: its value, and whether the text settles that value. */
interface Word {
  value: string;
  dynamic: boolean;
}

interface Simple {
  words: Word[];
  /** `> file` / `>> file` targets. */
  writes: string[];
  heredoc: string | null;
}

const HEREDOC_OPEN = /^<<(-?)[ \t]*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\2/;
const CAT_HEREDOC =
  /^\s*cat\s+<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1[ \t]*\n([\s\S]*?)\n[ \t]*\2[ \t]*\n?\s*$/;

class Reader {
  i = 0;
  pendingHeredocs: { tag: string; strip: boolean; target: Simple }[] = [];
  readonly s: string;
  constructor(s: string) {
    this.s = s;
  }

  /** The whole command as simple commands, in order. */
  simples(): Simple[] {
    const out: Simple[] = [];
    let cur: Simple = { words: [], writes: [], heredoc: null };
    const flush = () => {
      if (cur.words.length || cur.writes.length) out.push(cur);
      cur = { words: [], writes: [], heredoc: null };
    };
    while (this.i < this.s.length) {
      const c = this.s[this.i] as string;
      if (c === "\n") {
        this.i++;
        this.readHeredocBodies();
        flush();
        continue;
      }
      if (c === " " || c === "\t") {
        this.i++;
        continue;
      }
      if (c === "#" && (cur.words.length === 0 || /\s/.test(this.s[this.i - 1] ?? " "))) {
        while (this.i < this.s.length && this.s[this.i] !== "\n") this.i++;
        continue;
      }
      if (this.s.startsWith("&&", this.i) || this.s.startsWith("||", this.i)) {
        this.i += 2;
        flush();
        continue;
      }
      if (c === ";" || c === "|" || c === "&" || c === "(" || c === ")") {
        this.i++;
        flush();
        continue;
      }
      const redirect = /^\d?>>?[ \t]*/.exec(this.s.slice(this.i));
      if (redirect) {
        this.i += redirect[0].length;
        const target = this.word();
        if (!target.dynamic) cur.writes.push(target.value);
        continue;
      }
      const here = HEREDOC_OPEN.exec(this.s.slice(this.i));
      if (here) {
        this.i += here[0].length;
        this.pendingHeredocs.push({ tag: here[3] as string, strip: here[1] === "-", target: cur });
        continue;
      }
      if (c === "<") {
        this.i++;
        this.word();
        continue;
      }
      cur.words.push(this.word());
    }
    this.readHeredocBodies();
    flush();
    return out;
  }

  /** After a newline: the bodies of the heredocs opened on the line just ended. */
  private readHeredocBodies(): void {
    for (const h of this.pendingHeredocs) {
      const lines: string[] = [];
      while (this.i < this.s.length) {
        const end = this.s.indexOf("\n", this.i);
        const line = end < 0 ? this.s.slice(this.i) : this.s.slice(this.i, end);
        this.i = end < 0 ? this.s.length : end + 1;
        if ((h.strip ? line.replace(/^\t+/, "") : line).trim() === h.tag && line.trim() === h.tag)
          break;
        lines.push(h.strip ? line.replace(/^\t+/, "") : line);
      }
      h.target.heredoc = lines.join("\n");
    }
    this.pendingHeredocs = [];
  }

  /** One word, quotes resolved. `$(cat <<EOF … EOF)` resolves to the heredoc's text. */
  private word(): Word {
    let value = "";
    let dynamic = false;
    while (this.i < this.s.length) {
      const c = this.s[this.i] as string;
      if (/[\s;|&()]/.test(c)) break;
      if (c === "'") {
        const end = this.s.indexOf("'", this.i + 1);
        value += this.s.slice(this.i + 1, end < 0 ? undefined : end);
        this.i = end < 0 ? this.s.length : end + 1;
        continue;
      }
      if (c === '"') {
        this.i++;
        while (this.i < this.s.length && this.s[this.i] !== '"') {
          const d = this.s[this.i] as string;
          if (d === "\\" && this.i + 1 < this.s.length) {
            value += this.s[this.i + 1];
            this.i += 2;
          } else if (d === "$" && this.s[this.i + 1] === "(") {
            const sub = this.substitution();
            if (sub === null) dynamic = true;
            else value += sub;
          } else if (d === "$") {
            dynamic = true;
            this.i++;
          } else {
            value += d;
            this.i++;
          }
        }
        this.i++;
        continue;
      }
      if (c === "\\" && this.i + 1 < this.s.length) {
        value += this.s[this.i + 1];
        this.i += 2;
        continue;
      }
      if (c === "$") {
        if (this.s[this.i + 1] === "(") {
          const sub = this.substitution();
          if (sub === null) dynamic = true;
          else value += sub;
        } else {
          dynamic = true;
          this.i++;
        }
        continue;
      }
      value += c;
      this.i++;
    }
    return { value, dynamic };
  }

  /** `$( … )` at the cursor: its text when it is `cat` of a heredoc, else null (dynamic). */
  private substitution(): string | null {
    const start = this.i + 2;
    let depth = 1;
    let j = start;
    while (j < this.s.length && depth > 0) {
      const c = this.s[j] as string;
      if (c === "'" || c === '"') {
        const end = this.s.indexOf(c, j + 1);
        j = end < 0 ? this.s.length : end + 1;
        continue;
      }
      const here = HEREDOC_OPEN.exec(this.s.slice(j));
      if (here) {
        // Skip to the line after the terminator, where the body cannot close the paren.
        const tag = here[3] as string;
        const lineEnd = this.s.indexOf("\n", j);
        if (lineEnd < 0) break;
        let k = lineEnd + 1;
        for (;;) {
          const end = this.s.indexOf("\n", k);
          const line = end < 0 ? this.s.slice(k) : this.s.slice(k, end);
          k = end < 0 ? this.s.length : end + 1;
          if (line.trim() === tag || end < 0) break;
        }
        j = k;
        continue;
      }
      if (c === "(") depth++;
      else if (c === ")") depth--;
      j++;
    }
    const inner = this.s.slice(start, j - 1);
    this.i = j;
    const m = CAT_HEREDOC.exec(inner);
    return m ? (m[3] as string) : null;
  }
}

const FLAGS: Record<string, keyof Pick<PrCreate, "title" | "base" | "repo" | "body" | "bodyFile">> =
  {
    "--title": "title",
    "-t": "title",
    "--base": "base",
    "-B": "base",
    "--repo": "repo",
    "-R": "repo",
    "--body": "body",
    "-b": "body",
    "--body-file": "bodyFile",
    "-F": "bodyFile"
  };

/** The simple command's words with leading `VAR=value` assignments dropped. */
const stripAssignments = (words: Word[]): Word[] => {
  let i = 0;
  while (i < words.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[i]?.value ?? "")) i++;
  return words.slice(i);
};

/** What the command tells `gh pr create`, or that it does not run one. */
export function parsePrCreate(command: string): Parsed {
  const simples = new Reader(command).simples();
  const writes = new Map<string, string | null>();
  let cwd: string | null = null;
  let cwdUnresolved = false;
  for (const sc of simples) {
    const words = stripAssignments(sc.words);
    const argv0 = words[0]?.value ?? "";
    // Every `cd` before the PR opens moves the checkout; one the text cannot settle
    // (a substitution, a variable, `~`, `cd -`) leaves it unknown.
    if (argv0 === "cd" || argv0 === "pushd") {
      const dir = words.find((w, i) => i > 0 && !w.value.startsWith("-"));
      if (!dir || dir.dynamic || dir.value.startsWith("~")) cwdUnresolved = true;
      else cwd = cwd && !dir.value.startsWith("/") ? `${cwd}/${dir.value}` : dir.value;
    }
    if (argv0 === "popd") cwdUnresolved = true;
    if (argv0 === "gh" && words[1]?.value === "pr" && words[2]?.value === "create")
      return prCreate(words.slice(3), sc, writes, cwd, cwdUnresolved);
    // A file this command writes before the PR opens: its content is the heredoc, when
    // one was given; `null` when written from something the text does not settle.
    for (const w of sc.writes) writes.set(w, sc.heredoc);
    if (argv0 === "tee")
      for (const w of words.slice(1)) if (!w.value.startsWith("-")) writes.set(w.value, sc.heredoc);
  }
  return { found: false };
}

function prCreate(
  args: Word[],
  sc: Simple,
  writes: Map<string, string | null>,
  cwd: string | null,
  cwdUnresolved: boolean
): PrCreate {
  const out: PrCreate = {
    found: true,
    title: null,
    base: null,
    repo: null,
    body: null,
    bodyFile: null,
    cwd,
    cwdUnresolved,
    dynamic: false
  };
  for (let i = 0; i < args.length; i++) {
    const w = args[i] as Word;
    const eq = w.value.indexOf("=");
    const flag = w.value.startsWith("--") && eq > 0 ? w.value.slice(0, eq) : w.value;
    const key = FLAGS[flag];
    if (!key) {
      if (/^(--fill|--fill-first|--fill-verbose|-f|--web|-w|--template|-T)$/.test(flag))
        out.dynamic = true;
      continue;
    }
    const value =
      w.value.startsWith("--") && eq > 0
        ? { value: w.value.slice(eq + 1), dynamic: w.dynamic }
        : args[++i];
    if (!value) continue;
    if (value.dynamic) out.dynamic = true;
    else out[key] = value.value;
  }
  if (out.bodyFile !== null) {
    if (out.bodyFile === "-") {
      if (sc.heredoc !== null) out.body = sc.heredoc;
      else out.dynamic = true;
      out.bodyFile = null;
    } else if (writes.has(out.bodyFile)) {
      const written = writes.get(out.bodyFile) ?? null;
      if (written === null) out.dynamic = true;
      else out.body = written;
      out.bodyFile = null;
    }
  }
  return out;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { readFileSync } = await import("node:fs");
  process.stdout.write(`${JSON.stringify(parsePrCreate(readFileSync(0, "utf8")))}\n`);
}
