//! Send the page `failure_alert` decides on: the finalization hook, the
//! message, and the ops Slack channel it goes to — plus, when configured, its
//! All Quiet copy for ferret (`failure_incident`).
//!
//! With no `OXY_OPS_SLACK_BOT_TOKEN` / `OXY_OPS_SLACK_CHANNEL` (the pair
//! Workspace Health pages with) nothing is claimed or sent, All Quiet included;
//! the WARN line from `failure_signal` still goes out.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sentry::SentryFutureExt;
use uuid::Uuid;

use super::failure_alert::{
    FUNCTION_PAGE_WINDOW_HOURS, FailureKey, LOOKBACK_DAYS, PERSISTENT_AFTER_HOURS, THRESHOLD,
    Verdict, claim, mark_delivered,
};
use super::failure_incident;
use super::failure_signal::{Failure, HostCallFailure};

/// The migration that added `failure_fingerprint`. Invocations from before it
/// was applied carry none.
const FINGERPRINT_MIGRATION: &str = "m20260911_000001_function_failure_alerts";

/// How long the Slack post may take before it is abandoned for a later retry.
const POST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The finalization hook: page ops if `failure` is new for this function.
pub(super) async fn observe(
    db: &DatabaseConnection,
    app: &entity::apps::Model,
    environment: &oxy_app_core::custom_app_environment::AppEnvironment,
    function_name: &str,
    invocation_id: Uuid,
    failure: &Failure,
) {
    // Only production pages, and only production claims the page slot — see
    // `failure_alert`'s "Production only, guarded twice": this is the first
    // guard, and `claim` reads the invocation's own row for the second. A
    // failing staging function is visible in its invocation rows and logs,
    // and pages nobody.
    if *environment != oxy_app_core::custom_app_environment::AppEnvironment::Production {
        return;
    }
    let Some((token, channel)) = ops_slack_target() else {
        return;
    };
    let key = FailureKey {
        app_id: app.id,
        function_name,
        fingerprint: &failure.fingerprint,
        host_call: failure.host_call.is_some(),
    };
    let (first_seen, persistent) = match claim(
        db,
        invocation_id,
        key,
        Utc::now(),
        history_since(db).await,
    )
    .await
    {
        Ok(Verdict::Page {
            first_seen,
            persistent,
        }) => (first_seen, persistent),
        Ok(Verdict::Quiet) => return,
        Ok(Verdict::Suppressed(reason)) => {
            tracing::info!(target: "oxy::app_function", reason, "failure alert: held back");
            return;
        }
        Err(e) => {
            tracing::warn!(target: "oxy::app_function", error = %e, "failure alert: claim failed");
            return;
        }
    };
    let org_slug = org_slug(db, app.org_id).await;
    let text = message(&PageFacts {
        app,
        org_slug: org_slug.as_deref(),
        console_base: oxy_app_core::custom_apps_host_dispatch::admin_base_url(),
        key,
        invocation_id,
        failure,
        first_seen,
        persistent,
    });
    let incident = all_quiet_copy(
        app,
        org_slug.as_deref(),
        key,
        invocation_id,
        failure,
        first_seen,
    );
    let (db, app_id, host_call) = (db.clone(), app.id, key.host_call);
    let (function_name, fingerprint) = (function_name.to_string(), failure.fingerprint.clone());
    // Spawned, not a TaskSpec (a deliberate departure from
    // `oxy-task-spec-default`): one capped post whose delivery the claim row
    // already tracks. A replica that dies here leaves the claim unsent; it goes
    // stale and the next failure sends it.
    tokio::spawn(
        async move {
            // The All Quiet copy rides along and does not decide delivery: a
            // retry re-sends it, and All Quiet folds that into one incident.
            if let Some((url, body)) = incident
                && let Err(error) = failure_incident::post(&url, &body, POST_TIMEOUT).await
            {
                tracing::warn!(target: "oxy::app_function", error, "failure alert: All Quiet post failed");
            }
            let client = oxy_slack_client::SlackClient::new();
            let post = client.chat_post_message(&token, &channel, &text, None);
            let error = match tokio::time::timeout(POST_TIMEOUT, post).await {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(_) => Some(format!("timed out after {POST_TIMEOUT:?}")),
            };
            if let Some(error) = error {
                tracing::warn!(
                    target: "oxy::app_function",
                    error,
                    "failure alert: Slack post failed; the next failure retries once the claim is stale"
                );
                return;
            }
            let key = FailureKey {
                app_id,
                function_name: &function_name,
                fingerprint: &fingerprint,
                host_call,
            };
            if let Err(e) = mark_delivered(&db, key, Utc::now()).await {
                tracing::warn!(target: "oxy::app_function", error = %e, "failure alert: mark delivered failed");
            }
        }
        // `text` is the tenant function's own error message. This task outlives
        // the invocation, so it carries that hub itself — barrier 1 covers this
        // module's `oxy::app_function` lines, not a panic or an ERROR raised
        // inside the Slack client.
        .bind_hub(sentry::Hub::current()),
    );
}

