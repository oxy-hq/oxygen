//! Who asks for a compile, and when. No database: every case here fails (or is
//! served) at the YAML load, which is before `start_airway_run` touches one.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{load_pipeline_yaml_for_request, submit_airway_run};
use crate::airway_run::{AirwayRunError, StartAirwayRequest, start_airway_run};
use crate::pipeline_ref::PipelineRefError;

const PIPELINE_REF: &str = "pipelines/p.airway.yml";
const COMPILE_REQUESTED: &str = "A compile has been requested";

/// A host as `pipeline_ref`'s own tests fake it, plus the compile port: what it
/// answers, and how many times it was asked.
struct Host {
    root: PathBuf,
    compiled: Result<Option<String>, String>,
    revision: Option<Uuid>,
    takes_compile_requests: bool,
    compile_requests: AtomicUsize,
}

impl Host {
    /// A replica: no working copy, serving a promoted revision, with a compile
    /// queue to ask. `compiled` is what the boundary answers for the ref.
    fn replica(compiled: Result<Option<String>, String>) -> Self {
        Self {
            root: PathBuf::from("/nonexistent-oxy-workspace/does/not/exist"),
            compiled,
            revision: Some(Uuid::new_v4()),
            takes_compile_requests: true,
            compile_requests: AtomicUsize::new(0),
        }
    }

