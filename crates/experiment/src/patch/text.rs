//! Line-level editing of one driver edge in a `.view.yml`.
//!
//! Block-style YAML only, and only the shapes listed in the module docs of
//! [`super`]. Anything else is a named refusal — a wrong guess here would ship
//! a coefficient onto the wrong edge.

use serde_json::Value;

use super::diff::unified_diff;
mod locate;

use super::plan::{CoefficientEdit, PatchRefusal, edge_facts};
use locate::{Item, Site, comment_of, indent, is_block_opener, is_dash, locate, scalar, skippable};

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedPatch {
    pub patched: String,
    pub diff: String,
}

pub fn render_patch(source: &str, edit: &CoefficientEdit) -> Result<RenderedPatch, PatchRefusal> {
    let patched = apply_edit(source, edit)?;
    let diff = unified_diff(&edit.file_path, source, &patched);
    Ok(RenderedPatch { patched, diff })
}

pub fn apply_edit(source: &str, edit: &CoefficientEdit) -> Result<String, PatchRefusal> {
    let measure = edit
        .target_measure
        .split_once('.')
        .map(|(_, m)| m)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| PatchRefusal::BadReference {
            reference: edit.target_measure.clone(),
        })?;
    let mut doc = Doc::parse(source);
    let site = locate(&doc.lines, measure, &edit.driver_measure)?;
    let ops = plan_ops(&doc.lines, &site, edit)?;
    apply_ops(&mut doc, ops);
    let out = doc.render();
    verify(&out, measure, edit)?;
    Ok(out)
}

// ── the document ────────────────────────────────────────────────────────────

struct Doc {
    lines: Vec<String>,
    /// The terminator each line carried (`""` for a last line without one).
    eols: Vec<String>,
}

impl Doc {
    fn parse(src: &str) -> Doc {
        let mut lines = vec![];
        let mut eols = vec![];
        for raw in src.split_inclusive('\n') {
            let (body, eol) = match raw.strip_suffix("\r\n") {
                Some(b) => (b, "\r\n"),
                None => match raw.strip_suffix('\n') {
                    Some(b) => (b, "\n"),
                    None => (raw, ""),
                },
            };
            lines.push(body.to_string());
            eols.push(eol.to_string());
        }
        Doc { lines, eols }
    }

    fn render(&self) -> String {
        self.lines
            .iter()
            .zip(&self.eols)
            .map(|(l, e)| format!("{l}{e}"))
            .collect()
    }
}

// ── editing ─────────────────────────────────────────────────────────────────

enum Op {
    Replace(usize, String),
    InsertAfter(usize, Vec<String>),
}

impl Op {
    fn index(&self) -> usize {
        match self {
            Op::Replace(i, _) | Op::InsertAfter(i, _) => *i,
        }
    }
}

/// Applied bottom-up (stable, so ops at the same line keep the order they were
/// planned in) — an earlier op never shifts a later op's line number. A line
/// keeps its own terminator; an inserted line takes the one of the line it follows.
fn apply_ops(doc: &mut Doc, mut ops: Vec<Op>) {
    ops.sort_by(|a, b| b.index().cmp(&a.index()));
    for op in ops {
        match op {
            Op::Replace(i, text) => doc.lines[i] = text,
            Op::InsertAfter(i, new) => {
                let n = new.len();
                let mut eols = vec![doc.eols[i].clone(); n];
                if doc.eols[i].is_empty() {
                    // inserting after an unterminated last line: it gains a terminator,
                    // the new last line stays unterminated
                    let fallback = if i > 0 && !doc.eols[i - 1].is_empty() {
                        doc.eols[i - 1].clone()
                    } else {
                        "\n".to_string()
                    };
                    doc.eols[i] = fallback.clone();
                    eols = vec![fallback; n];
                    if let Some(last) = eols.last_mut() {
                        last.clear();
                    }
                }
                doc.lines.splice(i + 1..i + 1, new);
                doc.eols.splice(i + 1..i + 1, eols);
            }
        }
    }
}

fn format_coefficient(x: f64) -> Result<String, PatchRefusal> {
    if !x.is_finite() {
        return Err(PatchRefusal::NotFinite);
    }
    let s = format!("{x}");
    Ok(if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    })
}

fn last_content_line(lines: &[String], item: &Item) -> usize {
    (item.start..item.end)
        .rev()
        .find(|&i| !skippable(&lines[i]))
        .unwrap_or(item.start)
}

