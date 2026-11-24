//! The two streams of the inbox — **Activity** and **Operations** — derived
//! from each kind's [`Audience`] and stored nowhere.
//!
//! | Stream | Holds |
//! | --- | --- |
//! | Activity | every app notice, plus Oxygen's `Everyone` and `Addressed` kinds |
//! | Operations | Oxygen's `Operators`, `Officers` and `Owners` kinds |
//!
//! Activity is what happened *to you*: your apps' notices, the answer you asked
//! for, an anomaly a monitor named you in, your own device-code approval.
//! Operations is what happened *to the workspace*: a failed run, an unpaid
//! invoice. The split means one hourly pipeline failing does not inflate the
//! badge a crew member watches.
//!
//! Nothing is stored, so there is nothing to backfill and nothing a previous
//! binary would not understand: a stream is a condition over `app_id` and
//! `kind`, computed from the catalogue at request time. Adding a kind to
//! `catalog::platform::PLATFORM` therefore puts it in the right stream with no
//! second edit.

use crate::catalog::platform::oxygen;
use crate::catalog::source::{Audience, Source};

/// One of the inbox's two streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Activity,
    Operations,
}

impl Stream {
    /// The stream a kind of this `source` and `audience` is in.
    ///
    /// Every app notice is Activity: an app's notices are addressed to the
    /// people who use it, and an app has no operator tier of its own.
    pub fn of(source: Source, audience: Audience) -> Self {
        match (source, audience) {
            (Source::App, _) => Stream::Activity,
            (Source::Oxygen, Audience::Everyone | Audience::Addressed) => Stream::Activity,
            (Source::Oxygen, Audience::Operators | Audience::Officers | Audience::Owners) => {
                Stream::Operations
            }
        }
    }

    /// The `stream=` query parameter. `None` for anything else, which every
    /// route turns into `400` — `read-all` takes the same filter, so a typo
    /// must not clear both streams.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "activity" => Some(Stream::Activity),
            "operations" => Some(Stream::Operations),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Stream::Activity => "activity",
            Stream::Operations => "operations",
        }
    }
}

/// Oxygen's kinds in the Operations stream, read from the catalogue so the
/// table is the only place a kind's stream is decided.
///
/// The complement is not listed: Activity is "an app, or not one of these",
/// which also carries every kind this binary has never heard of into Activity
/// rather than hiding it in a tab nobody watches.
pub fn operations_kinds() -> Vec<String> {
    let catalogue = oxygen();
    catalogue
        .catalogue
        .kinds
        .keys()
        .filter(|kind| {
            Stream::of(Source::Oxygen, catalogue.audience_of(kind)) == Stream::Operations
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::kinds::{BILLING_PAST_DUE, KINDS, RUN_FAILED};

    #[test]
    fn operations_is_the_two_standing_based_kinds_and_activity_is_everything_else() {
        let mut operations = operations_kinds();
        operations.sort();
        assert_eq!(operations, vec![BILLING_PAST_DUE, RUN_FAILED]);
    }

    #[test]
    fn every_platform_kind_is_in_exactly_one_stream() {
        let catalogue = oxygen();
        let operations = operations_kinds();
        for kind in KINDS {
            let stream = Stream::of(Source::Oxygen, catalogue.audience_of(kind));
            assert_eq!(
                stream == Stream::Operations,
                operations.contains(&kind.to_string()),
                "{kind} must agree with the list the filter is built from"
            );
        }
    }

    #[test]
    fn an_app_notice_and_an_unknown_kind_are_activity() {
        // An app has no operator tier, and a kind from a newer binary defaults
        // to `Everyone` — both land where someone will see them.
        assert_eq!(
            Stream::of(Source::App, Audience::Operators),
            Stream::Activity
        );
        assert_eq!(
            Stream::of(Source::Oxygen, oxygen().audience_of("oxy.something_new")),
            Stream::Activity
        );
    }

    #[test]
    fn only_the_two_names_parse() {
        assert_eq!(Stream::parse("activity"), Some(Stream::Activity));
        assert_eq!(Stream::parse("operations"), Some(Stream::Operations));
        for bad in ["", "Activity", "ops", "all", "operation"] {
            assert_eq!(Stream::parse(bad), None, "{bad}");
        }
        assert_eq!(
            Stream::parse(Stream::Operations.as_str()),
            Some(Stream::Operations)
        );
    }
}
