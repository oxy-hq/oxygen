//! One test per kind: the rendered text is the literal the SDK contract and
//! the classifiers pin, and the constructor's kind is the kind that text
//! parses to — the typed route and the prefix fallback agree.

#[cfg(feature = "custom-app-functions")]
use super::super::runtime::RuntimeError;
use super::*;

/// The kind the string's head names, as an untyped string would get it.
fn parsed(error: &HostError) -> HostErrorKind {
    HostError::from(error.to_string()).kind
}

#[test]
fn capability_missing_renders_the_hosts_words_for_every_labelled_area() {
    for (area, label) in [
        (CapabilityArea::Email, "EmailCapabilityMissing"),
        (CapabilityArea::Notify, "NotifyCapabilityMissing"),
        (CapabilityArea::Org, "OrgCapabilityMissing"),
        (CapabilityArea::Platform, "PlatformCapabilityMissing"),
        (CapabilityArea::Storage, "StorageCapabilityMissing"),
        (CapabilityArea::Oltp, "OltpCapabilityMissing"),
        (CapabilityArea::Airhouse, "AirhouseCapabilityMissing"),
        (CapabilityArea::Events, "EventsCapabilityMissing"),
    ] {
        let text = format!("{label}: this function has not declared the `x` capability");
        let error = HostError::capability_missing(area, text.clone());
        assert_eq!(error.to_string(), text, "{area:?}");
        assert!(error.to_string().starts_with(&format!("{label}: ")));
        assert_eq!(parsed(&error), error.kind, "{area:?}");
        assert_eq!(
            area.label().map(|a| format!("{a}CapabilityMissing")),
            Some(label.to_string())
        );
    }
}

#[test]
fn the_secrets_gate_has_no_label_and_still_parses_to_its_kind() {
    let error = HostError::capability_missing(
        CapabilityArea::Secrets,
        "ctx.secrets.set: this function has not declared the `secrets.write` \
         capability (add it to oxy-app.json to permit writes)",
    );
    assert_eq!(CapabilityArea::Secrets.label(), None);
    assert_eq!(parsed(&error), error.kind);
}

#[test]
fn held_renders_the_environments_label_and_words() {
    let text = "HeldInStaging: ctx.warehouse.insert was not performed.";
    let error = HostError::held(text);
    assert_eq!(error, text);
    assert_eq!(error.kind, HostErrorKind::Held);
    assert_eq!(parsed(&error), HostErrorKind::Held);
}

#[test]
fn refused_renders_the_environments_label_and_words() {
    let text = "EnvironmentRefused: ctx.fetch is refused in the staging environment: x.";
    let error = HostError::refused(text);
    assert_eq!(error, text);
    assert_eq!(error.kind, HostErrorKind::Refused);
    assert_eq!(parsed(&error), HostErrorKind::Refused);
}

#[test]
fn backend_keeps_the_text_and_is_what_an_unlabelled_string_parses_to() {
    let error = HostError::backend("unknown op 'rename'");
    assert_eq!(error, "unknown op 'rename'");
    assert_eq!(error.kind, HostErrorKind::Backend);
    assert_eq!(parsed(&error), HostErrorKind::Backend);
    assert_eq!(
        HostError::from("no colon here").kind,
        HostErrorKind::Backend
    );
}

#[test]
fn threw_renders_function_threw() {
    let error = HostError::threw("TypeError: x is undefined");
    assert_eq!(error, "function threw: TypeError: x is undefined");
    assert_eq!(parsed(&error), HostErrorKind::Threw);
}

#[test]
fn internal_renders_internal_runtime_error() {
    let error = HostError::internal("isolate thread vanished");
    assert_eq!(error, "internal runtime error: isolate thread vanished");
    assert_eq!(parsed(&error), HostErrorKind::Internal);
}

#[test]
fn memory_limit_renders_its_whole_text() {
    let error = HostError::memory_limit();
    assert_eq!(error, "function exceeded its memory limit");
    assert_eq!(parsed(&error), HostErrorKind::MemoryLimit);
}

/// The runtime's own `Display` is the same sentence (`RuntimeError`'s
/// `#[error(..)]` strings), so the two owners cannot drift apart.
#[cfg(feature = "custom-app-functions")]
#[test]
fn the_invocation_kinds_render_what_the_runtime_error_renders() {
    assert_eq!(
        HostError::threw("TypeError: x is undefined").to_string(),
        RuntimeError::Js("TypeError: x is undefined".into()).to_string()
    );
    assert_eq!(
        HostError::internal("isolate thread vanished").to_string(),
        RuntimeError::Internal("isolate thread vanished".into()).to_string()
    );
    assert_eq!(
        HostError::memory_limit().to_string(),
        RuntimeError::OutOfMemory.to_string()
    );
}

#[test]
fn a_label_inside_a_thrown_message_does_not_retype_it() {
    // The function threw an error that quotes a host refusal: still `Threw`.
    let error = HostError::from("function threw: HeldInStaging: ctx.fetch was not performed.");
    assert_eq!(error.kind, HostErrorKind::Threw);
}

#[test]
fn a_string_converts_without_changing_a_byte() {
    for text in [
        "HeldInStaging: x",
        "EnvironmentRefused: x",
        "OltpCapabilityMissing: x",
        "StorageCapabilityMissing: x",
        "ctx.org.people is not available in this host",
        "",
    ] {
        assert_eq!(HostError::from(text).to_string(), text);
        assert_eq!(HostError::from(text.to_string()).as_str(), text);
    }
}
