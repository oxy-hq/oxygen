// Unified-diff parsing: which lines a PR changed, and which a review comment may anchor to.

import type { DiffFile, Hunk } from "./types.ts";

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

interface Cursor {
  old: number;
  new: number;
}

function unquote(p: string): string {
  return p.startsWith('"') ? (JSON.parse(p) as string) : p;
}

function newFile(header: string): DiffFile {
  // `diff --git a/x b/x`; renames carry their own lines below.
  const m = header.match(/^diff --git ("?a\/.+?"?) ("?b\/.+"?)$/);
  const strip = (s: string) => unquote(s).replace(/^[ab]\//, "");
  return {
    path: m?.[2] ? strip(m[2]) : header,
    oldPath: m?.[1] ? strip(m[1]) : header,
    status: "modified",
    binary: false,
    additions: 0,
    deletions: 0,
    hunks: []
  };
}

function applyMeta(file: DiffFile, line: string): void {
  if (line.startsWith("new file mode")) file.status = "added";
  else if (line.startsWith("deleted file mode")) file.status = "deleted";
  else if (line.startsWith("rename from ")) {
    file.oldPath = line.slice(12);
    file.status = "renamed";
  } else if (line.startsWith("rename to ")) file.path = line.slice(10);
  else if (line.startsWith("Binary files")) file.binary = true;
  else if (line.startsWith("+++ ") && line !== "+++ /dev/null")
    file.path = unquote(line.slice(4)).replace(/^b\//, "");
}

function addLine(file: DiffFile, hunk: Hunk, line: string, pos: Cursor): void {
  const type = line[0];
  if (type === "\\") return; // "\ No newline at end of file"
  const text = line.slice(1);
  if (type === "+") {
    hunk.lines.push({ type, oldNo: null, newNo: pos.new++, text });
    file.additions++;
  } else if (type === "-") {
    hunk.lines.push({ type, oldNo: pos.old++, newNo: null, text });
    file.deletions++;
  } else {
    hunk.lines.push({ type: " ", oldNo: pos.old++, newNo: pos.new++, text });
  }
}

function parseHunk(h: RegExpMatchArray): Hunk {
  return {
    oldStart: Number(h[1]),
    oldLines: h[2] === undefined ? 1 : Number(h[2]),
    newStart: Number(h[3]),
    newLines: h[4] === undefined ? 1 : Number(h[4]),
    lines: []
  };
}

/** Parse `git diff` output into files with hunks and numbered lines. */
export function parseUnifiedDiff(text: string): DiffFile[] {
  const files: DiffFile[] = [];
  let file: DiffFile | null = null;
  let hunk: Hunk | null = null;
  const pos: Cursor = { old: 0, new: 0 };
  for (const line of text.split("\n")) {
    if (line.startsWith("diff --git ")) {
      file = newFile(line);
      files.push(file);
      hunk = null;
      continue;
    }
    if (!file) continue;
    const h = line.match(HUNK);
    if (h) {
      hunk = parseHunk(h);
      file.hunks.push(hunk);
      pos.old = hunk.oldStart;
      pos.new = hunk.newStart;
      continue;
    }
    if (!hunk) {
      applyMeta(file, line);
      continue;
    }
    // git prints an empty context line as " ", but copies that strip trailing
    // whitespace print "". While the hunk still owes lines on both sides, "" is context.
    const owed = pos.old - hunk.oldStart < hunk.oldLines && pos.new - hunk.newStart < hunk.newLines;
    if (line !== "") addLine(file, hunk, line, pos);
    else if (owed) addLine(file, hunk, " ", pos);
  }
  return files;
}

/** New-side line numbers a PR review comment can anchor to (added and context lines). */
export function commentableLines(file: DiffFile): Set<number> {
  const lines = new Set<number>();
  for (const h of file.hunks) for (const l of h.lines) if (l.newNo !== null) lines.add(l.newNo);
  return lines;
}

/**
 * The head text of new-side lines `start`..`end` when one hunk holds all of them, else
 * null. GitHub anchors a multi-line comment (and so a suggestion) only inside one hunk.
 */
export function hunkText(file: DiffFile, start: number, end: number): string | null {
  if (end < start) return null;
  for (const h of file.hunks) {
    const lines = h.lines.filter((l) => l.newNo !== null && l.newNo >= start && l.newNo <= end);
    if (lines.length === end - start + 1) return lines.map((l) => l.text).join("\n");
  }
  return null;
}

export function addedText(file: DiffFile): string {
  const out: string[] = [];
  for (const h of file.hunks) for (const l of h.lines) if (l.type === "+") out.push(l.text);
  return out.join("\n");
}

/**
 * Old-side lines a change replaced or deleted, and for pure insertions the line it was
 * inserted after: what `git blame` on the parent should be asked about.
 */
export function touchedOldLines(file: DiffFile): { removed: number[]; anchors: number[] } {
  const removed: number[] = [];
  const anchors: number[] = [];
  for (const h of file.hunks) {
    // A pure insertion (`-3,0`) names the line it follows; otherwise the hunk starts at oldStart.
    let lastOld = h.oldLines === 0 ? h.oldStart : h.oldStart - 1;
    let sawRemoval = false;
    for (const l of h.lines) {
      if (l.type === "-" && l.oldNo !== null) {
        removed.push(l.oldNo);
        sawRemoval = true;
      } else if (l.type === " " && l.oldNo !== null) {
        lastOld = l.oldNo;
      } else if (l.type === "+" && !sawRemoval && lastOld > 0) {
        anchors.push(lastOld);
      }
    }
  }
  return { removed, anchors: [...new Set(anchors)] };
}

/** Group sorted line numbers into inclusive [start, end] ranges. */
export function toRanges(numbers: readonly number[]): [number, number][] {
  const sorted = [...new Set(numbers)].sort((a, b) => a - b);
  const ranges: [number, number][] = [];
  for (const n of sorted) {
    const last = ranges.at(-1);
    if (last && n === last[1] + 1) last[1] = n;
    else ranges.push([n, n]);
  }
  return ranges;
}
