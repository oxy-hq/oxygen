//! Validate a vendor report by running the loader's own code path over it.
//!
//! Lives here, next to [`crate::source_factory`], because this crate is the
//! seam that owns oxy's dependency on the airway engine — the platform calls
//! this rather than linking `airway` itself.
//!
//! # Why not reimplement the checks
//!
//! Everything a caller would need to validate an UberEats report — the
//! 49-column map, the JE-critical column set, header detection, period
//! derivation — is `pub(crate)` inside airway. Reproducing any of it on this
//! side would put two copies of one contract in two repositories on two release
//! cadences, and the copies would be free to disagree the moment upstream added
//! a column. Running the real source instead means the answer here cannot
//! differ from what the loader will do, because it *is* what the loader does.
//!
//! The cost is a temp file: the source reads paths, not buffers. That file is
//! per-process and ephemeral — it is not the workspace working copy, `.git`, or
//! the state dir — so a caller of this stays fleet-servable.

use std::io::Write;

use airway::connector::SourceConnector;
use airway::connector::sources::ubereats::UberEatsSource;

/// What a report turned out to be, once the loader had read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedReport {
    /// The period every row will be stamped with. Derived rather than echoed:
    /// when it came from the filename this is the only place it is observable
    /// before a load.
    pub report_year: i64,
    pub report_month: u32,
    /// Rows the report yielded. Zero is a valid empty report, not a failure.
    pub rows: usize,
}

/// Errors a caller is expected to render differently.
#[derive(Debug, thiserror::Error)]
pub enum ReportValidationError {
    /// The report itself is wrong — a renamed JE-critical column, an
    /// unparseable period, a file that is not a report. The message is
    /// airway's own, so the diagnosis here matches the one a load would give.
    #[error("{0}")]
    Rejected(String),
    /// The validation could not be performed. Not the caller's fault.
    #[error("could not validate the report: {0}")]
    Unavailable(String),
}

/// The bounds both the pipeline config and the upload path enforce on a
/// caller-supplied period.
///
/// One definition because they must agree: `source_factory` refuses an
/// impossible period so a load cannot stamp a partition that does not exist,
/// and the upload path refuses one because the period is interpolated into an
/// object key — `2026.13` produces a key the source's period scan will not
/// recognize, stranding the report in the zone.
///
/// The year bound is a sanity floor, not a business rule: UberEats did not
/// exist before 2000, and 2100 is a typo either way.
pub fn check_period(year: i64, month: u32) -> Result<(), String> {
    if !(1..=12).contains(&month) {
        return Err(format!("`report_month` must be 1–12, got {month}"));
    }
    if !(2000..=2100).contains(&year) {
        return Err(format!("`report_year` must be 2000–2100, got {year}"));
    }
    Ok(())
}

