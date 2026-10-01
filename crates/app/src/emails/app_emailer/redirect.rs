//! Outside production, `ctx.email.send` delivers to the invoking user alone
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §4.2): a
//! staging run of a function that emails a customer must never reach that
//! customer, and the person testing it should see exactly what production
//! would have sent.
//!
//! `replyTo` is the invoking user too: a reply to a staging message must not
//! reach whoever production's copy would have routed replies to. The message
//! is otherwise untouched — body, attachments and the validation `send`
//! applies — so a staging send fails for the same payload errors production's
//! would.

use super::{EmailSendInput, OneOrMany, addresses, check_recipients};

/// The recipients a redirected message named and did not reach, reported back
/// to the function (never logged: they are people's addresses).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IgnoredRecipients {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
}

impl IgnoredRecipients {
    /// How many addresses were dropped.
    pub fn len(&self) -> usize {
        self.to.len() + self.cc.len() + self.bcc.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "to": self.to, "cc": self.cc, "bcc": self.bcc })
    }
}

impl EmailSendInput {
    /// This message delivered to `recipient` alone, replies going back to
    /// `recipient` too, its subject prefixed `[<environment>] `. Returns what it
    /// would otherwise have reached.
    ///
    /// The recipients the call named are checked by production's rules first
    /// (none, or too many, is refused), and a blank or missing subject is left
    /// as it is for `send` to refuse — so staging fails exactly where
    /// production would.
    pub fn redirected_to(
        mut self,
        recipient: &str,
        environment: &str,
    ) -> Result<(Self, IgnoredRecipients), String> {
        let ignored = IgnoredRecipients {
            to: addresses(self.to.take()),
            cc: addresses(self.cc.take()),
            bcc: addresses(self.bcc.take()),
        };
        check_recipients(&ignored.to, &ignored.cc, &ignored.bcc)?;
        self.to = Some(OneOrMany::One(recipient.to_string()));
        self.reply_to = Some(recipient.to_string());
        self.subject = self.subject.take().map(|s| match s.trim() {
            "" => s,
            subject => format!("[{environment}] {subject}"),
        });
        Ok((self, ignored))
    }

    /// Everyone the message would reach — `to`, `cc` and `bcc`.
    #[cfg(test)]
    pub(crate) fn recipients(&self) -> Vec<String> {
        let own = |f: &Option<OneOrMany>| match f {
            Some(OneOrMany::One(one)) => vec![one.clone()],
            Some(OneOrMany::Many(many)) => many.clone(),
            None => Vec::new(),
        };
        [own(&self.to), own(&self.cc), own(&self.bcc)].concat()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(v: serde_json::Value) -> EmailSendInput {
        serde_json::from_value(v).expect("parses")
    }

    #[test]
    fn only_the_invoker_receives_it_and_the_rest_is_reported() {
        let (msg, ignored) = input(serde_json::json!({
            "to": ["cfo@customer.com", "ops@customer.com"],
            "cc": "controller@customer.com",
            "bcc": ["audit@customer.com"],
            "subject": "JE posted",
            "text": "x",
        }))
        .redirected_to("staff@oxy.tech", "staging")
        .expect("redirects");
        assert_eq!(addresses(msg.to), vec!["staff@oxy.tech"]);
        assert!(msg.cc.is_none() && msg.bcc.is_none());
        assert_eq!(msg.subject.as_deref(), Some("[staging] JE posted"));
        assert_eq!(ignored.to, vec!["cfo@customer.com", "ops@customer.com"]);
        assert_eq!(ignored.cc, vec!["controller@customer.com"]);
        assert_eq!(ignored.bcc, vec!["audit@customer.com"]);
        assert_eq!(ignored.len(), 4);
    }

    #[test]
    fn replies_go_to_the_invoker_not_the_address_the_call_named() {
        let (msg, _) = input(serde_json::json!({
            "to": "cfo@customer.com", "replyTo": "ap@customer.com",
            "subject": "s", "text": "x",
        }))
        .redirected_to("staff@oxy.tech", "staging")
        .expect("redirects");
        assert_eq!(msg.reply_to.as_deref(), Some("staff@oxy.tech"));
        let (msg, _) = input(serde_json::json!({ "to": "a@b.com", "subject": "s", "text": "x" }))
            .redirected_to("staff@oxy.tech", "staging")
            .expect("redirects");
        assert_eq!(
            msg.reply_to.as_deref(),
            Some("staff@oxy.tech"),
            "set when absent too"
        );
    }

    #[test]
    fn a_missing_subject_stays_missing_so_send_refuses_it_as_in_production() {
        let (msg, _) = input(serde_json::json!({ "to": "a@b.com", "text": "x" }))
            .redirected_to("staff@oxy.tech", "staging")
            .expect("redirects");
        assert!(msg.subject.is_none());
        let (msg, _) = input(serde_json::json!({ "to": "a@b.com", "subject": "  ", "text": "x" }))
            .redirected_to("staff@oxy.tech", "staging")
            .expect("redirects");
        assert_eq!(msg.subject.as_deref(), Some("  "));
    }

    #[test]
    fn recipients_production_would_refuse_are_refused_here_too() {
        let none = input(serde_json::json!({ "subject": "s", "text": "x" }))
            .redirected_to("staff@oxy.tech", "staging")
            .map(|_| ())
            .expect_err("no `to`");
        assert!(none.contains("`to` recipient is required"), "{none}");
        let many: Vec<String> = (0..60).map(|i| format!("u{i}@customer.com")).collect();
        let too_many = input(serde_json::json!({ "to": many, "subject": "s", "text": "x" }))
            .redirected_to("staff@oxy.tech", "staging")
            .map(|_| ())
            .expect_err("over the cap");
        assert!(too_many.starts_with("TooManyRecipients:"), "{too_many}");
    }
}
