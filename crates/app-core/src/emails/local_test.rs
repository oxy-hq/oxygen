use async_trait::async_trait;
use oxy_shared::errors::OxyError;

use super::{EmailMessage, EmailProvider};

/// Email provider for local development. Instead of sending email, writes the
/// HTML to a temp file and opens it in the system browser. Enable by setting
/// the `MAGIC_LINK_LOCAL_TEST` environment variable to any non-empty value.
pub struct LocalTestEmailProvider;

#[async_trait]
impl EmailProvider for LocalTestEmailProvider {
    async fn send(&self, _from: &str, to: &str, message: EmailMessage) -> Result<(), OxyError> {
        let path =
            std::env::temp_dir().join(format!("oxy-magic-link-{}.html", uuid::Uuid::new_v4()));

        std::fs::write(&path, &message.html_body).map_err(|e| {
            OxyError::RuntimeError(format!("Failed to write local test email: {e}"))
        })?;

        tracing::info!(
            "MAGIC_LINK_LOCAL_TEST: email for {} written to {}",
            to,
            path.display()
        );

        open_in_browser(&path);

        Ok(())
    }
}

pub(crate) fn open_in_browser(path: &std::path::Path) {
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
        tracing::warn!("MAGIC_LINK_LOCAL_TEST: failed to open browser: {e}");
    }
}