/// The All Quiet webhook URL and body for this page, when one is configured.
fn all_quiet_copy(
    app: &entity::apps::Model,
    org_slug: Option<&str>,
    key: FailureKey<'_>,
    invocation_id: Uuid,
    failure: &Failure,
    first_seen: DateTime<Utc>,
) -> Option<(String, serde_json::Value)> {
    let url = failure_incident::webhook_url()?;
    let body = failure_incident::payload(&failure_incident::Incident {
        org_id: app.org_id,
        org_slug,
        app_id: app.id,
        app_slug: &app.slug,
        function: key.function_name,
        kind: failure.kind,
        fingerprint: key.fingerprint,
        invocation_id,
        first_seen,
    });
    Some((url, body))
}

/// When invocations started carrying a fingerprint: the migration's
/// `applied_at`, read once per process. Unknown reads as now — the whole
/// lookback treated as unfingerprinted, quiet rather than noisy — and is not
/// cached, so a transient read error does not stick.
async fn history_since(db: &DatabaseConnection) -> DateTime<Utc> {
    static SINCE: tokio::sync::OnceCell<DateTime<Utc>> = tokio::sync::OnceCell::const_new();
    if let Some(since) = SINCE.get() {
        return *since;
    }
    let applied = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT applied_at FROM seaql_migrations WHERE version = $1",
            [FINGERPRINT_MIGRATION.into()],
        ))
        .await
        .ok()
        .flatten()
        .and_then(|row| row.try_get::<i64>("", "applied_at").ok())
        .and_then(|secs| DateTime::from_timestamp(secs, 0));
    match applied {
        Some(since) => *SINCE.get_or_init(|| async { since }).await,
        None => Utc::now(),
    }
}

/// The org's slug, read once per page: the Slack page names the app the way
/// the console and the route do, and the All Quiet incident names the customer
/// so ferret needs no lookup. `None` when the read fails: the page still goes,
/// with the id, and the incident sends null.
async fn org_slug(db: &DatabaseConnection, org_id: Uuid) -> Option<String> {
    use sea_orm::EntityTrait;
    match entity::organizations::Entity::find_by_id(org_id)
        .one(db)
        .await
    {
        Ok(org) => org.map(|o| o.slug),
        Err(e) => {
            tracing::warn!(target: "oxy::app_function", error = %e, "failure alert: org lookup failed");
            None
        }
    }
}

/// What one page says, gathered so [`message`] reads as the page does.
struct PageFacts<'a> {
    app: &'a entity::apps::Model,
    org_slug: Option<&'a str>,
    /// Where the admin console is served (`OXY_API_URL`'s origin); `None` on a
    /// deployment that cannot name its own URL.
    console_base: Option<String>,
    key: FailureKey<'a>,
    invocation_id: Uuid,
    failure: &'a Failure,
    first_seen: DateTime<Utc>,
    persistent: bool,
}