    fn compile_requests(&self) -> usize {
        self.compile_requests.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for Host {
    fn workspace_path(&self) -> Option<&Path> {
        Some(&self.root)
    }
    fn compiled_revision(&self) -> Option<Uuid> {
        self.revision
    }
    async fn request_compile(&self) -> bool {
        self.compile_requests.fetch_add(1, Ordering::SeqCst);
        self.takes_compile_requests
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        _name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err("unused".into())
    }
    async fn get_integration(
        &self,
        _name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err("unused".into())
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(&self, _r: &str) -> Result<String, crate::WorkspaceReadError> {
        Err("unused".into())
    }
    async fn resolve_pipeline_yaml(&self, _pipeline_ref: &str) -> Result<Option<String>, String> {
        self.compiled.clone()
    }
}

fn start_request() -> StartAirwayRequest {
    serde_json::from_value(serde_json::json!({ "pipeline_ref": PIPELINE_REF }))
        .expect("a start body is just a pipeline_ref")
}

/// THE case this module exists for: a replica asked to act on a ref its
/// promoted revision does not serve. The answer stays `NotInRevision` — it is
/// still not servable here — but a compile has been asked for, and the message
/// says so, so "retry" is something the caller can act on.
#[tokio::test]
async fn a_request_for_a_ref_the_revision_does_not_serve_asks_for_a_compile() {
    let host = Host::replica(Ok(None));
    assert!(!host.root.exists(), "precondition: no working copy");

    let err = load_pipeline_yaml_for_request(&host, PIPELINE_REF)
        .await
        .expect_err("the revision does not serve it and there is no disk to read");

    assert!(
        matches!(err, PipelineRefError::NotInRevision(_)),
        "got {err:?}"
    );
    assert_eq!(host.compile_requests(), 1, "exactly one compile request");
    assert!(err.to_string().contains(COMPILE_REQUESTED), "{err}");
}

/// A host with no compile queue was asked and declined. The message must not
/// then promise a compile nobody will run.
#[tokio::test]
async fn a_host_with_no_compile_queue_is_not_said_to_have_been_asked() {
    let host = Host {
        takes_compile_requests: false,
        ..Host::replica(Ok(None))
    };

    let err = load_pipeline_yaml_for_request(&host, PIPELINE_REF)
        .await
        .expect_err("still not servable");

    assert!(
        matches!(err, PipelineRefError::NotInRevision(_)),
        "got {err:?}"
    );
    assert!(!err.to_string().contains(COMPILE_REQUESTED), "{err}");
}

/// A compile fixes nothing when the boundary could not be ASKED — a database
/// blip, or no promoted revision at all. Asking for one there would turn every
/// blip into a new revision.
#[tokio::test]
async fn an_unanswered_boundary_does_not_ask_for_a_compile() {
    let blip = Host::replica(Err("connection reset by peer".into()));
    let err = load_pipeline_yaml_for_request(&blip, PIPELINE_REF)
        .await
        .expect_err("could not be asked");
    assert!(
        matches!(err, PipelineRefError::Unavailable(_)),
        "got {err:?}"
    );
    assert_eq!(blip.compile_requests(), 0);

    let nothing_promoted = Host {
        revision: None,
        ..Host::replica(Ok(None))
    };
    let err = load_pipeline_yaml_for_request(&nothing_promoted, PIPELINE_REF)
        .await
        .expect_err("nothing to ask");
    assert!(
        matches!(err, PipelineRefError::Unavailable(_)),
        "got {err:?}"
    );
    assert_eq!(nothing_promoted.compile_requests(), 0);
}

/// A served ref is served, and nothing is requested.
#[tokio::test]
async fn a_ref_the_boundary_serves_asks_for_nothing() {
    let host = Host::replica(Ok(Some("name: from_boundary\n".into())));
    let yaml = load_pipeline_yaml_for_request(&host, PIPELINE_REF)
        .await
        .expect("served from the boundary");
    assert_eq!(yaml, "name: from_boundary\n");
    assert_eq!(host.compile_requests(), 0);
}

/// "Existing behaviour is unchanged on a pod WITH a working copy", pinned.
///
/// Same host answers as the replica case — a promoted revision that declines
/// the ref — but the root is a real directory. The file there is read, a file
/// that is not there is the caller's mistake (`Invalid`, a 400), and in neither
/// case is a compile requested: the working copy already is the answer.
#[tokio::test]
async fn a_node_with_a_working_copy_never_asks_for_a_compile() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("pipelines")).expect("mkdir");
    std::fs::write(dir.path().join(PIPELINE_REF), "name: from_fs\n").expect("write");
    let host = Host {
        root: dir.path().to_path_buf(),
        ..Host::replica(Ok(None))
    };

    let yaml = load_pipeline_yaml_for_request(&host, PIPELINE_REF)
        .await
        .expect("read from the working copy");
    assert_eq!(yaml, "name: from_fs\n");

    let err = load_pipeline_yaml_for_request(&host, "pipelines/nope.airway.yml")
        .await
        .expect_err("no such file");
    assert!(matches!(err, PipelineRefError::Invalid(_)), "got {err:?}");

    assert_eq!(host.compile_requests(), 0);
}

/// The submit path carries the same distinction out as `AirwayRunError`, and
/// asks for the compile — so `POST /runs` on a replica answers 503 with
/// something behind the retry.
#[tokio::test]
async fn an_interactive_submit_asks_for_a_compile_and_stays_retryable() {
    let host = Host::replica(Ok(None));
    // Never queried: the load fails before `start_airway_run` reaches the
    // database, and a `Disconnected` handle would error if that ever changed.
    let db = DatabaseConnection::default();

    let err = submit_airway_run(&db, &host, start_request(), Uuid::new_v4())
        .await
        .expect_err("not servable on this node");

    assert!(
        matches!(err, AirwayRunError::NotInRevision(_)),
        "got {err:?}"
    );
    assert_eq!(host.compile_requests(), 1);
    assert!(err.to_string().contains(COMPILE_REQUESTED), "{err}");
}

/// The other half of the module's reason to exist: `start_airway_run` itself —
/// what a schedule tick, a retry and an Oxy Function call — reports the same
/// condition and requests NOTHING. A tick repeats on a timer; a compile per
/// tick for a ref that is gone would be a recompile storm.
#[tokio::test]
async fn a_non_interactive_start_never_asks_for_a_compile() {
    let host = Host::replica(Ok(None));
    let db = DatabaseConnection::default();

    let err = start_airway_run(
        &db,
        &host,
        start_request(),
        crate::TaskScope::Global,
        Uuid::new_v4(),
    )
    .await
    .expect_err("not servable on this node");

    assert!(
        matches!(err, AirwayRunError::NotInRevision(_)),
        "got {err:?}"
    );
    assert_eq!(host.compile_requests(), 0);
}

/// A blip at submit is `Unavailable`, not `NotInRevision`, and requests nothing.
#[tokio::test]
async fn a_submit_during_a_boundary_blip_is_unavailable_and_asks_for_nothing() {
    let host = Host::replica(Err("connection reset by peer".into()));
    let db = DatabaseConnection::default();

    let err = submit_airway_run(&db, &host, start_request(), Uuid::new_v4())
        .await
        .expect_err("could not be asked");

    assert!(matches!(err, AirwayRunError::Unavailable(_)), "got {err:?}");
    assert_eq!(host.compile_requests(), 0);
}
