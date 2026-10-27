use super::fixtures::*;
use super::*;

const NEW_QUALITATIVE_EDGE: &str = "\
      - measure: store_days.promo_redemptions
        coefficient: 4.25
        form: linear
        direction: positive
        strength: strong
        description: Each redemption pulls a beverage add-on.
        refs:
          - experiment:beverage_promo_pilot@v3
";

fn edit() -> CoefficientEdit {
    CoefficientEdit {
        file_path: "views/store_days.view.yml".into(),
        target_measure: TARGET.into(),
        driver_measure: DRIVER.into(),
        coefficient: 4.25,
        refs_entry: REFS_ENTRY.into(),
        replaces: None, // the editor ignores it: it rewrites the edge, never describes the prior
    }
}

/// `VIEW` with the qualitative edge swapped for `edge`.
fn with_edge(edge: &str) -> String {
    VIEW.replace(QUALITATIVE_EDGE, edge)
}

#[test]
fn a_qualitative_edge_gains_coefficient_form_and_refs_and_nothing_else_moves() {
    let out = apply_edit(VIEW, &edit()).expect("edits");
    assert_eq!(out, with_edge(NEW_QUALITATIVE_EDGE));
}

#[test]
fn only_the_named_measures_edge_is_edited() {
    // `entree_revenue` carries an edge from the SAME driver; it must stay byte-identical.
    let out = apply_edit(VIEW, &edit()).unwrap();
    let decoy = &VIEW[VIEW.find("  - name: entree_revenue").unwrap()..];
    assert!(
        out.ends_with(decoy),
        "the decoy measure's block changed:\n{out}"
    );
    assert!(out.starts_with(&VIEW[..VIEW.find("  - name: beverages_revenue").unwrap()]));
}

#[test]
fn an_existing_coefficient_is_replaced_in_place_keeping_its_comment_and_the_refs_list() {
    let before = "\
      - measure: store_days.promo_redemptions
        coefficient: 1.5 # ops guess
        form: linear
        refs:
          - https://wiki.example/promo
";
    let after = "\
      - measure: store_days.promo_redemptions
        coefficient: 4.25 # ops guess
        form: linear
        refs:
          - https://wiki.example/promo
          - experiment:beverage_promo_pilot@v3
";
    assert_eq!(
        apply_edit(&with_edge(before), &edit()).unwrap(),
        with_edge(after)
    );
}

