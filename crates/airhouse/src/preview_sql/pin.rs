//! Pinning live reads to a DuckLake point in time ([`SnapshotPin`]).

use sqlparser::ast::{
    DataType, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArgOperator,
    FunctionArgumentList, FunctionArguments, Ident, ObjectName, TableVersion, TimezoneInfo,
    TypedString, Value, ValueWithSpan,
};
use sqlparser::tokenizer::Span;

use super::Refused;

const MICROS_PER_SECOND: i64 = 1_000_000;
const SECONDS_PER_DAY: i64 = 86_400;
/// `0001-01-01T00:00:00Z` and `9999-12-31T23:59:59.999999Z`, in Unix
/// microseconds: the years a four-digit timestamp literal can hold.
const MIN_MICROS: i64 = -62_135_596_800 * MICROS_PER_SECOND;
const MAX_MICROS: i64 = 253_402_300_800 * MICROS_PER_SECOND - 1;

/// A DuckLake point in time that every live read is pinned to, so a read sees
/// the tables as they were then and not as production moves on.
///
/// It is a **timestamp, not a snapshot id**: `AT (TIMESTAMP => ts)` resolves
/// to the latest snapshot at or before `ts`, and that never changes once `ts`
/// has passed (until the snapshot is expired). It reads what the recorded id
/// would, and needs no catalog name and no `snapshots()` call to build.
///
/// The literal is `TIMESTAMP WITH TIME ZONE '…+00'`, the form Airhouse's own
/// expiry uses: the server is ICU-less, so a zone name or a bare `TIMESTAMP`
/// (read in the session's zone) is not safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotPin {
    micros: i64,
}

impl SnapshotPin {
    /// The instant `micros` microseconds after the Unix epoch (UTC). Refused
    /// outside years 0001 to 9999, which no timestamp literal holds.
    pub fn from_unix_micros(micros: i64) -> Result<Self, Refused> {
        if !(MIN_MICROS..=MAX_MICROS).contains(&micros) {
            return Err(Refused(format!(
                "{micros} microseconds is outside the years a snapshot pin can name"
            )));
        }
        Ok(Self { micros })
    }

    pub fn unix_micros(&self) -> i64 {
        self.micros
    }

    /// `YYYY-MM-DD HH:MM:SS.ffffff+00`: the pinned instant in UTC.
    pub fn literal(&self) -> String {
        let seconds = self.micros.div_euclid(MICROS_PER_SECOND);
        let fraction = self.micros.rem_euclid(MICROS_PER_SECOND);
        let (year, month, day) = civil_from_days(seconds.div_euclid(SECONDS_PER_DAY));
        let in_day = seconds.rem_euclid(SECONDS_PER_DAY);
        format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{fraction:06}+00",
            in_day / 3600,
            in_day % 3600 / 60,
            in_day % 60
        )
    }

    /// `AT(TIMESTAMP => TIMESTAMP WITH TIME ZONE '<literal>')`, the version
    /// clause a pinned table read carries. Built, not parsed: the DuckDB
    /// dialect has no table versioning, so this tree can only be rendered.
    pub(super) fn table_version(&self) -> TableVersion {
        let instant = Expr::TypedString(TypedString {
            data_type: DataType::Timestamp(None, TimezoneInfo::WithTimeZone),
            value: ValueWithSpan {
                value: Value::SingleQuotedString(self.literal()),
                span: Span::empty(),
            },
            uses_odbc_syntax: false,
        });
        TableVersion::Function(Expr::Function(Function {
            name: ObjectName::from(vec![Ident::new("AT")]),
            uses_odbc_syntax: false,
            parameters: FunctionArguments::None,
            args: FunctionArguments::List(FunctionArgumentList {
                duplicate_treatment: None,
                args: vec![FunctionArg::Named {
                    name: Ident::new("TIMESTAMP"),
                    arg: FunctionArgExpr::Expr(instant),
                    operator: FunctionArgOperator::RightArrow,
                }],
                clauses: Vec::new(),
            }),
            filter: None,
            null_treatment: None,
            over: None,
            within_group: Vec::new(),
        }))
    }
}

/// The proleptic Gregorian `(year, month, day)` of `days` since 1970-01-01
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: i64) -> String {
        SnapshotPin::from_unix_micros(micros).unwrap().literal()
    }

    #[test]
    fn a_pin_is_a_utc_literal_to_the_microsecond() {
        // 2026-11-01T09:00:00Z
        assert_eq!(
            at(1_793_523_600 * MICROS_PER_SECOND),
            "2026-11-01 09:00:00.000000+00"
        );
        assert_eq!(at(0), "1970-01-01 00:00:00.000000+00");
        assert_eq!(at(1), "1970-01-01 00:00:00.000001+00");
        assert_eq!(at(-1), "1969-12-31 23:59:59.999999+00");
        // A leap day and the turn of a century that is not a leap year.
        assert_eq!(
            at(951_782_400 * MICROS_PER_SECOND),
            "2000-02-29 00:00:00.000000+00"
        );
        assert_eq!(
            at(4_107_542_400 * MICROS_PER_SECOND),
            "2100-03-01 00:00:00.000000+00"
        );
        assert_eq!(at(MIN_MICROS), "0001-01-01 00:00:00.000000+00");
        assert_eq!(at(MAX_MICROS), "9999-12-31 23:59:59.999999+00");
    }

    #[test]
    fn an_instant_outside_four_digit_years_is_refused() {
        assert!(SnapshotPin::from_unix_micros(MIN_MICROS - 1).is_err());
        assert!(SnapshotPin::from_unix_micros(MAX_MICROS + 1).is_err());
        assert!(SnapshotPin::from_unix_micros(i64::MIN).is_err());
    }
}
