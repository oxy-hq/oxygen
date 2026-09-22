//! Source scans for the coverage tests: drop a source's comments, pull its string literals
//! or a Rust function's body. `shape_zoo_coverage` reads Rust and `canary_coverage` reads
//! TypeScript, and the two agree on `//` and `/* */` comments and on `"…"` strings; they
//! differ on `'…'` — a char literal or a lifetime in Rust, a string in TypeScript — and on
//! the backtick template string only TypeScript has. Literals are copied whole, so a comment
//! marker inside one never counts and a comment can never contribute a literal — the
//! property both tests rest on. Files come from `super::read_repo_file`.
//!
//! `host_call_attrs.rs` carries a third, smaller copy in its unit-test module: a `src`
//! unit test cannot reach an integration binary's `common`.

/// Which literals the scanned source has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
}

/// `src` without `//` and `/* */` comments; literals copied whole.
pub fn strip_comments(src: &str, lang: Lang) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                let mut j = i + 2;
                while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                    j += 1;
                }
                i = j + 2;
            }
            _ => {
                let end = literal_end(&chars, i, lang).unwrap_or(i + 1);
                out.extend(&chars[i..end]);
                i = end;
            }
        }
    }
    out
}

/// Every `"…"` literal in comment-free `s`, in order, `\"` unescaped. Other literals
/// (`'…'`, and a backtick string in TypeScript) are skipped whole, so a `"` inside one
/// never opens a string.
pub fn string_literals(s: &str, lang: Lang) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < chars.len() {
        match literal_end(&chars, i, lang) {
            Some(end) if chars[i] == '"' => {
                let text: String = chars[i + 1..end.saturating_sub(1).max(i + 1)]
                    .iter()
                    .collect();
                out.push(text.replace("\\\"", "\""));
                i = end;
            }
            Some(end) => i = end,
            None => i += 1,
        }
    }
    out
}

/// The body of the first `fn <name>(` in comment-free Rust `src`, braces matched outside
/// literals. Rust only — the `fn` keyword is what it finds — hence the name, where the
/// siblings take a [`Lang`].
pub fn rust_fn_body(src: &str, name: &str) -> String {
    let at = src
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("`fn {name}` not found"));
    let chars: Vec<char> = src[at..].chars().collect();
    let open = chars
        .iter()
        .position(|c| *c == '{')
        .expect("a function body");
    let (mut depth, mut i) = (0usize, open);
    while i < chars.len() {
        match chars[i] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return chars[open..=i].iter().collect();
                }
            }
            _ => {
                if let Some(end) = literal_end(&chars, i, Lang::Rust) {
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    panic!("unbalanced braces in `fn {name}`")
}

/// Index just past the literal that opens at `start`, if one does: `"…"` in either
/// language; `'…'` as a char literal (or the lone `'` of a lifetime) in Rust and as a
/// string in TypeScript; `` `…` `` in TypeScript.
fn literal_end(chars: &[char], start: usize, lang: Lang) -> Option<usize> {
    match (chars[start], lang) {
        ('"', _) => Some(quoted_end(chars, start, '"')),
        ('\'', Lang::Rust) => Some(char_end(chars, start)),
        ('\'', Lang::TypeScript) => Some(quoted_end(chars, start, '\'')),
        ('`', Lang::TypeScript) => Some(quoted_end(chars, start, '`')),
        _ => None,
    }
}

/// Index just past the literal quoted with `quote` that opens at `start`; the end of the
/// input when it never closes.
fn quoted_end(chars: &[char], start: usize, quote: char) -> usize {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

/// Index just past a Rust char literal at `start` (`'x'`, `'\n'`, `'\''`), or `start + 1`
/// for a lifetime.
fn char_end(chars: &[char], start: usize) -> usize {
    match (chars.get(start + 1), chars.get(start + 2)) {
        (Some('\\'), _) => chars
            .get(start + 3..)
            .and_then(|rest| rest.iter().position(|c| *c == '\''))
            .map_or(chars.len(), |p| start + 4 + p),
        (Some(_), Some('\'')) => start + 3,
        _ => start + 1,
    }
}
