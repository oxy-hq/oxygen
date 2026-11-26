//! Quoting a view's `table:` for the probes that query it directly
//! (`check_data_freshness`, the ambient freshness hint, `sample_columns`).

/// `name` as a quoted SQL relation, one identifier per dot-separated part:
/// `amazon_sp.fba_inventory` becomes `"amazon_sp"."fba_inventory"`. Quoting
/// the whole dotted name as one identifier names a table whose name contains
/// a dot, which does not exist, and DuckDB then tries it as a file path.
/// airlayer splits `table:` on dots the same way when it builds a semantic
/// query, so a probe reads the relation the view's queries read.
///
/// A name without a dot, or one that already carries a `"`, is quoted whole,
/// exactly as before: a dot inside an author's own quotes is not a separator.
pub fn quote_relation(name: &str) -> String {
    let quote = |part: &str| format!("\"{}\"", part.replace('"', "\"\""));
    if name.contains('"') || !name.contains('.') {
        return quote(name);
    }
    name.split('.').map(quote).collect::<Vec<_>>().join(".")
}

/// Whether `view` is defined by `sql:` rather than `table:`: it names no
/// relation a probe can read, and its watermark is a column of its SQL's
/// output only.
pub fn defined_by_sql(view: &oxy_airlayer_compat::schema::models::View) -> bool {
    view.table.is_none() && view.sql.is_some()
}

#[cfg(test)]
mod tests {
    use super::quote_relation;

    #[test]
    fn a_single_name_is_quoted_whole_as_before() {
        assert_eq!(quote_relation("orders"), "\"orders\"");
    }

    #[test]
    fn a_schema_qualified_name_quotes_each_part() {
        assert_eq!(
            quote_relation("amazon_sp.fba_inventory"),
            "\"amazon_sp\".\"fba_inventory\""
        );
        assert_eq!(quote_relation("db.sch.t"), "\"db\".\"sch\".\"t\"");
    }

    #[test]
    fn a_name_with_its_own_quotes_is_escaped_whole() {
        assert_eq!(quote_relation("my\"t.x"), "\"my\"\"t.x\"");
    }
}
