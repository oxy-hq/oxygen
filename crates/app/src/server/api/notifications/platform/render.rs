//! A platform notice in words, built from Oxy's own records when it is read
//! or delivered.
//!
//! The signal holds the kind and the subject; this looks the subject up and
//! says what happened. Nothing is copied at send time, so the words follow the
//! record (a renamed workspace reads with its new name) and Oxy's database
//! never holds notification text. A subject that is gone renders the kind's
//! plain sentence with no link rather than nothing.
//!
//! Batched: a page of the inbox reads each subject kind with one query.

use std::collections::{HashMap, HashSet};

use entity::{notifications, organizations};
use oxy_app_core::custom_apps_host_dispatch::admin_base_url;
use oxy_oltp::inbox::InboxMessage;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::kinds::{
    self, ANOMALY, ASK_FAILED, ASK_FINISHED, ASK_WAITING, BILLING_PAST_DUE, CLI_APPROVAL,
    RUN_FAILED, SUBJECT_AIRWAY_RUN, SUBJECT_ANALYTICS_RUN, SUBJECT_ANALYTICS_SUSPENSION,
    SUBJECT_ANOMALY, SUBJECT_AUTOMATION_RUN, SUBJECT_BILLING, SUBJECT_CLI_DEVICE_REQUEST,
    SUBJECT_TEST, SUBJECT_WORKSPACE, TEST, WORKSPACE_READY,
};
use super::subjects;

/// What a platform notice says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub title: String,
    pub body: Option<String>,
    /// A path in Oxygen's web app, relative to whatever host the reader is on.
    pub href: Option<String>,
}

/// What a signal is about: enough to render it, and nothing it says.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct About {
    pub org_id: Uuid,
    pub kind: String,
    pub subject_kind: &'static str,
    pub subject_id: String,
}

impl About {
    /// `Some` for a platform notice: no app, a platform kind, a subject.
    pub fn of(n: &notifications::Model) -> Option<Self> {
        if n.app_id.is_some() || !kinds::is_platform_kind(&n.kind) {
            return None;
        }
        Some(Self {
            org_id: n.org_id,
            kind: n.kind.clone(),
            subject_kind: kinds::subject_kind(n.subject_kind.as_deref()?)?,
            subject_id: n.subject_id.clone()?,
        })
    }
}

/// Whether `n` is a platform notice, rendered here rather than read from the
/// org's store. By the reserved prefix, not the closed list, so a kind from a
/// newer binary still reads as Oxygen's plain fallback instead of being looked
/// up in an OLTP store that never held it.
pub fn is_platform(n: &notifications::Model) -> bool {
    n.app_id.is_none() && crate::server::api::notifications::input::is_reserved_kind(&n.kind)
}

/// The words for each of `abouts`.
pub async fn render_all(
    db: &DatabaseConnection,
    abouts: &HashSet<About>,
) -> Result<HashMap<About, Rendered>, DbErr> {
    let orgs = org_names(db, abouts).await?;
    let records = subjects::load(db, abouts).await?;
    Ok(abouts
        .iter()
        .map(|a| {
            let org = orgs.get(&a.org_id);
            let rendered = records
                .get(&(a.subject_kind, a.subject_id.clone()))
                .and_then(|r| words(a, org, r))
                .unwrap_or_else(|| fallback(&a.kind));
            (a.clone(), rendered)
        })
        .collect())
}

/// What a channel sends for a platform send: the words for its subject, with
/// the email composed around them. `signal` is any of the send's signals;
/// they share the kind and the subject.
pub async fn message(
    db: &DatabaseConnection,
    signal: &notifications::Model,
) -> Result<InboxMessage, DbErr> {
    let about = About::of(signal);
    let words = match &about {
        Some(a) => render_all(db, &HashSet::from([a.clone()]))
            .await?
            .remove(a)
            .unwrap_or_else(|| fallback(&signal.kind)),
        None => fallback(&signal.kind),
    };
    let url = words
        .href
        .as_deref()
        .and_then(|href| Some(format!("{}{href}", admin_base_url()?)));
    let (html, text) = crate::emails::notification::render(
        "Oxygen",
        &words.title,
        words.body.as_deref(),
        url.as_deref(),
    );
    Ok(InboxMessage {
        id: signal.message_id.unwrap_or(signal.id),
        app_id: None,
        kind: signal.kind.clone(),
        email_subject: Some(words.title.clone()),
        email_html: Some(html),
        email_text: Some(text),
        title: words.title,
        body: words.body,
        link: words.href,
    })
}

/// The sentence a notice reads as when its record cannot be found.
pub fn fallback(kind: &str) -> Rendered {
    let title = match kind {
        BILLING_PAST_DUE => "Your subscription payment is past due",
        WORKSPACE_READY => "A workspace is ready",
        RUN_FAILED => "A run failed",
        ANOMALY => "A monitor found an anomaly",
        CLI_APPROVAL => "oxyc asks for your approval",
        TEST => super::self_test::TITLE,
        ASK_FINISHED | ASK_FAILED => super::ask_finished::title(kind),
        ASK_WAITING => super::ask_waiting::TITLE,
        _ => "Oxygen notice",
    };
    Rendered {
        title: title.to_string(),
        body: None,
        href: None,
    }
}

