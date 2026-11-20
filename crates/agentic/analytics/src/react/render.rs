//! Compact, pipe-separated tables for the model to read.

use serde_json::Value;

/// A header line, then at most `limit` rows, then `… <k> more rows` when cut.
pub fn render_table(columns: &[String], rows: &[Vec<Value>], limit: usize) -> String {
    let header: Vec<String> = columns.iter().map(|c| escape(c)).collect();
    let mut out = header.join(" | ");
    for row in rows.iter().take(limit) {
        out.push('\n');
        let cells: Vec<String> = row.iter().map(render_cell).collect();
        out.push_str(&cells.join(" | "));
    }
    if rows.len() > limit {
        out.push_str(&format!("\n… {} more rows", rows.len() - limit));
    }
    out
}

/// A cell on one line that can't be read as a column break: `|` becomes
/// `\|`, and each line break (`\r\n`, `\n`, `\r`) becomes `⏎`.
fn escape(s: &str) -> String {
    s.replace('|', "\\|")
        .replace("\r\n", "⏎")
        .replace(['\n', '\r'], "⏎")
}

/// Strings bare (escaped), `NULL` for null, and whole floats without the `.0` the
/// connector's `f64` cells would otherwise carry.
fn render_cell(v: &Value) -> String {
    match v {
        Value::String(s) => escape(s),
        Value::Null => "NULL".to_owned(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => escape(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn react_render_table_cuts_at_the_limit() {
        let cols = vec!["id".to_owned(), "name".to_owned()];
        let rows: Vec<Vec<Value>> = (0..3).map(|i| vec![json!(i as f64), json!(null)]).collect();
        assert_eq!(
            render_table(&cols, &rows, 2),
            "id | name\n0 | NULL\n1 | NULL\n… 1 more rows"
        );
        assert_eq!(
            render_table(&cols, &[vec![json!(1.5), json!("a")]], 5),
            "id | name\n1.5 | a"
        );
    }

    #[test]
    fn react_render_table_escapes_pipes_and_newlines_in_cells() {
        let cols = vec!["a|b".to_owned(), "note".to_owned()];
        let rows = vec![vec![json!("x | y"), json!("line1\nline2\r\nline3")]];
        assert_eq!(
            render_table(&cols, &rows, 5),
            "a\\|b | note\nx \\| y | line1⏎line2⏎line3"
        );
    }
}
