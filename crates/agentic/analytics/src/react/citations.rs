//! `{{result:<id>}}` markers: how the model says which results it used.

const OPEN: &str = "{{result:";
const CLOSE: &str = "}}";

/// The results an answer cites.
#[derive(Debug, Default, PartialEq)]
pub struct Citations {
    /// Cited ids in order of first appearance.
    pub cited: Vec<String>,
    /// The id marked `|primary`, the last such marker when several.
    pub primary: Option<String>,
}

/// One well-formed marker at the start of `s`: `(id, primary, length)`.
fn marker_at(s: &str) -> Option<(&str, bool, usize)> {
    let inner = s.strip_prefix(OPEN)?;
    let end = inner.find(CLOSE)?;
    let body = &inner[..end];
    let (id, primary) = match body.split_once('|') {
        Some((id, flag)) => (id, flag.trim() == "primary"),
        None => (body, false),
    };
    let id = id.trim();
    if id.is_empty() || id.contains(['{', '}']) {
        return None;
    }
    Some((id, primary, OPEN.len() + end + CLOSE.len()))
}

/// The ids `{{result:<id>}}` markers cite, in order of first appearance, and
/// the one marked `|primary`. Malformed markers (no closing braces, empty id)
/// are ignored.
pub fn parse_citations(text: &str) -> Citations {
    let mut out = Citations::default();
    let mut i = 0;
    while let Some(off) = text[i..].find(OPEN) {
        let at = i + off;
        match marker_at(&text[at..]) {
            Some((id, primary, len)) => {
                if !out.cited.iter().any(|c| c == id) {
                    out.cited.push(id.to_string());
                }
                if primary {
                    out.primary = Some(id.to_string());
                }
                i = at + len;
            }
            None => i = at + OPEN.len(),
        }
    }
    out
}

/// The answer text with every well-formed `{{result:<id>}}` marker removed.
/// A space before a marker goes when punctuation or the end follows it, and a
/// doubled space the removal would leave collapses. Malformed markers stay.
pub fn strip_citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(off) = rest.find(OPEN) {
        out.push_str(&rest[..off]);
        let tail = &rest[off..];
        let Some((_, _, len)) = marker_at(tail) else {
            out.push_str(OPEN);
            rest = &tail[OPEN.len()..];
            continue;
        };
        rest = &tail[len..];
        let next = rest.chars().next();
        if out.ends_with(' ') && next.is_none_or(|c| c.is_whitespace() || ".,;:!?)".contains(c)) {
            out.pop();
        }
    }
    out.push_str(rest);
    out
}

/// The result the answer rests on: the primary, else the last cited.
pub fn answer_result_id(c: &Citations) -> Option<&str> {
    c.primary
        .as_deref()
        .or_else(|| c.cited.last().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_prefer_primary_then_last() {
        let c = parse_citations("a {{result:r1}} b {{result:r2|primary}} c {{result:r3}}");
        assert_eq!(c.cited, vec!["r1", "r2", "r3"]);
        assert_eq!(answer_result_id(&c), Some("r2"));
        let c = parse_citations("a {{result:r1}} b {{result:r3}}");
        assert_eq!(answer_result_id(&c), Some("r3"));
        assert_eq!(answer_result_id(&parse_citations("no markers")), None);
    }

    #[test]
    fn strip_removes_markers() {
        assert_eq!(
            strip_citations("Total is 5 {{result:r1|primary}}."),
            "Total is 5."
        );
        assert_eq!(strip_citations("a {{result:r1}} b"), "a b");
    }

    #[test]
    fn malformed_markers_are_left_alone() {
        assert_eq!(strip_citations("a {{result:r1 b"), "a {{result:r1 b");
        assert!(parse_citations("{{result:}}").cited.is_empty());
    }
}