/// Run `UberEatsSource` over `bytes` as if loading them.
///
/// `allowed_stores` is the PIPELINE's scoping, passed through so this reports
/// what the load will actually land rather than what a bare source would.
///
/// `filename` is preserved on the temp file because the source derives the
/// period from it when `period` is `None` — a report named
/// `2026.08 UberEats SF.csv` carries its own period, and renaming it to
/// something opaque would strand it.
pub async fn validate_ubereats_report(
    bytes: &[u8],
    filename: &str,
    period: Option<(i64, u32)>,
    allowed_stores: Option<&std::collections::HashSet<String>>,
) -> Result<ValidatedReport, ReportValidationError> {
    // The name must be a BARE file name.
    //
    // `Path::join` with an absolute path discards the tempdir entirely, and a
    // `../` component walks out of it — so an unchecked name here is an
    // arbitrary write of caller-controlled bytes to any path whose parent
    // exists, as the server user. Refused rather than sanitized: the source
    // derives the period from this name, so quietly rewriting it would change
    // which month the report claims to be.
    //
    // Guarded in this crate, not only in the HTTP caller, because this is a
    // `pub fn` whose whole argument is that it cannot disagree with the
    // loader — the next caller inherits the check rather than re-deriving it.
    let bare = std::path::Path::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|base| *base == filename);
    let Some(bare) = bare else {
        return Err(ReportValidationError::Rejected(format!(
            "`{filename}` is not a bare file name — a path component would place \
             the report outside the directory it is validated in"
        )));
    };

    let dir = tempfile::tempdir()
        .map_err(|e| ReportValidationError::Unavailable(format!("temp dir: {e}")))?;
    let path = dir.path().join(bare);
    let mut file = std::fs::File::create(&path)
        .map_err(|e| ReportValidationError::Unavailable(format!("temp file: {e}")))?;
    file.write_all(bytes)
        .map_err(|e| ReportValidationError::Unavailable(format!("temp write: {e}")))?;
    drop(file);

    let Some(path) = path.to_str() else {
        return Err(ReportValidationError::Rejected(
            "the file name is not valid UTF-8, so it cannot name an object".to_string(),
        ));
    };
    let mut source = UberEatsSource::new(path);
    if let Some((year, month)) = period {
        source = source.with_period(year, month);
    }
    // The pipeline's OWN scoping, not a bare source.
    //
    // Without it this validated a different thing from what the load will do —
    // a report spanning six stores where two are in scope reported six rows
    // and then loaded two. The whole argument for validating by running the
    // real source is that the answer cannot differ from the loader's; a
    // configured loader and an unconfigured validator break exactly that.
    if let Some(stores) = allowed_stores {
        source = source.with_allowed_stores(stores.clone());
    }

    let out = source
        .extract("ubereats_transactions", None)
        .await
        .map_err(|e| ReportValidationError::Rejected(e.to_string()))?;

    // Read the period back off a row rather than trusting the input: when it
    // came from the filename, the loader is the only thing that knows it.
    let derived = out
        .records
        .first()
        .and_then(|r| {
            Some((
                r.get("report_year")?.as_i64()?,
                u32::try_from(r.get("report_month")?.as_i64()?).ok()?,
            ))
        })
        .or(period);

    let Some((report_year, report_month)) = derived else {
        // An empty report with no period is not a failure of the report — it
        // is a request with nothing to place. Callers need to distinguish it,
        // hence a rejection rather than a silent zero-row success.
        return Err(ReportValidationError::Rejected(
            "the report has no rows and no period was given, so there is nothing to \
             derive one from — pass the period explicitly, or name the file \
             `YYYY.MM …`"
                .to_string(),
        ));
    };

    Ok(ValidatedReport {
        report_year,
        report_month,
        rows: out.records.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report carrying every JE-critical column, one row.
    fn report(store: &str) -> Vec<u8> {
        let header = [
            "Store Name",
            "Sales (excl. tax)",
            "Price adjustments (excl. tax)",
            "Offers on items (incl. tax)",
            "Tax On Offers on items",
            "Order Error Adjustments",
            "Offer Redemption Fee",
            "Marketing Adjustment",
            "Other payments",
            "Marketplace Fee",
            "Total payout",
        ];
        let mut row = vec![store.to_string()];
        row.extend(std::iter::repeat_n("0".to_string(), header.len() - 1));
        format!("{}\n{}\n", header.join(","), row.join(",")).into_bytes()
    }

    /// The same report, spanning two stores — one a pipeline owns, one it
    /// does not.
    fn report_two_stores(a: &str, b: &str) -> Vec<u8> {
        let mut out = report(a);
        let second = report(b);
        // Drop the second copy's header row: one report, two rows.
        let body = second
            .split(|c| *c == b'\n')
            .nth(1)
            .expect("the helper emits a header and a row")
            .to_vec();
        out.extend(body);
        out.push(b'\n');
        out
    }

    /// Validation must count what THIS PIPELINE will load, not what a bare
    /// source would.
    ///
    /// The endpoint's whole claim is that validation cannot disagree with the
    /// loader because it *is* the loader. It quietly stopped being true: the
    /// handler validated before reading the pipeline, so a report spanning
    /// stores the pipeline does not own was reported whole and then loaded
    /// short — a two-store report answered 2 and landed 1. Uploading is where
    /// someone decides whether the month is complete, so a row count that
    /// overstates by exactly the out-of-scope rows is the worst possible
    /// answer: it reads as a successful full month.
    ///
    /// Pinned here rather than at the handler because the argument is what
    /// carries the scoping; a caller that drops it fails this test.
    #[tokio::test]
    async fn scoping_is_applied_to_the_row_count() {
        let bytes = report_two_stores("Poke House SF", "Someone Else BBQ");

        let unscoped = validate_ubereats_report(&bytes, "2026.08 SF.csv", None, None)
            .await
            .expect("a complete report validates");
        assert_eq!(unscoped.rows, 2, "no scoping loads both stores");

        let owned = std::collections::HashSet::from(["Poke House SF".to_string()]);
        let scoped = validate_ubereats_report(&bytes, "2026.08 SF.csv", None, Some(&owned))
            .await
            .expect("a complete report validates");
        assert_eq!(
            scoped.rows, 1,
            "an out-of-scope store must not be counted — it will not be loaded"
        );
    }

    /// A caller-supplied name reaches `Path::join`, so anything but a bare
    /// base name is an arbitrary write: an absolute path DISCARDS the tempdir
    /// entirely, and `../` walks out of it. Both must be refused, and nothing
    /// may be written outside the validation directory.
    #[tokio::test]
    async fn a_filename_with_a_path_component_cannot_escape() {
        let probe = std::env::temp_dir().join("oxy-ubereats-escape-probe.csv");
        let _ = std::fs::remove_file(&probe);

        let escapes = [
            "../escape.csv".to_string(),
            "../../../../escape.csv".to_string(),
            probe.to_string_lossy().into_owned(),
            "a/b.csv".to_string(),
        ];
        for name in escapes {
            let err = validate_ubereats_report(&report("SF"), &name, Some((2026, 8)), None)
                .await
                .err()
                .unwrap_or_else(|| panic!("`{name}` must be refused, not validated"));
            assert!(
                matches!(err, ReportValidationError::Rejected(_)),
                "`{name}` must be a rejection, not an internal error: {err}"
            );
        }

        assert!(
            !probe.exists(),
            "an absolute filename wrote outside the validation directory"
        );
    }

    /// One definition of the bounds, because the upload path and the pipeline
    /// config must agree: a period the source cannot parse strands the report.
    #[test]
    fn the_period_bounds_are_shared() {
        assert!(check_period(2026, 8).is_ok());
        assert!(check_period(2000, 1).is_ok());
        assert!(check_period(2100, 12).is_ok());

        for (y, m) in [(2026, 0), (2026, 13), (2026, 99)] {
            let e = check_period(y, m).expect_err("month must be refused");
            assert!(e.contains("report_month"), "names the field: {e}");
        }
        for (y, m) in [(-5, 8), (0, 8), (1900, 8), (3000, 8)] {
            let e = check_period(y, m).expect_err("year must be refused");
            assert!(e.contains("report_year"), "names the field: {e}");
        }
    }

    #[tokio::test]
    async fn a_good_report_yields_its_period_and_row_count() {
        let out = validate_ubereats_report(&report("Poke House SF"), "2026.08 SF.csv", None, None)
            .await
            .expect("a complete report validates");
        assert_eq!(
            out,
            ValidatedReport {
                report_year: 2026,
                report_month: 8,
                rows: 1
            }
        );
    }

    /// A report in the spelling Uber has shipped since about 2026-09-07 validates.
    ///
    /// Uber renamed the order-error columns to "Chargeback Amount"
    /// (airway-internal#199). Until airway 0.1.47 every report generated after
    /// that was refused here for lacking the JE-critical "Order Error
    /// Adjustments". The fix is an alias inside airway, so this pins that the
    /// upload path receives it. The old spelling stays canonical, which is why
    /// `report()` keeps passing alongside this.
    #[tokio::test]
    async fn a_report_in_ubers_chargeback_spelling_validates() {
        let renamed = String::from_utf8(report("Poke House SF"))
            .expect("the helper emits UTF-8")
            .replacen("Order Error Adjustments", "Chargeback Amount", 1);
        assert!(
            renamed.contains("Chargeback Amount") && !renamed.contains("Order Error"),
            "the helper's header must carry the renamed column and not the old one: {renamed}"
        );
        let out = validate_ubereats_report(renamed.as_bytes(), "2026.09 SF.csv", None, None)
            .await
            .expect("a report in Uber's current spelling validates");
        assert_eq!(
            out,
            ValidatedReport {
                report_year: 2026,
                report_month: 9,
                rows: 1
            }
        );
    }

    /// Where the period comes from, in precedence order: the filename supplies
    /// it when none is passed, an explicit one rescues a file that names none,
    /// and an explicit one BEATS a filename that disagrees.
    ///
    /// Named for the property rather than the first case, because a nextest
    /// summary shows the name and nothing else — and two of the three cases
    /// are the opposite of "the filename supplies it".
    #[tokio::test]
    async fn the_period_precedence() {
        let out = validate_ubereats_report(&report("SF"), "2026.11 UberEats SF.csv", None, None)
            .await
            .expect("validates");
        assert_eq!((out.report_year, out.report_month), (2026, 11));

        // An explicit period wins, and rescues a file with no period in its name.
        let out = validate_ubereats_report(&report("SF"), "payments.csv", Some((2025, 3)), None)
            .await
            .expect("validates");
        assert_eq!((out.report_year, out.report_month), (2025, 3));

        // …and wins over a CONFLICTING one in the name. This is not just about
        // which error message appears: the upload path builds the object key
        // from the value read back here, so if the filename outranked the
        // caller the report would land under a period nobody named — the
        // silent-wrong-shape class this whole source exists to avoid.
        //
        // `1900.05` deliberately: if precedence ever inverts, the caller's
        // range check catches it as a refusal rather than a wrong-but-plausible
        // key.
        let out = validate_ubereats_report(&report("SF"), "1900.05 SF.csv", Some((2026, 8)), None)
            .await
            .expect("validates");
        assert_eq!(
            (out.report_year, out.report_month),
            (2026, 8),
            "a period in the file name must not outrank one the caller supplied"
        );
    }

    /// The whole point: a renamed JE-critical column is caught HERE, with
    /// airway's own wording, rather than hours later in a pipeline run.
    #[tokio::test]
    async fn a_missing_je_column_is_rejected_with_airways_own_message() {
        let csv = b"Store Name,Total payout\nPoke House SF,10\n";
        let err = validate_ubereats_report(csv, "2026.08 SF.csv", None, None)
            .await
            .expect_err("a header variant must be refused");

        assert!(matches!(err, ReportValidationError::Rejected(_)));
        let msg = err.to_string();
        assert!(msg.contains("Marketplace Fee"), "names the column: {msg}");
        assert!(msg.contains("JE-critical"), "airway's wording: {msg}");
    }

    /// A file with no period anywhere is refused rather than guessed at — a
    /// wrong period aggregates a month of payouts into the wrong JE.
    #[tokio::test]
    async fn a_report_with_no_period_anywhere_is_rejected() {
        let err = validate_ubereats_report(&report("SF"), "payments.csv", None, None)
            .await
            .expect_err("no period must be refused");
        assert!(matches!(err, ReportValidationError::Rejected(_)));
    }

    /// `.xlsx` is out of scope, and reading one as CSV would fail as a
    /// confusing missing-column error rather than "unsupported".
    #[tokio::test]
    async fn a_non_csv_is_rejected() {
        let err = validate_ubereats_report(b"PK\x03\x04", "2026.08 SF.xlsx", None, None)
            .await
            .expect_err("a workbook must be refused");
        let msg = err.to_string();
        assert!(msg.contains(".csv") || msg.contains("csv"), "got: {msg}");
    }
}