/// The org's name and slug.
#[derive(Debug, Clone)]
pub struct OrgNames {
    pub name: String,
    pub slug: String,
}

async fn org_names(
    db: &DatabaseConnection,
    abouts: &HashSet<About>,
) -> Result<HashMap<Uuid, OrgNames>, DbErr> {
    let ids: HashSet<Uuid> = abouts.iter().map(|a| a.org_id).collect();
    Ok(organizations::Entity::find()
        .filter(organizations::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|o| {
            (
                o.id,
                OrgNames {
                    name: o.name,
                    slug: o.slug,
                },
            )
        })
        .collect())
}

/// The words for one notice from its loaded record. `None` when the record
/// does not fit the kind (a newer binary's row), which falls back.
fn words(about: &About, org: Option<&OrgNames>, record: &subjects::Record) -> Option<Rendered> {
    use subjects::Record;
    let slug = org.map(|o| o.slug.as_str());
    match (about.subject_kind, record) {
        (SUBJECT_BILLING, Record::Billing { grace_ends_at }) => Some(Rendered {
            title: format!(
                "Payment past due for {}",
                org.map(|o| o.name.as_str()).unwrap_or("your organization")
            ),
            body: Some(format!(
                "Update the payment method by {} to keep editing. Reading keeps working.",
                grace_ends_at.format("%B %-d, %Y")
            )),
            href: slug.map(|s| format!("/{s}?settings=organization.billing")),
        }),
        (SUBJECT_WORKSPACE, Record::Workspace { id, name }) => Some(Rendered {
            title: format!("{name} is ready"),
            body: Some("It finished setting up and can be opened.".to_string()),
            href: slug.map(|s| format!("/{s}/workspaces/{id}")),
        }),
        (SUBJECT_AUTOMATION_RUN | SUBJECT_AIRWAY_RUN, Record::Run(run)) => {
            Some(run_words(run, slug))
        }
        (SUBJECT_ANOMALY, Record::Anomaly(a)) => Some(Rendered {
            title: format!("{} looks unusual", a.name),
            body: Some(a.summary()),
            href: slug.map(|s| {
                format!(
                    "/{s}/workspaces/{}/ide/semantic?view=anomalies",
                    a.workspace_id
                )
            }),
        }),
        (SUBJECT_CLI_DEVICE_REQUEST, record @ Record::CliDeviceRequest { .. }) => {
            super::cli_approval::words(record, chrono::Utc::now())
        }
        (SUBJECT_TEST, Record::Test(t)) => Some(t.words(slug)),
        (SUBJECT_ANALYTICS_RUN, Record::Ask(a)) => {
            Some(super::ask_finished::words(a, &about.kind, slug))
        }
        (SUBJECT_ANALYTICS_SUSPENSION, Record::AskWaiting(w)) => {
            Some(super::ask_waiting::words(w, slug))
        }
        _ => None,
    }
}

/// A failed run: the failure itself, or — when its source had already failed
/// since it last succeeded — how long it has been failing. Only the first
/// failure of a streak and a daily reminder are sent (`run_failed.rs`), so
/// the second shape is the reminder.
fn run_words(run: &subjects::RunRecord, slug: Option<&str>) -> Rendered {
    let href = slug.map(|s| run.href(s));
    if let Some(streak) = run.streak.filter(|s| s.failures > 1) {
        return Rendered {
            title: format!("{} is still failing", run.name),
            body: Some(format!(
                "It has failed {} times since {}. Open the latest run to see why.",
                streak.failures,
                streak.since.format("%B %-d at %H:%M UTC")
            )),
            href,
        };
    }
    Rendered {
        title: format!("{} failed", run.name),
        body: Some(format!(
            "The {}run that started {} failed and will not be retried. Open it to see why.",
            if run.scheduled { "scheduled " } else { "" },
            run.started_at.format("%B %-d at %H:%M UTC")
        )),
        href,
    }
}

#[cfg(test)]
mod tests {
    use super::super::run_streak::Streak;
    use super::*;
    use chrono::{TimeZone, Utc};

    fn about(kind: &str, subject_kind: &'static str) -> About {
        About {
            org_id: Uuid::nil(),
            kind: kind.to_string(),
            subject_kind,
            subject_id: "x".into(),
        }
    }

    fn acme() -> OrgNames {
        OrgNames {
            name: "Acme".into(),
            slug: "acme".into(),
        }
    }

