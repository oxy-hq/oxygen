//! `OXY_RUN_RETENTION_DAYS`: how long a finished run stays in `agentic_runs`.
//!
//! Unset is [`DEFAULT_RUN_RETENTION_DAYS`]. Any number that is zero (`0`,
//! `00`, `+0`), `off` or `never` keeps every run. Anything else that is not a
//! whole number of days up to [`MAX_RUN_RETENTION_DAYS`] refuses a strict boot;
//! on the lenient path it keeps every run too, because the prune deletes and a
//! typo must not decide what goes. The cap is what makes "a huge number to
//! mean forever" safe: past it the value is refused, never wrapped or
//! overflowed into a window that deletes everything.

use super::ConfigError;

/// Days a finished run is kept when the deployment says nothing.
pub const DEFAULT_RUN_RETENTION_DAYS: u32 = 90;
/// The longest window accepted: a hundred years. Use `off` for forever.
pub const MAX_RUN_RETENTION_DAYS: u32 = 36_500;

const VAR: &str = "OXY_RUN_RETENTION_DAYS";

/// What `OXY_RUN_RETENTION_DAYS` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunRetentionSetting {
    /// Not set: [`DEFAULT_RUN_RETENTION_DAYS`].
    Unset,
    /// A number of days, 1 to [`MAX_RUN_RETENTION_DAYS`].
    Days(u32),
    /// Zero, `off` or `never`: keep every run.
    Off,
    /// Not a day count. Kept verbatim so a strict boot can name it.
    Unrecognised(String),
}

impl RunRetentionSetting {
    pub fn parse(raw: Option<String>) -> Self {
        let Some(raw) = raw else {
            return RunRetentionSetting::Unset;
        };
        let value = raw.trim().to_ascii_lowercase();
        if value == "off" || value == "never" {
            return RunRetentionSetting::Off;
        }
        // On the parsed number, not the spelling: `00` and `+0` are zero too.
        match value.parse::<u32>() {
            Ok(0) => RunRetentionSetting::Off,
            Ok(days) if days <= MAX_RUN_RETENTION_DAYS => RunRetentionSetting::Days(days),
            _ => RunRetentionSetting::Unrecognised(raw),
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        match self {
            RunRetentionSetting::Unrecognised(value) => Err(ConfigError {
                var: VAR,
                value: value.clone(),
                problem: format!(
                    "expected a number of days up to {MAX_RUN_RETENTION_DAYS}, \
                     or 0 / off / never to keep every run"
                ),
            }),
            _ => Ok(()),
        }
    }

    /// Days a finished run is kept, or `None` to keep every run.
    pub fn days(&self) -> Option<u32> {
        match self {
            RunRetentionSetting::Unset => Some(DEFAULT_RUN_RETENTION_DAYS),
            RunRetentionSetting::Days(days) => Some(*days),
            RunRetentionSetting::Off => None,
            RunRetentionSetting::Unrecognised(value) => {
                tracing::warn!(
                    value = value.as_str(),
                    "{VAR}: not a number of days; keeping every run"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> RunRetentionSetting {
        RunRetentionSetting::parse(Some(raw.to_string()))
    }

    #[test]
    fn unset_keeps_ninety_days() {
        assert_eq!(RunRetentionSetting::parse(None).days(), Some(90));
    }

    #[test]
    fn a_day_count_is_the_window() {
        assert_eq!(parse("30").days(), Some(30));
        assert_eq!(parse(" 365 ").days(), Some(365));
        assert_eq!(parse("36500").days(), Some(MAX_RUN_RETENTION_DAYS));
        assert_eq!(parse("30").validate(), Ok(()));
    }

    #[test]
    fn zero_off_and_never_keep_every_run() {
        for off in ["0", "00", "+0", "off", "never", " OFF "] {
            assert_eq!(parse(off), RunRetentionSetting::Off, "{off}");
            assert_eq!(parse(off).days(), None, "{off}");
        }
    }

    #[test]
    fn anything_else_refuses_the_boot_and_otherwise_keeps_every_run() {
        for bad in [
            "90d",
            "-1",
            "ninety",
            "",
            "36501",
            "4294967295",
            "99999999999",
        ] {
            let setting = parse(bad);
            assert_eq!(setting.validate().unwrap_err().var, VAR, "{bad}");
            assert_eq!(setting.days(), None, "{bad}");
        }
    }
}