#[test]
fn a_single_line_flow_refs_list_is_extended_in_place() {
    let before = "\
      - measure: store_days.promo_redemptions
        form: linear
        refs: [https://wiki.example/promo]
";
    let after = "\
      - measure: store_days.promo_redemptions
        coefficient: 4.25
        form: linear
        refs: [https://wiki.example/promo, \"experiment:beverage_promo_pilot@v3\"]
";
    assert_eq!(
        apply_edit(&with_edge(before), &edit()).unwrap(),
        with_edge(after)
    );
}

#[test]
fn applying_twice_changes_nothing_the_second_time() {
    let once = apply_edit(VIEW, &edit()).unwrap();
    assert_eq!(apply_edit(&once, &edit()).unwrap(), once);
}

#[test]
fn line_endings_and_the_trailing_newline_are_preserved() {
    let crlf = VIEW.replace('\n', "\r\n");
    let out = apply_edit(&crlf, &edit()).unwrap();
    assert_eq!(out, with_edge(NEW_QUALITATIVE_EDGE).replace('\n', "\r\n"));
    let no_newline = VIEW.trim_end_matches('\n');
    assert!(!apply_edit(no_newline, &edit()).unwrap().ends_with('\n'));
}

#[test]
fn non_linear_edge_is_refused_by_name() {
    let text = with_edge(
        "\
      - measure: store_days.promo_redemptions
        form: log-log
        coefficient: 1.2
",
    );
    assert_eq!(
        apply_edit(&text, &edit()),
        Err(PatchRefusal::NonLinearForm {
            form: "log-log".into()
        })
    );
}

#[test]
fn an_edge_declaring_coefficients_is_refused() {
    let text = with_edge(
        "\
      - measure: store_days.promo_redemptions
        form: quadratic
        coefficients: [1.0, 2.0]
",
    );
    assert_eq!(
        apply_edit(&text, &edit()),
        Err(PatchRefusal::DeclaresCoefficients)
    );
}

#[test]
fn a_flow_style_edge_is_not_edited_and_says_so() {
    let text =
        with_edge("      - { measure: store_days.promo_redemptions, direction: positive }\n");
    assert!(matches!(
        apply_edit(&text, &edit()),
        Err(PatchRefusal::EdgeNotInText { .. })
    ));
}

#[test]
fn a_scalar_refs_is_refused_rather_than_guessed() {
    let text = with_edge(
        "\
      - measure: store_days.promo_redemptions
        refs: https://wiki.example/promo
",
    );
    assert!(matches!(
        apply_edit(&text, &edit()),
        Err(PatchRefusal::UnsupportedLayout { .. })
    ));
}

#[test]
fn a_missing_edge_or_measure_is_edge_not_in_text() {
    let mut e = edit();
    e.driver_measure = "store_days.someone_else".into();
    assert!(matches!(
        apply_edit(VIEW, &e),
        Err(PatchRefusal::EdgeNotInText { .. })
    ));
    let mut e = edit();
    e.target_measure = "store_days.nope".into();
    assert!(matches!(
        apply_edit(VIEW, &e),
        Err(PatchRefusal::EdgeNotInText { .. })
    ));
}

#[test]
fn render_patch_returns_the_patched_text_and_a_git_appliable_diff() {
    let p = render_patch(VIEW, &edit()).unwrap();
    assert_eq!(p.patched, with_edge(NEW_QUALITATIVE_EDGE));
    assert_eq!(
        p.diff,
        "\
--- a/views/store_days.view.yml
+++ b/views/store_days.view.yml
@@ -11,9 +11,13 @@
     drivers:
       # declared by ops, 2026-01
       - measure: store_days.promo_redemptions
+        coefficient: 4.25
+        form: linear
         direction: positive
         strength: strong
         description: Each redemption pulls a beverage add-on.
+        refs:
+          - experiment:beverage_promo_pilot@v3
       - measure: store_days.foot_traffic
         coefficient: 0.5
         form: linear
"
    );
}

/// A view whose only edge is the last thing in the file.
const TAIL: &str = "\
name: store_days
measures:
  - name: beverages_revenue
    drivers:
      - measure: store_days.promo_redemptions
        direction: positive";

/// Writes `before` to a scratch dir and asks real `git apply --check` to accept `diff`;
/// then applies it and returns the file's bytes.
fn git_apply(before: &str, diff: &str) -> String {
    let dir =
        std::env::temp_dir().join(format!("oxy-patch-{}-{}", std::process::id(), before.len()));
    std::fs::create_dir_all(dir.join("views")).unwrap();
    std::fs::write(dir.join("views/store_days.view.yml"), before).unwrap();
    std::fs::write(dir.join("p.diff"), diff).unwrap();
    let run = |args: &[&str]| {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    };
    run(&["apply", "--check", "p.diff"]);
    run(&["apply", "p.diff"]);
    let out = std::fs::read_to_string(dir.join("views/store_days.view.yml")).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    out
}

#[test]
fn the_diff_applies_with_git_to_a_crlf_file() {
    let crlf = VIEW.replace('\n', "\r\n");
    let p = render_patch(&crlf, &edit()).unwrap();
    assert_eq!(git_apply(&crlf, &p.diff), p.patched);
}

#[test]
fn the_diff_applies_with_git_when_the_edge_ends_a_file_with_no_trailing_newline() {
    let p = render_patch(TAIL, &edit()).unwrap();
    assert!(!p.patched.ends_with('\n'));
    assert!(
        p.diff.contains("\\ No newline at end of file"),
        "{}",
        p.diff
    );
    assert_eq!(git_apply(TAIL, &p.diff), p.patched);
}

#[test]
fn mixed_line_endings_are_kept_line_by_line() {
    // LF everywhere except two CRLF lines: one inside the edge, one outside it.
    let text = VIEW
        .replace(
            "        direction: positive\n",
            "        direction: positive\r\n",
        )
        .replace("name: store_days\n", "name: store_days\r\n");
    let out = apply_edit(&text, &edit()).unwrap();
    // inserted lines take the ending of the line they follow (the `measure:` line is LF)
    let expected = text.replace(
        "        direction: positive\r\n",
        "        coefficient: 4.25\n        form: linear\n        direction: positive\r\n",
    );
    let expected = expected.replace(
        "Each redemption pulls a beverage add-on.\n",
        "Each redemption pulls a beverage add-on.\n        refs:\n          - experiment:beverage_promo_pilot@v3\n",
    );
    assert_eq!(out, expected);
    assert!(out.starts_with("name: store_days\r\n# Daily"));
    let p = render_patch(&text, &edit()).unwrap();
    assert_eq!(git_apply(&text, &p.diff), p.patched);
}

#[test]
fn a_tab_before_a_comment_is_a_comment() {
    let before = "\
      - measure: store_days.promo_redemptions\t# the edge
        coefficient: 1.5\t# guess
        form: linear\t# x
";
    let after = "\
      - measure: store_days.promo_redemptions\t# the edge
        coefficient: 4.25\t# guess
        form: linear\t# x
        refs:
          - experiment:beverage_promo_pilot@v3
";
    assert_eq!(
        apply_edit(&with_edge(before), &edit()).unwrap(),
        with_edge(after)
    );
}
