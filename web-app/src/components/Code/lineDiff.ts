/** One line of an inline diff. */
export type DiffLine =
  | { kind: "same"; text: string; oldNo: number; newNo: number }
  | { kind: "removed"; text: string; oldNo: number }
  | { kind: "added"; text: string; newNo: number };

/**
 * Past this many cells the LCS table is not worth building: the diff is shown
 * as every old line removed and every new line added, which is still correct.
 */
const MAX_CELLS = 4_000_000;

const splitLines = (text: string) => (text === "" ? [] : text.replace(/\n$/, "").split("\n"));

/**
 * A line diff of `original` → `modified`, by longest common subsequence. Line
 * numbers count from `startLine` on both sides (a snippet of a larger file).
 */
export function lineDiff(original: string, modified: string, startLine = 1): DiffLine[] {
  const a = splitLines(original);
  const b = splitLines(modified);
  const out: DiffLine[] = [];
  let oldNo = startLine;
  let newNo = startLine;
  if (a.length * b.length > MAX_CELLS) {
    for (const text of a) out.push({ kind: "removed", text, oldNo: oldNo++ });
    for (const text of b) out.push({ kind: "added", text, newNo: newNo++ });
    return out;
  }
  // lcs[i][j]: the LCS length of a[i..] and b[j..].
  const lcs = Array.from({ length: a.length + 1 }, () => new Uint32Array(b.length + 1));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      out.push({ kind: "same", text: a[i], oldNo: oldNo++, newNo: newNo++ });
      i++;
      j++;
    } else if (i < a.length && (j === b.length || lcs[i + 1][j] >= lcs[i][j + 1])) {
      // On a tie the removed line goes first, so a change reads old then new.
      out.push({ kind: "removed", text: a[i], oldNo: oldNo++ });
      i++;
    } else {
      out.push({ kind: "added", text: b[j], newNo: newNo++ });
      j++;
    }
  }
  return out;
}
