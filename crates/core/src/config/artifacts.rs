use oxy_shared::errors::OxyError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    pub name: String,
    pub file_path: String,
    pub title: Option<String>,
    pub published: bool,
}

/// An Airway pipeline as a listing row. `name` follows the same rule as every
/// other compiled entity: the YAML `name:`, else derived from the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineEntry {
    pub name: String,
    pub file_path: String,
    /// `source.kind` — what the UI keys source-specific surfaces on. Both
    /// origins fill it the same way ([`pipeline_source_kind`]), so the compiled
    /// row and the working-copy read answer the same kind for the same
    /// pipeline. `None` means the definition could not be read or did not
    /// parse — a surface gated on it stays hidden rather than appearing and
    /// then failing.
    pub source_kind: Option<String>,
}

/// `source.kind` out of a pipeline definition, if it names one.
pub fn pipeline_source_kind(definition: &serde_json::Value) -> Option<String> {
    definition
        .get("source")
        .and_then(|src| src.get("kind"))
        .and_then(|k| k.as_str())
        .filter(|k| !k.is_empty())
        .map(str::to_string)
}

/// An analytics agent as a listing row. `model_ref` and `timezone` are pulled
/// out so the home page can flag a missing LLM key for the agent chat will
/// actually use, and render the workspace's local clock, without a caller
/// re-parsing the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    pub name: String,
    pub file_path: String,
    pub model_ref: Option<String>,
    pub timezone: Option<String>,
}

/// An automation as a listing row.
///
/// `extension` is carried because three of them are accepted — `.automation.yml`
/// is canonical, `.procedure.yml` is legacy-but-live, and `.workflow.yml` is no
/// longer a recognised file kind, so the walker never compiles one and it
/// resolves from the working copy or not at all. The file tree groups by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationEntry {
    pub name: String,
    pub file_path: String,
    pub extension: String,
}

/// One compiled artifact with its full body — the shape every kind shares
/// (semantic views, topics, automations), as opposed to the listing rows above
/// which carry only what a list needs.
///
/// `blob_key` rather than the body: when a large body lives in S3 the row keeps
/// only the key, and fetching it needs the S3 client, which lives above this
/// crate. Core reports what the row says; the caller resolves the blob and
/// falls back to `definition` when the bucket is unset or the object is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledArtifact {
    pub name: String,
    pub file_path: String,
    pub definition: serde_json::Value,
    pub blob_key: Option<String>,
}

/// A declared world (`.simulation.yml`) as a listing row.
///
/// The one listing row that carries its whole `definition`. Both callers need
/// the body and neither can get it from a second lookup for free: the grid
/// renders each world's declared horizon and mechanism straight off the list,
/// and a run reads its seed and replicate count out of the same value. A row
/// without it would send every caller back for one read per world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationEntry {
    pub name: String,
    pub file_path: String,
    pub definition: serde_json::Value,
}

/// A verified query (`.sql`). No parsed `definition` — its body IS the SQL,
/// carried verbatim with the hash the compile worker recorded so a reader can
/// check the Postgres/S3 round-trip did not corrupt it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedQueryEntry {
    pub file_path: String,
    pub content_sha256: String,
    pub content: String,
}

/// A markdown context document (`.md`) an agent's `context:` reaches. Like a
/// verified query, its body IS the artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextDocument {
    /// Workspace-relative, `/`-separated — the same string on both arms.
    pub file_path: String,
    pub content: String,
}

/// What a manager can say about the context documents an agent reads.
///
/// Three different things can be true and each has its own shape, because a
/// caller does something different with each: `Read` is an answer, an `Err` is
/// "could not look", and `NotCompiled` is neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextDocuments {
    /// Looked. These are the agent's documents, in its pattern order. An empty
    /// list is an answer too: the agent reads none.
    Read(Vec<ContextDocument>),
    /// Nobody looked, and nobody can from here. The pinned revision was
    /// compiled before documents were a compiled kind, so it says nothing
    /// about them, and this node holds no files to read instead.
    ///
    /// Not an error: it is the state every revision was in before documents
    /// were compiled, and a run on such a pod has always gone ahead without
    /// them. Not `Read(vec![])` either: that would claim the agent has none,
    /// when the next compile may well carry some. A caller that proceeds
    /// should say so and ask for that compile.
    NotCompiled,
}

impl ContextDocuments {
    /// The documents, when this is an answer.
    pub fn read(self) -> Option<Vec<ContextDocument>> {
        match self {
            Self::Read(documents) => Some(documents),
            Self::NotCompiled => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("workspace is not available on this node: {0}")]
    WorkspaceUnavailable(String),
    #[error("not in the compiled revision, and this process holds no working copy")]
    NoSource,
    #[error("compile boundary unavailable: {0}")]
    Backend(String),
    #[error(transparent)]
    Config(#[from] OxyError),
}

/// So a caller still on `OxyError` can use `?` without restating the mapping.
/// The wrapped variant passes through unchanged; the three that describe a
/// source being unavailable become a runtime error, because that is what they
/// are to code that cannot act on the distinction.
impl From<ArtifactError> for OxyError {
    fn from(e: ArtifactError) -> Self {
        match e {
            ArtifactError::Config(inner) => inner,
            other => OxyError::RuntimeError(other.to_string()),
        }
    }
}

impl ArtifactError {
    pub fn retryable(&self) -> bool {
        !matches!(self, Self::Config(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_config_fault_is_permanent() {
        assert!(ArtifactError::NoSource.retryable());
        assert!(ArtifactError::WorkspaceUnavailable("x".into()).retryable());
        assert!(ArtifactError::Backend("db down".into()).retryable());
        assert!(
            !ArtifactError::Config(OxyError::ConfigurationError("bad yaml".into())).retryable(),
            "a broken file is the caller's fault and will not fix itself on retry"
        );
    }
}
