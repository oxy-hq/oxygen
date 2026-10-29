//! Multi-template registry. Walks `sdk/create-oxy-app/templates/` at
//! startup, parses each subdirectory's `template.json` metadata, and
//! exposes the list. The actual file rendering lives in the parent
//! module's `render_template_files` function; this module is just
//! the discovery + metadata layer.

use std::collections::BTreeMap;

use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};

/// All baked-in templates. Adding a new template = drop a directory
/// under `sdk/create-oxy-app/templates/` and add the matching `include_dir!`
/// call below.
static VITE_TEMPLATE: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../sdk/create-oxy-app/templates/vite");

static FUNCTIONS_TEMPLATE: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../sdk/create-oxy-app/templates/functions");

static DASHBOARD_TEMPLATE: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../sdk/create-oxy-app/templates/dashboard");

static SINGLE_STORE_TEMPLATE: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../sdk/create-oxy-app/templates/single-store");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateMeta {
    pub id: String,
    pub name: String,
    pub description: String,
}

pub struct Template {
    pub meta: TemplateMeta,
    pub dir: &'static Dir<'static>,
}

/// Returns the template registry. Constructed at first call; baked-in
/// templates never change at runtime, so this is effectively a static
/// lookup table.
pub fn templates() -> &'static BTreeMap<String, Template> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<BTreeMap<String, Template>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut map = BTreeMap::new();
        for dir in [
            &VITE_TEMPLATE,
            &FUNCTIONS_TEMPLATE,
            &DASHBOARD_TEMPLATE,
            &SINGLE_STORE_TEMPLATE,
        ] {
            match load_meta(dir) {
                Ok(meta) => {
                    let id = meta.id.clone();
                    map.insert(id, Template { meta, dir });
                }
                Err(e) => {
                    // A malformed template.json shouldn't crash startup;
                    // log and skip. The skipped template just won't appear
                    // in the gallery.
                    tracing::error!("template registry: skipping bad template: {e}");
                }
            }
        }
        map
    })
}

fn load_meta(dir: &Dir<'_>) -> Result<TemplateMeta, String> {
    let f = dir
        .get_file("template.json")
        .ok_or_else(|| format!("template directory {:?} missing template.json", dir.path()))?;
    let bytes = f.contents();
    serde_json::from_slice::<TemplateMeta>(bytes)
        .map_err(|e| format!("template.json in {:?} not parseable: {}", dir.path(), e))
}

pub fn get_template(id: &str) -> Option<&'static Template> {
    templates().get(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_loads_vite_template() {
        let t = get_template("vite").expect("vite must always be registered");
        assert_eq!(t.meta.id, "vite");
        assert!(!t.meta.name.is_empty());
    }

    // The opt-in template that carries the server-side Oxy Function + email
    // demo. `vite` deliberately ships no `functions/` — scaffolding a backend
    // is an explicit choice, not the default.
    #[test]
    fn registry_loads_functions_template() {
        let t = get_template("functions").expect("functions must be registered");
        assert_eq!(t.meta.id, "functions");
        assert!(!t.meta.name.is_empty());
    }

    // The default template must stay backend-free: no `functions/` handlers and
    // no `emails/` templates. This is the guardrail for the whole point of the
    // `vite` template — a clean custom app, not a backend project.
    #[test]
    fn vite_template_ships_no_backend() {
        let t = get_template("vite").expect("vite must always be registered");
        assert!(
            t.dir.get_dir("functions").is_none(),
            "vite template must not ship server-side functions"
        );
        assert!(
            t.dir.get_dir("emails").is_none(),
            "vite template must not ship email templates"
        );
    }

    // Dashboard + single-store templates have placeholder template.json
    // files (Task 10 fills in real content). Verify they appear in the
    // registry now that the dirs exist.
    #[test]
    fn registry_loads_dashboard_template() {
        let t = get_template("dashboard").expect("dashboard must be registered");
        assert_eq!(t.meta.id, "dashboard");
    }

    #[test]
    fn registry_loads_single_store_template() {
        let t = get_template("single-store").expect("single-store must be registered");
        assert_eq!(t.meta.id, "single-store");
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(get_template("does-not-exist").is_none());
    }
}