/// The page. Names the function and the shape of the failure, never its
/// message: that stays in `app_function_invocations.error`, where reading it
/// takes the same assume-role session as any other tenant data — or, for a
/// `host_call` failure, went to the app's catch block and is nowhere the
/// platform holds, so the page names the op and kind instead.
///
/// The app is named by its slugs, linked to its admin console where the
/// deployment can say its own URL. It used to be two UUIDs, and on 2026-10-21
/// the first reader of a page took Poke House Staging's for Poke House's.
fn message(page: &PageFacts<'_>) -> String {
    let first = page.first_seen.format("%Y-%m-%d %H:%M");
    // The persistent route pages below the threshold, so the fast route's
    // "3+ failed invocations" would be false there. Say what is true: it is
    // new, it has not stopped, and nothing succeeded in between.
    let how = if page.persistent {
        format!(
            "every call since {first} UTC has failed ({PERSISTENT_AFTER_HOURS}h+, no success \
             in between) — low traffic, so it never reached {THRESHOLD} calls in a day"
        )
    } else {
        format!("{THRESHOLD}+ failed invocations since {first} UTC")
    };
    let (app, failure) = (page.app, page.failure);
    let cause = failure
        .host_call
        .as_ref()
        .map(|hc| format!("{}\n", cause_line(hc)))
        .unwrap_or_default();
    format!(
        ":rotating_light: Custom-app function `{slug}/{function}`{in_org} is failing in a way it \
         has not in {LOOKBACK_DAYS} days: {how}.\n\
         {cause}\
         • kind `{kind}` · fingerprint `{fingerprint}`\n\
         • app {app_ref} · latest invocation `{invocation_id}`\n\
         {where_to_look}",
        slug = app.slug,
        function = page.key.function_name,
        in_org = page
            .org_slug
            .map(|org| format!(" in `{org}`"))
            .unwrap_or_default(),
        kind = failure.kind,
        fingerprint = page.key.fingerprint,
        app_ref = app_ref(page),
        invocation_id = page.invocation_id,
        where_to_look = where_to_look(failure),
    )
}

/// The app, as a link to its console opened on the failing function when the
/// deployment knows its own URL, and as `org/app` otherwise. The id stays: it
/// is what `oxyc api` takes.
fn app_ref(page: &PageFacts<'_>) -> String {
    let app = page.app;
    let Some(org) = page.org_slug else {
        return format!("`{}` in org `{}`", app.id, app.org_id);
    };
    let name = format!("{org}/{}", app.slug);
    let shown = match &page.console_base {
        Some(base) => format!(
            "<{base}/admin/apps/{name}?fn={function}|{name}>",
            function = page.key.function_name
        ),
        None => format!("`{name}`"),
    };
    format!("{shown} (`{}`)", app.id)
}

/// What failed, when a `ctx.*` call did: the op, the closed-list kind, and for
/// the kinds that name a refusal, what that means to whoever reads the page
/// first. Nothing of the host's text — the kind is the cause in a word.
fn cause_line(hc: &HostCallFailure) -> String {
    let meaning = match hc.kind {
        "quota_exceeded" => {
            " — the backend refused for a quota or a plan limit; the fix is the limit, not the app"
        }
        "unreachable" => {
            " — nothing answered at the backend's address, or its name did not resolve"
        }
        _ => "",
    };
    format!("• cause: `{}` failed as `{}`{meaning}", hc.op, hc.kind)
}

