//! The values of the `ctx.env` keys one invocation read through staging's or
//! production's shared fallback, and the test for whether a request carries
//! one (`provider_fetch`). Never printed.

/// The values of the `ctx.env` keys this invocation read through a fallback.
/// Never printed: `Debug` says how many there are and nothing else.
#[derive(Default)]
pub(super) struct FallbackSecrets(Vec<String>);

impl FallbackSecrets {
    /// The non-empty values of `values`; an empty one would match everything.
    pub(super) fn new(values: Vec<String>) -> Self {
        Self(values.into_iter().filter(|v| !v.is_empty()).collect())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether any value is a substring of `haystack`, as written.
    pub(super) fn found_in(&self, haystack: &str) -> bool {
        self.0
            .iter()
            .any(|secret| haystack.contains(secret.as_str()))
    }

    /// [`Self::found_in`], and also in `text` percent-decoded
    /// (`encodeURIComponent` turns `+ / = @ :` and a space into `%2B`…).
    pub(super) fn found_in_percent_encoded(&self, text: &str) -> bool {
        self.found_in(text) || self.found_in(&percent_decoded(text))
    }

    /// [`Self::found_in_percent_encoded`], and also in `text` form-decoded
    /// (`URLSearchParams` writes a space as `+`).
    pub(super) fn found_in_form_encoded(&self, text: &str) -> bool {
        self.found_in_percent_encoded(text)
            || self.found_in(&percent_decoded(&text.replace('+', " ")))
    }
}

/// `text` with every `%XX` decoded; a stray `%` stays as written.
pub(super) fn percent_decoded(text: &str) -> String {
    String::from_utf8_lossy(&urlencoding::decode_binary(text.as_bytes())).into_owned()
}

impl std::fmt::Debug for FallbackSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FallbackSecrets({} values)", self.0.len())
    }
}
