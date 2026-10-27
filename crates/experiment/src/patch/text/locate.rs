//! Locating one driver edge in block-style YAML lines.

use super::PatchRefusal;

// ── line helpers ────────────────────────────────────────────────────────────

pub(super) fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

pub(super) fn skippable(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

pub(super) fn is_dash(line: &str) -> bool {
    let t = line.trim_start_matches(' ');
    t == "-" || t.starts_with("- ")
}

/// `key: rest` where the key is a plain identifier. `rest` is what follows the colon.
pub(super) fn split_key(content: &str) -> Option<(&str, &str)> {
    let colon = content.find(':')?;
    let (key, rest) = (&content[..colon], &content[colon + 1..]);
    let key_ok = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let rest_ok = rest.is_empty() || rest.starts_with(' ');
    (key_ok && rest_ok).then_some((key, rest))
}

/// Byte index of the whitespace that opens a trailing `# comment` (space or tab).
fn comment_start(s: &str) -> Option<usize> {
    s.as_bytes()
        .windows(2)
        .position(|w| (w[0] == b' ' || w[0] == b'\t') && w[1] == b'#')
}

/// A scalar value: quotes stripped, a trailing ` # comment` removed.
pub(super) fn scalar(rest: &str) -> String {
    let t = rest.trim();
    if let Some(q) = t.chars().next().filter(|c| *c == '"' || *c == '\'') {
        if let Some(end) = t[1..].find(q) {
            return t[1..1 + end].to_string();
        }
    }
    match comment_start(t) {
        Some(i) => t[..i].trim().to_string(),
        None => t.to_string(),
    }
}

/// The trailing ` # comment` of a value, including its leading space; `""` if none.
pub(super) fn comment_of(rest: &str) -> &str {
    comment_start(rest).map(|i| &rest[i..]).unwrap_or("")
}

/// True when what follows `key:` opens a block (nothing, or only a comment).
pub(super) fn is_block_opener(rest: &str) -> bool {
    let t = rest.trim();
    t.is_empty() || t.starts_with('#')
}

// ── locating the edge ───────────────────────────────────────────────────────

/// One `- ` list entry: the lines it occupies and the column its keys sit at.
pub(super) struct Item {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) key_col: usize,
}

pub(super) struct Site {
    pub(super) item: Item,
    pub(super) measure_line: usize,
    pub(super) coefficient: Option<(usize, String)>,
    pub(super) has_coefficients: bool,
    pub(super) form: Option<(usize, String)>,
    pub(super) refs: Option<(usize, String)>,
}

/// `<key>:` at column 0 opening a block, and where that block ends.
pub(super) fn top_level_block(lines: &[String], key: &str) -> Option<(usize, usize)> {
    let open = lines.iter().position(|l| {
        indent(l) == 0 && split_key(l).is_some_and(|(k, rest)| k == key && is_block_opener(rest))
    })?;
    let end = (open + 1..lines.len())
        .find(|&i| !skippable(&lines[i]) && indent(&lines[i]) == 0 && !is_dash(&lines[i]))
        .unwrap_or(lines.len());
    Some((open, end))
}

/// The entries of the block list that starts in `[from, to)` with its dash at or
/// right of `min_col`. Blank and comment lines never end an entry.
pub(super) fn list_items(lines: &[String], from: usize, to: usize, min_col: usize) -> Vec<Item> {
    let Some(first) = (from..to).find(|&i| !skippable(&lines[i])) else {
        return vec![];
    };
    if !is_dash(&lines[first]) || indent(&lines[first]) < min_col {
        return vec![];
    }
    let dash_col = indent(&lines[first]);
    let mut items: Vec<Item> = vec![];
    let mut i = first;
    while i < to {
        let line = &lines[i];
        if skippable(line) {
            i += 1;
            continue;
        }
        let col = indent(line);
        if col < dash_col || (col == dash_col && !is_dash(line)) {
            break;
        }
        if col == dash_col {
            if let Some(prev) = items.last_mut() {
                prev.end = i;
            }
            let after = &line[col + 1..];
            let key_col = col + 1 + (after.len() - after.trim_start_matches(' ').len());
            items.push(Item {
                start: i,
                end: to,
                key_col,
            });
        }
        i += 1;
    }
    if let Some(last) = items.last_mut() {
        last.end = i.min(to);
    }
    items
}

/// `key:` at the entry's own key column (the first line counts, after its dash).
pub(super) fn item_key(lines: &[String], item: &Item, key: &str) -> Option<(usize, String)> {
    for i in item.start..item.end {
        let line = &lines[i];
        if skippable(line) || (i != item.start && indent(line) != item.key_col) {
            continue;
        }
        let Some(content) = line.get(item.key_col..) else {
            continue;
        };
        if let Some((k, rest)) = split_key(content) {
            if k == key {
                return Some((i, rest.to_string()));
            }
        }
    }
    None
}

pub(super) fn locate(lines: &[String], measure: &str, driver: &str) -> Result<Site, PatchRefusal> {
    let missing = || PatchRefusal::EdgeNotInText {
        measure: measure.to_string(),
        driver: driver.to_string(),
    };
    let (open, end) = top_level_block(lines, "measures").ok_or_else(missing)?;
    let measures = list_items(lines, open + 1, end, 0);
    let m = measures
        .iter()
        .find(|it| item_key(lines, it, "name").is_some_and(|(_, v)| scalar(&v) == measure))
        .ok_or_else(missing)?;
    let (drivers_line, rest) = item_key(lines, m, "drivers").ok_or_else(missing)?;
    if !is_block_opener(&rest) {
        return Err(PatchRefusal::UnsupportedLayout {
            what: "`drivers:` is written inline, not as a block list".to_string(),
        });
    }
    let mut hits = list_items(lines, drivers_line + 1, m.end, m.key_col)
        .into_iter()
        .filter(|it| item_key(lines, it, "measure").is_some_and(|(_, v)| scalar(&v) == driver));
    let item = hits.next().ok_or_else(missing)?;
    if hits.next().is_some() {
        return Err(PatchRefusal::AmbiguousEdge {
            measure: measure.to_string(),
            driver: driver.to_string(),
        });
    }
    let measure_line = item_key(lines, &item, "measure")
        .map(|(i, _)| i)
        .ok_or_else(missing)?;
    Ok(Site {
        measure_line,
        coefficient: item_key(lines, &item, "coefficient"),
        has_coefficients: item_key(lines, &item, "coefficients").is_some(),
        form: item_key(lines, &item, "form"),
        refs: item_key(lines, &item, "refs"),
        item,
    })
}
