//! Local email preview: a mail that would have been sent is written to
//! [`preview_dir`] and opened in the system browser instead. On when
//! `MAGIC_LINK_LOCAL_TEST` (transactional mail) or `OXY_APP_EMAIL_LOCAL_TEST`
//! (custom-app `ctx.email.send`, token mail) is set — every dev box sets both.
//!
//! Opening a tab is the one part with no natural bound, so [`open_in_browser`]
//! holds two:
//!
//! - **Never under the test runner.** nextest sets `NEXTEST=1` in every test
//!   process; a test reads the mail from an outbox, not from a tab. Before this,
//!   one run of the token-auth tests opened a tab per mail it previewed.
//! - **One tab per burst.** A sweep (token expiry notices) or a bulk invite
//!   previews many mails in one go. The first in any [`OPEN_INTERVAL`] opens; the
//!   rest are written and logged with their path, and never lost.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use oxy_shared::errors::OxyError;

use super::{EmailMessage, EmailProvider};

/// Email provider for local development: writes the HTML to [`preview_dir`]
/// and opens it (see the module docs for when it does not).
pub struct LocalTestEmailProvider;

#[async_trait]
impl EmailProvider for LocalTestEmailProvider {
    async fn send(&self, _from: &str, to: &str, message: EmailMessage) -> Result<(), OxyError> {
        let dir = preview_dir();
        prune_old_previews(&dir);
        let path = dir.join(format!("mail-{}.html", uuid::Uuid::new_v4()));

        std::fs::write(&path, &message.html_body).map_err(|e| {
            OxyError::RuntimeError(format!("Failed to write local test email: {e}"))
        })?;

        tracing::info!(
            to,
            subject = %message.subject,
            path = %path.display(),
            "email preview written, not sent"
        );

        open_in_browser(&path);

        Ok(())
    }
}

/// Directory for locally-rendered email previews — one dedicated subdir of the
/// system temp dir (so previews never litter temp), created on demand and kept
/// small by [`prune_old_previews`].
pub(crate) fn preview_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("oxy-email-previews");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Age at which a stale preview file becomes eligible for cleanup.
const PREVIEW_TTL: Duration = Duration::from_secs(3600);

/// Best-effort cleanup: delete preview files older than `PREVIEW_TTL` so the dir
/// stays small. Runs before each write; the just-written file survives until a
/// later send ages it out.
pub(crate) fn prune_old_previews(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let aged_out = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age > PREVIEW_TTL);
        if aged_out {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The shortest gap between two preview tabs from one process.
const OPEN_INTERVAL: Duration = Duration::from_secs(10);

/// When this process last opened a preview tab.
static LAST_OPENED: Mutex<Option<Instant>> = Mutex::new(None);

/// What [`open_in_browser`] does with one written preview.
#[derive(Debug, PartialEq, Eq)]
enum Opening {
    Open,
    /// A test process: the test reads the outbox, never a tab.
    UnderTestRunner,
    /// Another preview opened this long ago, under [`OPEN_INTERVAL`].
    Throttled(Duration),
}

/// Decide, and claim the tab slot when the answer is [`Opening::Open`]. A
/// throttled preview does not push the window out, so a steady stream still
/// gets a tab every [`OPEN_INTERVAL`].
fn decide(under_tests: bool, last: &Mutex<Option<Instant>>, now: Instant) -> Opening {
    if under_tests {
        return Opening::UnderTestRunner;
    }
    let mut last = last.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(at) = *last {
        let since = now.saturating_duration_since(at);
        if since < OPEN_INTERVAL {
            return Opening::Throttled(since);
        }
    }
    *last = Some(now);
    Opening::Open
}

fn under_test_runner() -> bool {
    std::env::var_os("NEXTEST").is_some()
}

/// Open a written preview, within the bounds in the module docs.
pub(crate) fn open_in_browser(path: &Path) {
    match decide(under_test_runner(), &LAST_OPENED, Instant::now()) {
        Opening::Open => spawn_opener(path),
        Opening::UnderTestRunner => {
            tracing::debug!(path = %path.display(), "email preview not opened: test process");
        }
        Opening::Throttled(since) => tracing::info!(
            path = %path.display(),
            dir = %preview_dir().display(),
            "email preview not opened: another opened {}s ago; every preview is kept in dir",
            since.as_secs()
        ),
    }
}

fn spawn_opener(path: &Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();

    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", &path.to_string_lossy()])
        .spawn();

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let result: Result<_, std::io::Error> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "unsupported platform",
    ));

    if let Err(e) = result {
        tracing::warn!("email preview: failed to open browser: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_previews_opens_one_tab() {
        let last = Mutex::new(None);
        let t0 = Instant::now();
        assert_eq!(decide(false, &last, t0), Opening::Open);
        for ms in [1, 50, 2_000, 9_999] {
            assert!(
                matches!(
                    decide(false, &last, t0 + Duration::from_millis(ms)),
                    Opening::Throttled(_)
                ),
                "{ms}ms after the first"
            );
        }
        assert_eq!(decide(false, &last, t0 + OPEN_INTERVAL), Opening::Open);
    }

    #[test]
    fn a_throttled_preview_does_not_push_the_window_out() {
        let last = Mutex::new(None);
        let t0 = Instant::now();
        decide(false, &last, t0);
        decide(false, &last, t0 + Duration::from_secs(9));
        assert_eq!(
            decide(false, &last, t0 + Duration::from_secs(10)),
            Opening::Open
        );
    }

    #[test]
    fn a_test_process_never_opens_and_never_claims_the_slot() {
        let last = Mutex::new(None);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert_eq!(decide(true, &last, t0), Opening::UnderTestRunner);
        }
        assert!(last.lock().unwrap().is_none());
    }
}
