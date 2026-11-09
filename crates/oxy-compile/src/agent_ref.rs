//! THE normalisation of an agent reference: the one place that says which
//! spellings a ref (`agent_ref:` in an automation, a Slack saved path, a chat
//! run's agent id) may resolve by. The compiler's `agent_ref` check and the
//! runtime lookup (`ConfigManager::agent_by_ref`) both build on it, so the
//! compiler never calls "resolves" a ref the runtime answers "not found" for.
//!
//! This does not decide what the compiler writes as an agent's key (`name:`,
//! else the path minus the extension): that is the retirement memo's open Q4.

/// The extensions an agentic agent's file may carry, most common first.
pub const AGENT_EXTENSIONS: &[&str] = &[".agentic.yml", ".agentic.yaml"];

/// The three spellings one ref names, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRefSpellings<'a> {
    /// The ref as written, minus a leading `./`: a workspace path or a name.
    pub full: &'a str,
    /// `full` minus `.agentic.yml` / `.agentic.yaml`: the compile key of a
    /// nameless agent.
    pub bare: &'a str,
    /// The last `/` segment of `bare`: the file stem.
    pub stem: &'a str,
}

/// The spellings of `agent_ref`, taken exactly as written apart from a
/// leading `./` (no whitespace trimming: a stored key is matched as stored).
pub fn agent_ref_spellings(agent_ref: &str) -> AgentRefSpellings<'_> {
    let full = agent_ref.trim_start_matches("./");
    let bare = strip_agent_extension(full);
    let stem = bare.rsplit('/').next().unwrap_or(bare);
    AgentRefSpellings { full, bare, stem }
}

/// The spellings an `agent_ref` may resolve by, most specific first and
/// without repeats: the full ref, the ref minus its extension, and the stem.
/// Surrounding whitespace is trimmed first, as an automation's YAML may carry
/// it.
pub fn agent_ref_candidates(target: &str) -> Vec<String> {
    let AgentRefSpellings { full, bare, stem } = agent_ref_spellings(target.trim());
    let mut out = vec![full.to_string()];
    for c in [bare, stem] {
        if !out.iter().any(|o| o == c) {
            out.push(c.to_string());
        }
    }
    out
}

fn strip_agent_extension(path: &str) -> &str {
    AGENT_EXTENSIONS
        .iter()
        .find_map(|ext| path.strip_suffix(ext))
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_split_a_path_into_its_three_keys() {
        assert_eq!(
            agent_ref_spellings("./agents/sales/foo.agentic.yaml"),
            AgentRefSpellings {
                full: "agents/sales/foo.agentic.yaml",
                bare: "agents/sales/foo",
                stem: "foo",
            }
        );
        let bare = agent_ref_spellings("foo");
        assert_eq!((bare.full, bare.bare, bare.stem), ("foo", "foo", "foo"));
    }

    /// A classic path is not an agentic one: nothing is stripped, so it names
    /// no compiled agent rather than being quietly read as its sibling.
    #[test]
    fn a_classic_path_keeps_its_extension() {
        let s = agent_ref_spellings("agents/foo.agent.yml");
        assert_eq!(s.bare, "agents/foo.agent.yml");
        assert_eq!(s.stem, "foo.agent.yml");
    }
}
