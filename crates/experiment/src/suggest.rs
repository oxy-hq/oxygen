//! Near-miss suggestions for a unit name that matched nothing.
//!
//! A suggestion is a hint attached to a refusal; it never decides one. A name is
//! refused whenever it is not an exact, case-sensitive match for a panel unit,
//! whether or not anything is suggested.

/// A panel name is suggested when it is within this many single-character
/// edits of the unmatched name once both are trimmed and lowercased.
pub(crate) const MAX_EDITS: usize = 2;
/// At most this many names are put in a message.
pub(crate) const MAX_SUGGESTIONS: usize = 3;

fn fold(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Levenshtein distance over `char`s.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The panel names closest to `name`, nearest first: ranked by folded distance,
/// then raw distance (so `bondi` outranks `bondI` for `bondi `), then name.
pub(crate) fn closest(name: &str, panel: &[String]) -> Vec<String> {
    let wanted = fold(name);
    let mut near: Vec<(usize, usize, &String)> = panel
        .iter()
        .filter_map(|candidate| {
            let folded = edit_distance(&wanted, &fold(candidate));
            (folded <= MAX_EDITS).then(|| (folded, edit_distance(name, candidate), candidate))
        })
        .collect();
    near.sort();
    near.dedup_by(|a, b| a.2 == b.2);
    near.into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, _, c)| c.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn edit_distance_counts_single_character_edits() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", "abc"), 0);
    }

    #[test]
    fn case_and_surrounding_whitespace_cost_nothing() {
        assert_eq!(closest("  BONDI ", &panel(&["bondi", "manly"])), ["bondi"]);
    }

    #[test]
    fn a_name_more_than_two_edits_away_is_not_suggested() {
        assert!(closest("bondi", &panel(&["coogee"])).is_empty());
    }
}
