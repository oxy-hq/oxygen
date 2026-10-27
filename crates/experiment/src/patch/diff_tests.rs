use super::diff::unified_diff;

#[test]
fn identical_text_has_no_diff() {
    assert_eq!(unified_diff("x.yml", "a\nb\n", "a\nb\n"), "");
}

#[test]
fn a_single_changed_line_is_a_one_hunk_diff_with_context() {
    assert_eq!(
        unified_diff("x.yml", "a\nb\nc\n", "a\nB\nc\n"),
        "--- a/x.yml\n+++ b/x.yml\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"
    );
}

#[test]
fn pure_insertions_are_marked_as_additions_only() {
    let before = "1\n2\n3\n4\n5\n6\n7\n";
    let after = "1\n2\n3\n4\nnew\n5\n6\n7\n";
    assert_eq!(
        unified_diff("x.yml", before, after),
        "--- a/x.yml\n+++ b/x.yml\n@@ -2,6 +2,7 @@\n 2\n 3\n 4\n+new\n 5\n 6\n 7\n"
    );
}

#[test]
fn an_empty_side_has_a_zero_zero_range() {
    assert_eq!(
        unified_diff("x.yml", "", "a\n"),
        "--- a/x.yml\n+++ b/x.yml\n@@ -0,0 +1,1 @@\n+a\n"
    );
}

#[test]
fn crlf_lines_keep_their_carriage_return() {
    assert_eq!(
        unified_diff("x.yml", "a\r\nb\r\n", "a\r\nB\r\n"),
        "--- a/x.yml\n+++ b/x.yml\n@@ -1,2 +1,2 @@\n a\r\n-b\r\n+B\r\n"
    );
}

#[test]
fn a_changed_unterminated_last_line_is_marked() {
    assert_eq!(
        unified_diff("x.yml", "a\nb", "a\nb\nc"),
        "--- a/x.yml\n+++ b/x.yml\n@@ -1,2 +1,3 @@\n a\n-b\n\\ No newline at end of file\n+b\n+c\n\\ No newline at end of file\n"
    );
}
