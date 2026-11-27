//! A rendered mail, the one shape both sides of platform mail share.
//!
//! The code that writes a mail (a domain crate's template, such as
//! `oxy_usage_report::email`) and the code that sends one
//! (`oxy_app_core::emails`' providers: SES, the browser preview) live in
//! different crates, and the writer must not depend on the sender. Three
//! strings, so it costs the kernel nothing to own them.

pub struct EmailMessage {
    pub subject: String,
    pub html_body: String,
    pub text_body: String,
}
