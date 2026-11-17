//! The identity of a compiled revision's semantic model, by content.
//!
//! [`digest`] hashes exactly what the layer is parsed from — every view's and
//! topic's path and text, in the order the scan hands them to the parser — so
//! two scans with the same digest build the same layer and the same engine.
//! `oxy_airlayer_compat::content_alias` keys the layer and engine caches on it,
//! which is what lets a promote that did not touch `semantics/` keep every
//! pod's engine warm. The rule that only fully deterministic scans are recorded
//! lives at the call site, `scan::load_compiled_scan`.

use sha2::{Digest, Sha256};

use super::scan::CompiledSemanticFiles;

/// SHA-256 over the scan's files, length-prefixed so no two different file
/// lists can hash alike by moving a boundary.
pub(super) fn digest(files: &CompiledSemanticFiles) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for (label, list) in [("views", files.views()), ("topics", files.topics())] {
        put(&mut hasher, label);
        hasher.update((list.len() as u64).to_le_bytes());
        for file in list {
            put(&mut hasher, &file.file_path);
            put(&mut hasher, &file.yaml);
        }
    }
    hasher.finalize().into()
}

fn put(hasher: &mut Sha256, part: &str) {
    hasher.update((part.len() as u64).to_le_bytes());
    hasher.update(part.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::super::scan::{CompiledSemanticFiles, SemanticFile};
    use super::*;

    fn file(path: &str, yaml: &str) -> SemanticFile {
        SemanticFile {
            file_path: path.to_string(),
            yaml: yaml.to_string(),
        }
    }

    #[test]
    fn identical_files_have_one_digest_and_any_difference_changes_it() {
        let base = CompiledSemanticFiles::from_parts(
            vec![file("semantics/views/a.view.yml", "name: a")],
            vec![file("semantics/topics/t.topic.yml", "name: t")],
        );
        let same = CompiledSemanticFiles::from_parts(
            vec![file("semantics/views/a.view.yml", "name: a")],
            vec![file("semantics/topics/t.topic.yml", "name: t")],
        );
        assert_eq!(digest(&base), digest(&same));

        for different in [
            // text
            CompiledSemanticFiles::from_parts(
                vec![file("semantics/views/a.view.yml", "name: b")],
                vec![file("semantics/topics/t.topic.yml", "name: t")],
            ),
            // path
            CompiledSemanticFiles::from_parts(
                vec![file("semantics/views/b.view.yml", "name: a")],
                vec![file("semantics/topics/t.topic.yml", "name: t")],
            ),
            // a view moved to the topics list
            CompiledSemanticFiles::from_parts(
                vec![],
                vec![
                    file("semantics/views/a.view.yml", "name: a"),
                    file("semantics/topics/t.topic.yml", "name: t"),
                ],
            ),
            // a boundary moved between path and text
            CompiledSemanticFiles::from_parts(
                vec![file("semantics/views/a.view.ymlname: ", "a")],
                vec![file("semantics/topics/t.topic.yml", "name: t")],
            ),
        ] {
            assert_ne!(digest(&base), digest(&different));
        }
    }

    #[test]
    fn order_is_part_of_the_identity() {
        let a = file("semantics/views/a.view.yml", "name: a");
        let b = file("semantics/views/b.view.yml", "name: b");
        let forward = CompiledSemanticFiles::from_parts(vec![a.clone(), b.clone()], vec![]);
        let backward = CompiledSemanticFiles::from_parts(vec![b, a], vec![]);
        assert_ne!(digest(&forward), digest(&backward));
    }
}