/// The page's last line: where the failure's message is, and what the same
/// failure elsewhere would mean. A `host_call` row has `status = success` and
/// a NULL `error` — the app caught the message — so pointing at the column
/// would send on-call to an empty cell; the op and kind are what there is.
fn where_to_look(failure: &Failure) -> String {
    match &failure.host_call {
        Some(hc) => {
            let message = if failure.kind == "host_call" {
                format!(
                    "• host call `{}` failed as `{}`; the handler caught it and answered, so \
                     `app_function_invocations.error` is NULL.",
                    hc.op, hc.kind
                )
            } else {
                "• the handler did not catch the failed call; the message is in \
                 `app_function_invocations.error`."
                    .to_string()
            };
            format!(
                "{message} The same op and kind on other apps point at the platform rather than \
                 the app. Another function of this app that fails with this fingerprint in the \
                 next {FUNCTION_PAGE_WINDOW_HOURS}h is held back (`suppressed:cause_rate`) \
                 rather than paged; the app's Issues list shows every one."
            )
        }
        // Every heap kill carries the same message, so every function that is
        // killed has one fingerprint, in every app. The line below would read
        // that as the platform failing; it is each app going over the limit on
        // its own.
        None if failure.kind == "exceeded_memory" => format!(
            "The function went over the per-invocation heap limit{limit}: it was holding more \
             in memory at once than one invocation may. `request_body_bytes` on the \
             invocation's span is the size of the request, which is the first thing to check. \
             Every function that hits the limit has this fingerprint, so the same fingerprint \
             on other functions does not point at the platform.",
            limit = heap_limit(),
        ),
        None => "The message is in `app_function_invocations.error`; the same fingerprint on \
                 other functions points at the platform rather than the app."
            .to_string(),
    }
}

/// The heap limit in force in this process, as ` (128 MiB)` — the process that
/// finalizes an invocation is the one that ran it. Empty when no limit is set
/// or the runtime is not compiled in; neither produces an `exceeded_memory`.
fn heap_limit() -> String {
    #[cfg(feature = "custom-app-functions")]
    {
        if let Some(bytes) = super::runtime::heap_limit_bytes() {
            return format!(" ({} MiB)", bytes / (1024 * 1024));
        }
    }
    String::new()
}

/// The ops Slack bot token and the channel custom-app pages go to.
///
/// `OXY_OPS_SLACK_CUSTOM_APPS_CHANNEL` when it is set, else
/// `OXY_OPS_SLACK_CHANNEL` — the channel Workspace Health pages with. The
/// split exists because of the Sep 2026 warehouse incident: this module paged
/// correctly, eleven hours after the release that broke the app, into a
/// channel where 54 of 57 messages that week were one workspace's health
/// check, red for three weeks over an unrelated connection. A page nobody can
/// find among the noise is a page nobody reads. Unset or empty token turns
/// failure alerts off.
pub(crate) fn ops_slack_target() -> Option<(String, String)> {
    ops_slack_target_from(|name| std::env::var(name).ok())
}

