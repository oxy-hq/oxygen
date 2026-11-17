//! Build a semantic model at compile time, the way a serving pod will.
//!
//! `oxy_compile` hands the gate each view's and topic's pushed text
//! (`oxy_compile::ConfigGate::check_semantic_model`); this builds the model
//! from it with the reader's own pieces, in the reader's own order:
//! [`super::scan::reader_order`] for the order, `build_layer_from_yaml` for the
//! layer, and `build_engine` over the compiled `config.yml`'s databases for
//! the engine (the dialect mapping `database_configs` uses at request time).
//! A model that passes here is one the reader can build; a model that fails
//! here would have failed every semantic request on every pod.
//!
//! Per-file parse errors are attributed to their file first, so the failure an
//! operator sees names the view to fix rather than "the model".

use std::path::PathBuf;

use oxy_compile::{CompiledConfig, SemanticModelError, SemanticSource, merge_compiled_config};

use super::model::Config;
use super::scan::{SemanticFile, reader_order, relative_scan_path};

/// Build the model `views` and `topics` describe, under `config`'s dialects.
pub fn check_compiled_semantic_model(
    config: Option<&CompiledConfig>,
    views: &[SemanticSource<'_>],
    topics: &[SemanticSource<'_>],
) -> Result<(), SemanticModelError> {
    let views = in_reader_order(views, "view.yml");
    let topics = in_reader_order(topics, "topic.yml");
    for file in &views {
        oxy_airlayer_compat::parse_view_yaml(&file.yaml).map_err(|e| in_file(file, e))?;
    }
    for file in &topics {
        oxy_airlayer_compat::parse_topic_yaml(&file.yaml).map_err(|e| in_file(file, e))?;
    }
    let view_texts: Vec<&str> = views.iter().map(|f| f.yaml.as_str()).collect();
    let topic_texts: Vec<&str> = topics.iter().map(|f| f.yaml.as_str()).collect();
    let layer = oxy_airlayer_compat::build_layer_from_yaml(&view_texts, &topic_texts)
        .map_err(model_wide)?;
    // A config that does not deserialise fails the compile on its own, in the
    // config gate; without one there is no dialect map to check the engine
    // against, so only the layer is checked.
    let Some(databases) = dialect_databases(config) else {
        return Ok(());
    };
    oxy_airlayer_compat::build_engine(layer, &databases).map_err(model_wide)?;
    Ok(())
}

/// The files as the scan would hand them to the parser.
fn in_reader_order(sources: &[SemanticSource<'_>], suffix: &str) -> Vec<SemanticFile> {
    let keyed: Vec<(PathBuf, SemanticFile)> = sources
        .iter()
        .filter_map(|source| {
            let relative = relative_scan_path(source.file_path, "", suffix)?;
            Some((
                relative,
                SemanticFile {
                    file_path: source.file_path.to_string(),
                    yaml: source.text.to_string(),
                },
            ))
        })
        .collect();
    reader_order(keyed)
}

/// The dialect map a serving pod builds the engine with: `config.yml`'s
/// databases, in order (order is load-bearing: the first is the default
/// dialect). `None` when there is no compiled config to read it from.
fn dialect_databases(
    config: Option<&CompiledConfig>,
) -> Option<Vec<oxy_airlayer_compat::DatabaseConfig>> {
    let merged = merge_compiled_config(config?);
    let config = serde_json::from_value::<Config>(merged).ok()?;
    Some(
        config
            .databases
            .iter()
            .map(|db| oxy_airlayer_compat::database_config(db.name.clone(), db.dialect()))
            .collect(),
    )
}

fn in_file(file: &SemanticFile, e: oxy_airlayer_compat::SemanticError) -> SemanticModelError {
    SemanticModelError {
        file_path: Some(file.file_path.clone()),
        message: e.to_string(),
    }
}

fn model_wide(e: oxy_airlayer_compat::SemanticError) -> SemanticModelError {
    SemanticModelError {
        file_path: None,
        message: e.to_string(),
    }
}

#[cfg(test)]
#[path = "semantic_check_tests.rs"]
mod tests;
