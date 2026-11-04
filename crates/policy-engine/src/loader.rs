//! Loads `file://` JavaScript modules. TypeScript is compiled ahead of time by
//! `build-js.sh` (esbuild, as `oxyc publish` does for Oxy Functions), so the
//! engine carries no transpiler; a `.ts` path is refused with the fix in the
//! message.

use deno_core::error::ModuleLoaderError;
use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind, resolve_import,
};
use deno_error::JsErrorBox;

pub(crate) struct JsModuleLoader;

impl ModuleLoader for JsModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        resolve_import(specifier, referrer).map_err(JsErrorBox::from_err)
    }

    fn load(
        &self,
        specifier: &ModuleSpecifier,
        _referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        ModuleLoadResponse::Sync(load(specifier))
    }
}

fn load(specifier: &ModuleSpecifier) -> Result<ModuleSource, ModuleLoaderError> {
    let path = specifier
        .to_file_path()
        .map_err(|_| JsErrorBox::generic(format!("only file:// modules load: {specifier}")))?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("js" | "mjs") => {}
        Some("ts" | "mts") => {
            return Err(JsErrorBox::generic(format!(
                "{specifier} is TypeScript; the engine runs JavaScript only. \
                 Compile it first (crates/policy-engine/build-js.sh) and load the .js"
            )));
        }
        _ => {
            return Err(JsErrorBox::generic(format!(
                "not a JavaScript module: {specifier}"
            )));
        }
    }
    let code = std::fs::read_to_string(&path).map_err(JsErrorBox::from_err)?;
    Ok(ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String(code.into()),
        specifier,
        None,
    ))
}