fn ops_slack_target_from(var: impl Fn(&str) -> Option<String>) -> Option<(String, String)> {
    let set = |name: &str| var(name).filter(|v| !v.trim().is_empty());
    let channel =
        set("OXY_OPS_SLACK_CUSTOM_APPS_CHANNEL").or_else(|| set("OXY_OPS_SLACK_CHANNEL"))?;
    Some((set("OXY_OPS_SLACK_BOT_TOKEN")?, channel))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_ops() -> entity::apps::Model {
        entity::apps::Model {
            id: Uuid::from_u128(0xf090),
            slug: "store-ops".into(),
            name: "Store Ops".into(),
            org_id: Uuid::from_u128(0xbb0f),
            project_id: Uuid::nil(),
            branch: "main".into(),
            source_repo: String::new(),
            status: "published".into(),
            source_type: "s3".into(),
            source_config: serde_json::json!({}),
            last_synced_at: None,
            manifest_override: None,
            bootstrap_pr_url: None,
            published_at: None,
            repo_path: None,
            last_promoted_by: None,
            last_promoted_at: None,
            visibility: "org".into(),
            created_at: Utc::now().fixed_offset(),
            updated_at: Utc::now().fixed_offset(),
        }
    }

    fn quota_refusal() -> HostCallFailure {
        HostCallFailure {
            op: "oltp.query",
            kind: "quota_exceeded",
            message: "could not open a transaction: connection error: connect failed: db error: \
                      error: your account or project has exceeded the quota."
                .into(),
            target: None,
        }
    }

    /// The page the 2026-10-21 outage would have sent with this change: the
    /// app by name and link, and the cause in a word, ahead of the fingerprint.
    fn page_text(failure: &Failure, org_slug: Option<&str>, console_base: Option<&str>) -> String {
        let app = store_ops();
        let key = FailureKey {
            app_id: app.id,
            function_name: "list-due",
            fingerprint: &failure.fingerprint,
            host_call: failure.host_call.is_some(),
        };
        message(&PageFacts {
            app: &app,
            org_slug,
            console_base: console_base.map(str::to_string),
            key,
            invocation_id: Uuid::nil(),
            failure,
            first_seen: Utc::now(),
            persistent: false,
        })
    }

    #[test]
    fn a_page_names_the_app_by_its_slugs_and_links_its_console() {
        let threw = Failure::of("error", 0, Some("function threw: Error: x"), None, None).unwrap();
        let text = page_text(
            &threw,
            Some("poke-house-staging"),
            Some("https://app.oxygen-hq.com"),
        );
        assert!(
            text.contains("`store-ops/list-due` in `poke-house-staging`"),
            "{text}"
        );
        assert!(
            text.contains(
                "<https://app.oxygen-hq.com/admin/apps/poke-house-staging/store-ops?fn=list-due\
                 |poke-house-staging/store-ops>"
            ),
            "{text}"
        );
        // The id stays, for `oxyc api`.
        assert!(text.contains(&store_ops().id.to_string()), "{text}");
        // Not a host call: no cause line to print.
        assert!(!text.contains("• cause:"), "{text}");

        // No URL for the deployment: the name, unlinked.
        let unlinked = page_text(&threw, Some("poke-house-staging"), None);
        assert!(
            unlinked.contains("`poke-house-staging/store-ops`"),
            "{unlinked}"
        );
        assert!(!unlinked.contains("<http"), "{unlinked}");

        // The org could not be read: the page still goes, with the ids it had.
        let bare = page_text(&threw, None, Some("https://app.oxygen-hq.com"));
        assert!(
            bare.contains(&format!("in org `{}`", store_ops().org_id)),
            "{bare}"
        );
        assert!(!bare.contains("/admin/apps/"), "{bare}");
    }

    #[test]
    fn a_failed_host_call_states_its_cause_and_not_the_hosts_words() {
        let hc = quota_refusal();
        // The noted call itself, escaped: only that takes the note.
        let thrown = format!(
            "function threw: HostError: ctx.oltp: {}\n    at oxy:bootstrap:154:19",
            hc.message
        );
        let escaped = Failure::of("error", 0, Some(&thrown), None, Some(&hc)).unwrap();
        let text = page_text(&escaped, Some("poke-house-staging"), None);
        assert!(
            text.contains("• cause: `oltp.query` failed as `quota_exceeded` — the backend refused"),
            "{text}"
        );
        assert!(text.contains("kind `threw`"), "{text}");
        // The host's text feeds the fingerprint and nothing else.
        assert!(!text.contains("exceeded the quota"), "{text}");
        assert!(!text.contains(&hc.message), "{text}");

        // It let the call escape, so the message IS in the column; and the
        // functions that share the cause are held back rather than paged.
        let line = where_to_look(&escaped);
        assert!(line.contains("did not catch"), "{line}");
        assert!(
            line.contains("the message is in `app_function_invocations.error`"),
            "{line}"
        );
        assert!(line.contains("`suppressed:cause_rate`"), "{line}");

        let unreachable = HostCallFailure {
            kind: "unreachable",
            ..quota_refusal()
        };
        assert!(cause_line(&unreachable).contains("nothing answered"));
        // A kind with no refusal to explain is named and left at that.
        let plain = HostCallFailure {
            kind: "host_call_failed",
            ..quota_refusal()
        };
        assert_eq!(
            cause_line(&plain),
            "• cause: `oltp.query` failed as `host_call_failed`"
        );
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn custom_app_pages_prefer_their_own_channel_and_fall_back_to_the_ops_one() {
        let both = [
            ("OXY_OPS_SLACK_BOT_TOKEN", "xoxb"),
            ("OXY_OPS_SLACK_CHANNEL", "C_OPS"),
            ("OXY_OPS_SLACK_CUSTOM_APPS_CHANNEL", "C_APPS"),
        ];
        assert_eq!(
            ops_slack_target_from(env(&both)),
            Some(("xoxb".into(), "C_APPS".into()))
        );
        // Unset or blank: the ops channel, exactly as before.
        let ops_only = [
            ("OXY_OPS_SLACK_BOT_TOKEN", "xoxb"),
            ("OXY_OPS_SLACK_CHANNEL", "C_OPS"),
        ];
        assert_eq!(
            ops_slack_target_from(env(&ops_only)),
            Some(("xoxb".into(), "C_OPS".into()))
        );
        let blank = [
            ("OXY_OPS_SLACK_BOT_TOKEN", "xoxb"),
            ("OXY_OPS_SLACK_CHANNEL", "C_OPS"),
            ("OXY_OPS_SLACK_CUSTOM_APPS_CHANNEL", "  "),
        ];
        assert_eq!(
            ops_slack_target_from(env(&blank)),
            Some(("xoxb".into(), "C_OPS".into()))
        );
        // No token, no pages — whichever channel is set.
        assert_eq!(
            ops_slack_target_from(env(&[("OXY_OPS_SLACK_CUSTOM_APPS_CHANNEL", "C_APPS")])),
            None
        );
    }

    #[test]
    fn a_host_call_page_names_the_op_and_kind_instead_of_the_null_error_column() {
        let hc = HostCallFailure {
            op: "warehouse.insert",
            kind: "host_call_failed",
            message: "warehouse insert failed: query failed: HTTP # Bad Request: Code: #.".into(),
            target: None,
        };
        let caught = Failure::of("success", 200, None, None, Some(&hc)).unwrap();
        let line = where_to_look(&caught);
        assert!(line.contains("`warehouse.insert`"), "{line}");
        assert!(line.contains("`host_call_failed`"), "{line}");
        assert!(line.contains("is NULL"), "{line}");
        assert!(!line.contains("The message is in"), "{line}");
        assert!(line.contains("`suppressed:cause_rate`"), "{line}");
        // The message feeds the fingerprint and stops there: the page names
        // the op and kind, and nothing of what the host said.
        assert!(!line.contains("Bad Request"), "{line}");
        assert!(!line.contains(&hc.message), "{line}");

        let threw = Failure::of("error", 0, Some("function threw: Error: x"), None, None).unwrap();
        assert!(where_to_look(&threw).starts_with("The message is in"));
    }

    /// Every heap kill carries the same message, so every function that is
    /// killed has the same fingerprint, in every app. The generic last line
    /// reads a fingerprint shared between functions as the platform failing,
    /// which is backwards for this kind: the first such page (2026-10-05) sent
    /// its reader to look for a platform fault behind two upload handlers.
    #[test]
    fn an_exceeded_memory_page_does_not_read_a_shared_fingerprint_as_a_platform_fault() {
        let killed = Failure::of(
            "error",
            0,
            Some("function exceeded its memory limit"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(killed.kind, "exceeded_memory");

        let line = where_to_look(&killed);
        assert!(line.contains("heap limit"), "{line}");
        assert!(line.contains("does not point at the platform"), "{line}");
        assert!(
            !line.contains("points at the platform rather than the app"),
            "{line}"
        );
        // What to read next is the size of the request, and where it is.
        assert!(line.contains("`request_body_bytes`"), "{line}");
    }
}
