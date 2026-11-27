//! Promote checks: every build that reaches a custom app's staging is
//! checked automatically, and the results stay on the build
//! (`app_build_checks`), for the console's Live build card and Releases and
//! for `oxyc checks show`. The living contract is
//! `internal-docs/custom-app-sandboxes.md` § "Promote checks".
//!
//! **Whether a check holds a promote is the org's `promote_checks_gate`
//! flag** ([`settings`]). Off (phase 1), an attempt has four kinds and
//! nothing here changes promote. On, an attempt also has `smoke` and
//! `changes`, and the kinds an app requires (`apps.promotion_checks`) hold
//! its promote to production: `custom_apps_promote::checks_gate` reads these
//! rows and decides. On, production is checked too, after every move
//! ([`production`]).
//!
//! **Two environments** ([`environment`]). A *staging attempt* is what every
//! build that reaches staging gets. A *production attempt* is queued after
//! production's pointer moves to another build, with one kind,
//! `production_smoke`; a failure of it tells people and, where the app says
//! so, rolls production back. Every route that names an attempt names its
//! environment; absent, it is staging.
//!
//! **Producer.** Every move that makes staging serve a build queues ONE
//! durable task for it ([`queue::after_staging_move`]) with its `queued`
//! rows ([`task::BuildChecksTask::kinds`]): a publish to the channels or a
//! sandbox agent token's draft (`custom_apps_publish::pipeline`), a sandbox's
//! build promoted to staging and staging rolled back
//! (`custom_apps_promote::act`). Idempotent per
//! (environment, app, build, attempt); a staging move never fails for it.
//! [`queue::rerun`] queues the next attempt.
//!
//! **The task only starts a runner** ([`executor`] → [`dispatch`]): one GitHub
//! `workflow_dispatch`, then it ends. No check runs on the worker fleet.
//!
//! **The runner drives the checks** with one command, `oxyc checks run
//! <app-id> --build <row id> --attempt <n> --report`, on a GitHub Actions
//! runner signed in as the platform checker ([`runner`], [`admit`]):
//!
//! | Kind | Verdict computed by | How |
//! | --- | --- | --- |
//! | `validation` | the platform, from the bundle's recorded validation | written when the runner claims the attempt ([`validation`]) |
//! | `functions` | the platform, from its own function-run records | the build's `"check": true` functions are started at the claim and settled as the runner polls ([`functions`]) |
//! | `loads` | the runner: Playwright at desk 1440×900 and phone 390×844, waiting for `oxy-app-ready` | reported |
//! | `design_system` | the runner: the SDK floor, and the SDK's theme guard over the CSS [`design_system`] hands it | reported |
//! | `smoke`, `changes` | the runner (gate on only; `changes` is advisory) | reported |
//! | `production_smoke` | the runner, on production, against what production showed before the move (a production attempt's one kind) | reported |
//!
//! **Routes.** The console's, in the staff console nest ([`handlers`]): `GET
//! …/builds/{build}/checks`, `POST …/builds/{build}/checks/rerun`, `GET
//! …/checks/files/{name}` ([`files`]) and `POST …/checks/changes/seen`
//! ([`seen`]). The runner's five, outside that nest's layers ([`runner`]):
//! `POST …/checks/claim`, `POST …/checks/functions/settle`, `GET
//! …/checks/stylesheets`, `POST …/checks/report`, `PUT …/checks/files/{name}`.
//!
//! **An attempt nobody answers** is read off its rows ([`stalled`]): no runner
//! within ten minutes of being queued, or a runner whose latest claim left a
//! check open for thirty. Such an attempt reads `failed`, its open checks
//! `error`, and a rerun may start at once. **An attempt nothing was started
//! for** (a deployment with no runner configured) says so on its rows
//! (`stalled::NotStarted`) and is not one of those: its open checks read
//! `skipped` at once, saying how to run them by hand, and a rerun may always
//! start. An advisory row never stalls an attempt and is never left open
//! either: once nobody will write it, it is answered like the others, and an
//! attempt whose runner died with only `changes` left is closed by its next
//! reader and settles ([`settled::close_abandoned`]).
//!
//! **A skip nobody decided by judging the build is no pass**
//! ([`dto::not_checked`]): a check of an attempt nothing was started for,
//! and a check a claim closed because its environment no longer served the
//! build (`kind::Outcome::not_checked`). Both read `skipped`; neither
//! satisfies the gate, and their attempt never rolls up as `passed`.
//!
//! **What is kept.** In Postgres: statuses, a one-line summary and capped
//! findings, and the *names* of an attempt's files. The files themselves (a
//! screen's picture on staging and on production, their diff, a changed
//! screen's visible text, the CSS digest) are bytes in the build store,
//! beside the build and deleted with it ([`files`]); no trace or response
//! body is kept anywhere.
//!
//! **A settled attempt** is one whose every row that is not a model's has a
//! verdict. A staging attempt is handed on once when it settles ([`settled`]):
//! where the advisory notes start.
//!
//! **Advisory notes** ([`ai`]). A row carries `notes` beside its `detail`, and
//! a build one `summary` row. Where the gate is on, a settled staging attempt
//! queues one task that has the workspace's own model write them: a note on
//! each check that did not pass, a note on the changed screens, a
//! `design_audit` row and the summary. They inform and nothing else: no
//! status, state, gate or stall reads them.

pub(crate) mod admit;
pub mod ai;
pub mod design_system;
pub mod dispatch;
pub mod dto;
pub mod environment;
pub mod error;
pub mod executor;
pub mod files;
pub mod functions;
pub mod handlers;
pub mod kind;
pub mod production;
pub mod queue;
pub mod requester;
pub mod runner;
pub mod seen;
pub mod settings;
pub mod settled;
pub mod stalled;
pub mod store;
pub mod task;
pub mod validation;

pub use ai::BUILD_CHECKS_AI_KIND;
pub use production::PRODUCTION_FAILED_KIND;
pub use task::BUILD_CHECKS_KIND;