    #[test]
    fn billing_links_to_the_billing_settings_and_names_the_deadline() {
        let r = words(
            &about(BILLING_PAST_DUE, SUBJECT_BILLING),
            Some(&acme()),
            &subjects::Record::Billing {
                grace_ends_at: Utc.with_ymd_and_hms(2026, 10, 21, 9, 0, 0).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(r.title, "Payment past due for Acme");
        assert!(r.body.unwrap().contains("October 21, 2026"));
        assert_eq!(
            r.href.as_deref(),
            Some("/acme?settings=organization.billing")
        );
    }

    #[test]
    fn a_ready_workspace_opens_itself() {
        let id = Uuid::new_v4();
        let r = words(
            &about(WORKSPACE_READY, SUBJECT_WORKSPACE),
            Some(&acme()),
            &subjects::Record::Workspace {
                id,
                name: "Sales".into(),
            },
        )
        .unwrap();
        assert_eq!(r.title, "Sales is ready");
        assert_eq!(r.href, Some(format!("/acme/workspaces/{id}")));
    }

    fn failed_run(streak: Option<Streak>) -> subjects::Record {
        subjects::Record::Run(subjects::RunRecord {
            id: "r-9".into(),
            workspace_id: Uuid::nil(),
            name: "restaurant_analytics".into(),
            file: Some("restaurant_analytics.airway.yml".into()),
            airway: true,
            scheduled: false,
            started_at: Utc.with_ymd_and_hms(2026, 10, 19, 14, 59, 0).unwrap(),
            streak,
        })
    }

    #[test]
    fn a_run_that_kept_failing_says_how_long_and_one_that_did_not_says_it_failed() {
        let about = about(RUN_FAILED, SUBJECT_AIRWAY_RUN);
        let reminder = words(
            &about,
            Some(&acme()),
            &failed_run(Some(Streak {
                failures: 9,
                since: Utc.with_ymd_and_hms(2026, 10, 19, 6, 59, 0).unwrap(),
            })),
        )
        .unwrap();
        assert_eq!(reminder.title, "restaurant_analytics is still failing");
        assert_eq!(
            reminder.body.as_deref(),
            Some(
                "It has failed 9 times since October 19 at 06:59 UTC. Open the latest run to see why."
            )
        );
        assert!(
            reminder.href.unwrap().ends_with("/runs/r-9"),
            "the latest run"
        );

        for first in [None, Some(1)] {
            let streak = first.map(|failures| Streak {
                failures,
                since: Utc.with_ymd_and_hms(2026, 10, 19, 14, 59, 0).unwrap(),
            });
            let r = words(&about, Some(&acme()), &failed_run(streak)).unwrap();
            assert_eq!(r.title, "restaurant_analytics failed", "{first:?}");
            assert_eq!(
                r.body.as_deref(),
                Some(
                    "The run that started October 19 at 14:59 UTC failed and will not be retried. Open it to see why."
                )
            );
        }
    }

    fn cli_request(status: &str, expires_in: chrono::Duration) -> subjects::Record {
        subjects::Record::CliDeviceRequest {
            user_code: "BCDFGHJK".into(),
            hostname: "build-box".into(),
            kind: "sandbox_agent",
            apps: vec!["acme/store".into()],
            hours: Some(2),
            status: status.into(),
            expires_at: Utc::now() + expires_in,
        }
    }

    #[test]
    fn an_oxyc_approval_names_the_host_and_links_to_the_page_without_the_code() {
        let about = about(CLI_APPROVAL, SUBJECT_CLI_DEVICE_REQUEST);
        let r = words(
            &about,
            Some(&acme()),
            &cli_request("pending", chrono::Duration::minutes(5)),
        )
        .unwrap();
        assert_eq!(
            r.title,
            "oxyc on build-box asks for a sandbox agent token for acme/store (2 h)"
        );
        assert!(
            r.body
                .unwrap()
                .starts_with("Open the approval page and type the code")
        );
        assert_eq!(r.href.as_deref(), Some("/cli-auth/device"));
    }

    #[test]
    fn an_oxyc_approval_that_was_decided_or_lapsed_says_so_under_the_same_title() {
        let about = about(CLI_APPROVAL, SUBJECT_CLI_DEVICE_REQUEST);
        let say = |status, left| words(&about, Some(&acme()), &cli_request(status, left)).unwrap();
        let waiting = say("pending", chrono::Duration::minutes(5));
        for (status, left, body) in [
            ("approved", chrono::Duration::minutes(5), "Approved."),
            ("denied", chrono::Duration::minutes(5), "Declined."),
            ("pending", chrono::Duration::minutes(-1), "Expired."),
        ] {
            let r = say(status, left);
            assert_eq!(r.title, waiting.title);
            assert_eq!(r.body.as_deref(), Some(body));
        }
        // Swept, or from a newer binary: the plain sentence, no link.
        assert_eq!(fallback(CLI_APPROVAL).title, "oxyc asks for your approval");
        assert_eq!(fallback(CLI_APPROVAL).href, None);
    }

    #[test]
    fn a_record_of_the_wrong_shape_falls_back_without_a_link() {
        let r = words(
            &about(WORKSPACE_READY, SUBJECT_WORKSPACE),
            Some(&acme()),
            &subjects::Record::Billing {
                grace_ends_at: Utc::now(),
            },
        );
        assert_eq!(r, None);
        assert_eq!(fallback(WORKSPACE_READY).href, None);
    }
}