fn plan_ops(
    lines: &[String],
    site: &Site,
    edit: &CoefficientEdit,
) -> Result<Vec<Op>, PatchRefusal> {
    if site.has_coefficients {
        return Err(PatchRefusal::DeclaresCoefficients);
    }
    if let Some((_, raw)) = &site.form {
        let form = scalar(raw);
        if form != "linear" {
            return Err(PatchRefusal::NonLinearForm { form });
        }
    }
    let value = format_coefficient(edit.coefficient)?;
    let pad = " ".repeat(site.item.key_col);
    let mut ops: Vec<Op> = vec![];
    // Planned first so, at equal line numbers, refs land below coefficient/form.
    ops.extend(refs_op(lines, site, &edit.refs_entry, &pad)?);
    let coefficient_line = format!("{pad}coefficient: {value}");
    let form_line = format!("{pad}form: linear");
    match (&site.coefficient, &site.form) {
        (Some((ci, raw)), form) => {
            let prefix = &lines[*ci][..site.item.key_col];
            ops.push(Op::Replace(
                *ci,
                format!("{prefix}coefficient: {value}{}", comment_of(raw)),
            ));
            if form.is_none() {
                ops.push(Op::InsertAfter(*ci, vec![form_line]));
            }
        }
        (None, None) => ops.push(Op::InsertAfter(
            site.measure_line,
            vec![coefficient_line, form_line],
        )),
        (None, Some(_)) => ops.push(Op::InsertAfter(site.measure_line, vec![coefficient_line])),
    }
    Ok(ops)
}

fn refs_op(
    lines: &[String],
    site: &Site,
    entry: &str,
    pad: &str,
) -> Result<Option<Op>, PatchRefusal> {
    let Some((ri, rest)) = &site.refs else {
        let last = last_content_line(lines, &site.item);
        return Ok(Some(Op::InsertAfter(
            last,
            vec![format!("{pad}refs:"), format!("{pad}  - {entry}")],
        )));
    };
    if is_block_opener(rest) {
        return Ok(block_refs_op(lines, site, *ri, entry));
    }
    let comment = comment_of(rest);
    let body = rest[..rest.len() - comment.len()].trim();
    if body.starts_with('[') && body.ends_with(']') {
        if body.contains(entry) {
            return Ok(None);
        }
        let head = body[..body.len() - 1].trim_end();
        let sep = if head.ends_with('[') { "" } else { ", " };
        let key_end = lines[*ri].find("refs:").map_or(0, |p| p + "refs:".len());
        let line = format!(
            "{} {head}{sep}\"{entry}\"]{comment}",
            &lines[*ri][..key_end]
        );
        return Ok(Some(Op::Replace(*ri, line)));
    }
    Err(PatchRefusal::UnsupportedLayout {
        what: "`refs:` is neither a block list nor a single-line flow list".to_string(),
    })
}

fn block_refs_op(lines: &[String], site: &Site, ri: usize, entry: &str) -> Option<Op> {
    let kc = site.item.key_col;
    let (mut last, mut dash_col, mut seen_dash) = (ri, kc + 2, false);
    for i in ri + 1..site.item.end {
        let line = &lines[i];
        if skippable(line) {
            continue;
        }
        let col = indent(line);
        if col > kc || (col == kc && is_dash(line)) {
            if !seen_dash && is_dash(line) {
                dash_col = col;
                seen_dash = true;
            }
            last = i;
        } else {
            break;
        }
    }
    let present = lines[ri + 1..=last].iter().any(|l| {
        l.trim_start()
            .strip_prefix("- ")
            .is_some_and(|v| scalar(v) == entry)
    });
    (!present).then(|| Op::InsertAfter(last, vec![format!("{}- {entry}", " ".repeat(dash_col))]))
}

/// The edited text must parse, and its edge must read back as intended.
fn verify(out: &str, measure: &str, edit: &CoefficientEdit) -> Result<(), PatchRefusal> {
    let bad = |what: String| PatchRefusal::EditDidNotRoundTrip { what };
    let value: Value = serde_yaml::from_str(out)
        .map_err(|e| bad(format!("the edited file no longer parses: {e}")))?;
    let facts =
        edge_facts(&value, measure, &edit.driver_measure).map_err(|e| bad(e.to_string()))?;
    let ok = facts.coefficient == Some(edit.coefficient)
        && facts.form.as_deref() == Some("linear")
        && facts.refs.iter().any(|r| r == &edit.refs_entry);
    ok.then_some(()).ok_or_else(|| {
        bad(
            "the edge does not show the new coefficient, `form: linear` and the refs entry"
                .to_string(),
        )
    })
}
