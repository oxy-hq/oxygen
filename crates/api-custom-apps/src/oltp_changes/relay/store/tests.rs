//! Unit tests of the store side's pure parts.

use oxy_oltp::changes::Column;

use super::captured;

fn column(attnum: i16, name: &str, type_name: &str) -> Column {
    Column {
        attnum,
        name: name.to_owned(),
        type_name: type_name.to_owned(),
    }
}

#[test]
fn a_shape_captures_its_columns_less_the_excluded() {
    let columns = r#"[{"attnum":1,"name":"id","type":"bigint"},
        {"attnum":2,"name":"secret","type":"text"},
        {"attnum":3,"name":"total","type":"numeric(10,2)"}]"#;
    assert_eq!(
        captured(columns, &["secret".to_owned()]),
        Ok(vec![
            column(1, "id", "bigint"),
            column(3, "total", "numeric(10,2)")
        ])
    );
    assert_eq!(captured(columns, &[]).map(|c| c.len()), Ok(3));
}

#[test]
fn a_shape_that_is_not_columns_is_an_error() {
    assert!(captured("{}", &[]).is_err());
}
