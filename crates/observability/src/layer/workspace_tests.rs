//! Which workspace a span row is stamped with.
//!
//! These are the write half of tenant scoping (`crate::scope`): a read can
//! only keep a workspace's traces apart if every row of a trace was stored
//! with the workspace its root named — and with nothing when the root named
//! none.

use tokio::sync::mpsc;
use tracing_subscriber::layer::SubscriberExt;

use super::*;

const ACME: &str = "70787bb2-e11b-5488-b2c3-02e60d5fc7d3";
const GLOBEX: &str = "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d";

/// Run `spans` under a collector and return what it stored, keyed by span name.
fn collect(spans: impl FnOnce()) -> HashMap<String, SpanRecord> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let subscriber =
        tracing_subscriber::registry().with(SpanCollectorLayer::new(tx, "test".to_string()));
    tracing::subscriber::with_default(subscriber, spans);

    let mut records = HashMap::new();
    while let Ok(record) = rx.try_recv() {
        records.insert(record.span_name.clone(), record);
    }
    records
}

fn workspace_of_span<'a>(records: &'a HashMap<String, SpanRecord>, name: &str) -> &'a str {
    &records
        .get(name)
        .unwrap_or_else(|| panic!("no record for span {name}"))
        .workspace_id
}

#[test]
fn a_root_that_names_its_workspace_hands_it_to_every_span_beneath_it() {
    let acme = Uuid::parse_str(ACME).unwrap();
    let records = collect(|| {
        // The shape `analytics.run` is created with: `%uuid`.
        let root = tracing::info_span!("root", oxy.workspace_id = %acme);
        let _root = root.enter();
        let child = tracing::info_span!("child");
        let _child = child.enter();
        let grandchild = tracing::info_span!("grandchild");
        let _grandchild = grandchild.enter();
    });

    // Every row, not the root alone: a trace's rows arrive in separate
    // flushes, and a read filters each row it returns.
    for name in ["root", "child", "grandchild"] {
        assert_eq!(workspace_of_span(&records, name), ACME, "{name}");
    }
}

#[test]
fn a_trace_whose_root_names_no_workspace_belongs_to_nobody() {
    let records = collect(|| {
        let root = tracing::info_span!("root");
        let _root = root.enter();
        let child = tracing::info_span!("child");
        let _child = child.enter();
    });

    // `""`, which no `WorkspaceScope` can equal.
    assert_eq!(workspace_of_span(&records, "root"), "");
    assert_eq!(workspace_of_span(&records, "child"), "");
}

/// A trace is one tenant's or nobody's. If a span under ACME's root could
/// stamp itself GLOBEX, its row — prompt, SQL and all — would be served to
/// GLOBEX's console as part of a trace GLOBEX never ran.
#[test]
fn a_span_cannot_leave_the_workspace_its_trace_belongs_to() {
    let records = collect(|| {
        let root = tracing::info_span!("root", oxy.workspace_id = ACME);
        let _root = root.enter();
        let child = tracing::info_span!("child", oxy.workspace_id = GLOBEX);
        let _child = child.enter();
        let late = tracing::info_span!("late", oxy.workspace_id = tracing::field::Empty);
        let _late = late.enter();
        late.record("oxy.workspace_id", GLOBEX);
    });

    assert_eq!(workspace_of_span(&records, "child"), ACME);
    assert_eq!(workspace_of_span(&records, "late"), ACME);
}

/// The `Empty`-then-`record` shape `llm.call` uses. The span that records its
/// workspace gets it, and so does everything opened afterwards; a child opened
/// *before* the record was handed nothing, which is why a root names its
/// workspace at creation.
#[test]
fn a_workspace_recorded_after_creation_reaches_the_span_and_later_children() {
    let records = collect(|| {
        let root = tracing::info_span!("root", oxy.workspace_id = tracing::field::Empty);
        let _root = root.enter();
        {
            let early = tracing::info_span!("early");
            let _early = early.enter();
        }
        root.record("oxy.workspace_id", ACME);
        let later = tracing::info_span!("later");
        let _later = later.enter();
    });

    assert_eq!(workspace_of_span(&records, "root"), ACME);
    assert_eq!(workspace_of_span(&records, "later"), ACME);
    assert_eq!(workspace_of_span(&records, "early"), "");
}

/// An LLM call outside any run names its own workspace (`GenAiContext`). That
/// row is that workspace's; the tenant-less root above it stays nobody's.
#[test]
fn a_span_under_a_tenantless_root_may_name_its_own_workspace() {
    let records = collect(|| {
        let root = tracing::info_span!("root");
        let _root = root.enter();
        let call = tracing::info_span!("call", oxy.workspace_id = ACME);
        let _call = call.enter();
    });

    assert_eq!(workspace_of_span(&records, "root"), "");
    assert_eq!(workspace_of_span(&records, "call"), ACME);
}

#[test]
fn a_stamp_that_is_not_a_workspace_id_leaves_the_row_unstamped() {
    let records = collect(|| {
        let root = tracing::info_span!("root", oxy.workspace_id = "demo");
        let _root = root.enter();
        let child = tracing::info_span!("child");
        let _child = child.enter();
    });

    assert_eq!(workspace_of_span(&records, "root"), "");
    assert_eq!(workspace_of_span(&records, "child"), "");
}

/// The stamp is stored in the one spelling a scope compares against, whatever
/// case the caller wrote.
#[test]
fn a_stamp_is_stored_in_the_spelling_a_scope_uses() {
    let records = collect(|| {
        let root = tracing::info_span!("root", oxy.workspace_id = ACME.to_uppercase());
        let _root = root.enter();
    });

    assert_eq!(workspace_of_span(&records, "root"), ACME);
    assert_eq!(
        crate::scope::WorkspaceScope::of(Uuid::parse_str(ACME).unwrap()).workspace_id(),
        workspace_of_span(&records, "root")
    );
}
