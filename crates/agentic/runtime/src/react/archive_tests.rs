use chrono::{TimeDelta, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use super::*;

const RUN: &str = "0b6a1f2e-6a43-4c4e-9b1e-1d1f3f6c2a10";

fn result(rows: Vec<Vec<Value>>) -> ArchivedResult {
    ArchivedResult {
        sql: "select region, revenue from sales".into(),
        columns: vec!["region".into(), "revenue".into()],
        rows: std::sync::Arc::new(rows),
        truncated: false,
        created_at: Utc::now(),
    }
}

fn req(offset: usize, limit: usize, sort: Option<usize>, desc: bool) -> PageRequest {
    PageRequest {
        offset,
        limit,
        sort,
        desc,
    }
}

#[test]
fn a_key_names_one_file_and_refuses_anything_else() {
    let ws = Uuid::nil();
    let key = ArchiveKey::new(ws, RUN, "r12").unwrap();
    assert_eq!(key.path(), format!("{ws}/{RUN}/r12"));
    for bad in ["r", "12", "r1/../r2", "r1.json", "R1", "r-1", ""] {
        assert!(ArchiveKey::new(ws, RUN, bad).is_err(), "{bad:?}");
    }
    assert!(ArchiveKey::new(ws, "../other", "r1").is_err());
}

#[test]
fn a_column_is_a_number_only_when_every_value_is() {
    let rows = vec![
        vec![json!("a"), json!(1), json!(null)],
        vec![json!("b"), json!(-2.5), json!(null)],
        vec![json!(null), json!(null), json!("x")],
    ];
    assert_eq!(
        column_kinds(3, &rows),
        vec![ColumnKind::Text, ColumnKind::Number, ColumnKind::Text]
    );
}

/// Values, negatives included, come back as they were stored.
#[test]
fn a_page_slices_the_rows_as_stored() {
    let rows: Vec<Vec<Value>> = (0..7)
        .map(|i| vec![json!(format!("r{i}")), json!(-i)])
        .collect();
    let r = result(rows.clone());
    let p = page(&r, &req(2, 3, None, false), Utc::now()).unwrap();
    assert_eq!(p.rows, rows[2..5].to_vec());
    assert_eq!((p.offset, p.row_count), (2, 7));
    let last = page(&r, &req(6, 3, None, false), Utc::now()).unwrap();
    assert_eq!(last.rows, rows[6..].to_vec());
    assert!(
        page(&r, &req(9, 3, None, false), Utc::now())
            .unwrap()
            .rows
            .is_empty()
    );
}

/// Sorting is numeric for numbers, nulls first, and equal cells keep row
/// order, so walking the pages of a sort sees every row exactly once.
#[test]
fn sorted_pages_cover_every_row_once() {
    let rows = vec![
        vec![json!("a"), json!(10)],
        vec![json!("b"), json!(-3)],
        vec![json!("c"), json!(null)],
        vec![json!("d"), json!(10)],
        vec![json!("e"), json!(2.5)],
    ];
    let r = result(rows);
    let names = |p: &ResultPage| -> Vec<String> {
        p.rows
            .iter()
            .map(|r| r[0].as_str().unwrap().to_string())
            .collect()
    };
    let asc = page(&r, &req(0, 10, Some(1), false), Utc::now()).unwrap();
    assert_eq!(names(&asc), ["c", "b", "e", "a", "d"]);
    let desc = page(&r, &req(0, 10, Some(1), true), Utc::now()).unwrap();
    assert_eq!(names(&desc), ["a", "d", "e", "b", "c"]);
    let mut seen = Vec::new();
    for offset in (0..5).step_by(2) {
        seen.extend(names(
            &page(&r, &req(offset, 2, Some(1), true), Utc::now()).unwrap(),
        ));
    }
    assert_eq!(seen, names(&desc));
}

#[test]
fn a_page_refuses_a_sort_column_out_of_range_and_caps_its_size() {
    let r = result((0..900).map(|i| vec![json!("x"), json!(i)]).collect());
    let err = page(&r, &req(0, 10, Some(2), false), Utc::now()).unwrap_err();
    assert!(matches!(err, ArchiveError::BadRequest(_)), "{err}");
    let big = page(&r, &req(0, 100_000, None, false), Utc::now()).unwrap();
    assert_eq!(big.rows.len(), MAX_PAGE_ROWS);
    let zero = page(&r, &req(0, 0, None, false), Utc::now()).unwrap();
    assert_eq!(zero.rows.len(), 1, "a zero limit still sends a row");
}

#[test]
fn a_result_older_than_its_ttl_answers_expired_with_its_sql() {
    let mut r = result(vec![vec![json!("a"), json!(1)]]);
    r.created_at = Utc::now() - TimeDelta::days(31);
    let err = page(&r, &req(0, 10, None, false), Utc::now()).unwrap_err();
    assert_eq!(
        err,
        ArchiveError::Expired {
            sql: "select region, revenue from sales".into()
        }
    );
    r.created_at = Utc::now() - TimeDelta::days(29);
    assert!(page(&r, &req(0, 10, None, false), Utc::now()).is_ok());
}

/// A join can return two columns with one name: the sort names a column by
/// its place, so the second `id` sorts by the second `id`.
#[test]
fn a_sort_picks_its_column_by_place_not_name() {
    let mut r = result(vec![
        vec![json!(1), json!(30)],
        vec![json!(2), json!(10)],
        vec![json!(3), json!(20)],
    ]);
    r.columns = vec!["id".into(), "id".into()];
    let p = page(&r, &req(0, 10, Some(1), false), Utc::now()).unwrap();
    let firsts: Vec<i64> = p.rows.iter().map(|row| row[0].as_i64().unwrap()).collect();
    assert_eq!(firsts, [2, 3, 1]);
}
