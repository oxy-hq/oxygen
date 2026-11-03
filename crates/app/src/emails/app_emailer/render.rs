//! A `ctx.email.send` message validated and composed, and not sent.
//!
//! An environment whose messages plane is `capture` keeps what a function
//! sent in its outbox instead of mailing it. The message still has to be a
//! valid one: [`AppEmailer::render`] runs the checks `send` runs, and builds
//! the MIME `send` would hand to SES, so a payload production would refuse
//! fails here with the same label, and one it accepts is kept as it would
//! have gone out.

use super::{AppEmailer, EmailSendInput};

/// A message as `send` would have sent it: normalized addresses, the
/// platform-composed body parts, the attachments by file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEmail {
    pub subject: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub reply_to: Option<String>,
    pub html: Option<String>,
    pub text: Option<String>,
    /// File names only: the bytes are not kept.
    pub attachments: Vec<String>,
}

impl AppEmailer {
    /// Validate and compose `input`, send nothing. `Err` is `send`'s own for
    /// the same payload.
    pub fn render(&self, input: EmailSendInput) -> Result<RenderedEmail, String> {
        let msg = self.validate(input)?;
        // Composed for the failure alone, as `send` composes before it forks
        // between SES and the local preview.
        self.compose(&msg)?;
        Ok(RenderedEmail {
            subject: msg.subject,
            to: msg.to,
            cc: msg.cc,
            bcc: msg.bcc,
            reply_to: msg.reply_to,
            html: msg.html,
            text: msg.text,
            attachments: msg.attachments.into_iter().map(|a| a.filename).collect(),
        })
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
