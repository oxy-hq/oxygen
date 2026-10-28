const STRIPS: &[&str] = &[
    ".agentic.yml",
    ".view.yml",
    ".topic.yml",
    ".procedure.yml",
    ".automation.yml",
    ".airway.yml",
    ".simulation.yml",
    ".yml",
];

pub fn artifact_name(rel_path: &str) -> String {
    for suffix in STRIPS {
        if let Some(stem) = rel_path.strip_suffix(suffix) {
            return stem.to_string();
        }
    }
    rel_path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_suffixes_are_stripped_and_directories_kept() {
        assert_eq!(artifact_name("agents/foo.agentic.yml"), "agents/foo");
        // Longest suffix wins over the bare `.yml`, or a nameless world would
        // be identified as `worlds/flat.simulation` while the compile worker
        // called the same file `worlds/flat`.
        assert_eq!(artifact_name("worlds/flat.simulation.yml"), "worlds/flat");
        assert_eq!(artifact_name("noext"), "noext");
    }
}
