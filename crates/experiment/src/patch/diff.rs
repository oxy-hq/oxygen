//! A unified diff a person reads and `git apply` accepts.
//!
//! The common prefix and suffix are trimmed first; an LCS over what is left keeps
//! unchanged lines *inside* the hunk as context, so an edit that inserts above
//! and below an untouched block shows as two insertions, not a rewrite. One hunk
//! covers the whole change — valid, and the files this edits are small.

const CONTEXT: usize = 3;
/// Above this many table cells the LCS is skipped for a plain replace.
const MAX_CELLS: usize = 4_000_000;

enum Op<'a> {
    Same(&'a str),
    Del(&'a str),
    Add(&'a str),
}

pub fn unified_diff(path: &str, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let a: Vec<&str> = before.split_inclusive('\n').collect();
    let b: Vec<&str> = after.split_inclusive('\n').collect();
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - pre;
    let suf = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let (mid_a, mid_b) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);

    let (ctx_before, ctx_after) = (pre.min(CONTEXT), suf.min(CONTEXT));
    let start = pre - ctx_before;
    let mut body: Vec<String> = a[start..pre].iter().map(|l| line(' ', l)).collect();
    let (mut old_len, mut new_len) = (ctx_before + ctx_after, ctx_before + ctx_after);
    for op in diff_ops(mid_a, mid_b) {
        match op {
            Op::Same(l) => {
                body.push(line(' ', l));
                old_len += 1;
                new_len += 1;
            }
            Op::Del(l) => {
                body.push(line('-', l));
                old_len += 1;
            }
            Op::Add(l) => {
                body.push(line('+', l));
                new_len += 1;
            }
        }
    }
    let tail = a.len() - suf;
    body.extend(a[tail..tail + ctx_after].iter().map(|l| line(' ', l)));
    let from = |len: usize| if len == 0 { start } else { start + 1 };
    format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{},{old_len} +{},{new_len} @@\n{}",
        from(old_len),
        from(new_len),
        body.concat()
    )
}

/// One diff body line. `text` keeps its own terminator (so CRLF survives); a line
/// with none is the file's unterminated last line and gets git's marker.
fn line(tag: char, text: &str) -> String {
    if text.ends_with('\n') {
        format!("{tag}{text}")
    } else {
        format!("{tag}{text}\n\\ No newline at end of file\n")
    }
}

fn diff_ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    let (n, m) = (a.len(), b.len());
    if (n + 1) * (m + 1) > MAX_CELLS {
        return a
            .iter()
            .map(|l| Op::Del(*l))
            .chain(b.iter().map(|l| Op::Add(*l)))
            .collect();
    }
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n + m);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Op::Same(a[i]));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(Op::Del(a[i]));
            i += 1;
        } else {
            out.push(Op::Add(b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| Op::Del(*l)));
    out.extend(b[j..].iter().map(|l| Op::Add(*l)));
    out
}
