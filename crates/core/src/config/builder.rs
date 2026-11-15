use std::path::Path;

use oxy_shared::errors::OxyError;

use super::{
    compiled,
    manager::{ConfigManager, Origin, ReadOnly, WorkingCopy},
    model::Config,
    storage::{ConfigStorage, FsStorage},
};

/// What a missing or unreadable `config.yml` means to this caller.
///
/// `Fail` is for anything that needs a real workspace — `oxy run`, the compile
/// worker. `Empty` is for surfaces that must still render for a workspace with
/// no config yet, notably first-run onboarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnMissing {
    Fail,
    Empty,
}

pub struct ConfigBuilder {
    storage: Option<FsStorage>,
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigBuilder {
    pub fn new() -> Self {
        Self { storage: None }
    }

    pub fn with_workspace_path<P: AsRef<Path>>(
        mut self,
        workspace_path: P,
    ) -> Result<Self, OxyError> {
        self.storage = Some(FsStorage::new(workspace_path)?);
        Ok(self)
    }

    pub async fn build_with_working_copy(
        self,
        origin: Origin,
        on_missing: OnMissing,
    ) -> Result<ConfigManager<WorkingCopy>, OxyError> {
        let storage = self.take_storage()?;
        let (config, origin) = load_config(Some(&storage), origin, on_missing).await?;
        let config = with_workspace_path(config, &storage);
        Ok(ConfigManager::new(
            config,
            WorkingCopy::new(storage),
            origin,
        ))
    }

    /// A manager with no filesystem capability — which is NOT the same as
    /// "construction may not read the disk". Whether the resulting manager may
    /// walk the working copy later, and where its `Config` came from, are
    /// separate questions: on the ide (and in local mode) the working copy is
    /// right there and an uncompiled workspace must still read its real
    /// `config.yml` from it.
    ///
    /// `Origin::Disk` with no slot is NOT a contradiction, and an earlier
    /// version of this refused it. `Origin` says where the `Config` came from;
    /// the slot says whether later artifact reads may walk the working copy.
    /// The toast webhook is the worked example: on the ide it builds a
    /// slot-less manager whose `config.yml` was read right here, deliberately,
    /// so integrations resolve while artifact reads still cannot reach a disk.
    /// Refusing the pair turned "this workspace is unconfigured" into a 500 —
    /// `toast_webhook_compile_boundary` (`oxy-api-world-model`'s integration
    /// group) says the unpromoted case must resolve
    /// to `None`, not error.
    pub async fn build_without_working_copy(
        self,
        origin: Origin,
        on_missing: OnMissing,
    ) -> Result<ConfigManager<ReadOnly>, OxyError> {
        // The path is still required — `Config::workspace_path` is `#[serde(skip)]`,
        // so a compiled config comes back with it empty and downstream resolvers
        // depend on it.
        let storage = self.take_storage()?;
        let (config, origin) = load_config(Some(&storage), origin, on_missing).await?;
        Ok(ConfigManager::new(
            with_workspace_path(config, &storage),
            ReadOnly::empty(),
            origin,
        ))
    }

    /// Build around a `Config` the caller already holds, skipping the load.
    ///
    /// The escape hatch for the one thing the two loading terminals cannot do:
    /// produce a manager whose `Origin` is `Compiled` without a database. The
    /// loaders deliberately downgrade to `Disk` when a revision yields no
    /// config, so a test pinning the capability/origin combination has to state
    /// it directly. Not for request paths — those should pass a revision and
    /// let the manager load.
    pub fn build_with_provided_config_and_working_copy(
        self,
        config: Config,
        origin: Origin,
    ) -> Result<ConfigManager<WorkingCopy>, OxyError> {
        let storage = self.take_storage()?;
        let config = with_workspace_path(config, &storage);
        Ok(ConfigManager::new(
            config,
            WorkingCopy::new(storage),
            origin,
        ))
    }

    fn take_storage(self) -> Result<FsStorage, OxyError> {
        self.storage.ok_or(OxyError::RuntimeError(
            "Config source is required".to_string(),
        ))
    }
}

/// Load the config the origin points at, and report the origin it actually came
/// from.
///
/// The returned origin can be weaker than the requested one: a workspace whose
/// revision carries no readable config row is served from the working copy, and
/// the manager must then say `Disk` — otherwise every later read would query the
/// boundary for a revision this one already found wanting.
async fn load_config(
    storage: Option<&FsStorage>,
    origin: Origin,
    on_missing: OnMissing,
) -> Result<(Config, Origin), OxyError> {
    if let Origin::Compiled { revision_id, .. } = origin {
        match compiled::load_config_at(revision_id).await {
            Ok(Some(json)) => match serde_json::from_value::<Config>(json) {
                Ok(config) => return Ok((config, origin)),
                Err(e) => tracing::warn!(
                    %revision_id,
                    error = %e,
                    "compiled config failed to deserialise; falling through to the working copy"
                ),
            },
            Ok(None) => {}
            Err(e) => tracing::warn!(
                %revision_id,
                error = %e,
                "compiled config unavailable; falling through to the working copy"
            ),
        }
    }

    let Some(storage) = storage else {
        return Err(OxyError::ConfigurationError(
            "workspace has no compiled config and this process holds no working \
             copy to read one from"
                .to_string(),
        ));
    };
    let config = match on_missing {
        OnMissing::Fail => storage.load_config().await?,
        OnMissing::Empty => storage.load_config_with_fallback().await,
    };
    Ok((config, Origin::Disk))
}

fn with_workspace_path(mut config: Config, storage: &FsStorage) -> Config {
    config.workspace_path = storage.project_path().to_path_buf();
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{DatabaseType, DuckDBOptions};

    /// Ported from the recovery router when config loading moved here. Two
    /// things the compiled arm must get right, neither visible from a type:
    /// the compiler-injected `s3_mirror` has to survive the JSONB round-trip,
    /// and `workspace_path` has to be re-stamped because it is
    /// `#[serde(skip)]` — unstamped, every file resolver (the DuckDB dataset
    /// dir, a BigQuery `key_path`) would resolve against an empty path.
    #[test]
    fn a_compiled_config_keeps_its_s3_mirror_and_gets_its_workspace_path_back() {
        // Shaped like a real promoted revision: the working copy's `config.yml`
        // for this workspace has the `dataset` but NO `s3_mirror` — the compile
        // worker adds that on the way into Postgres.
        let json = serde_json::json!({
            "defaults": null,
            "builder_agent": null,
            "models": [],
            "databases": [{
                "name": "local",
                "type": "duckdb",
                "dataset": ".db/",
                "s3_mirror": {
                    "bucket": "oxy-compile-blobs",
                    "region": "us-east-1",
                    "tables": [
                        { "table": "orders", "key": "ws/local/orders.csv", "format": "csv" }
                    ]
                }
            }],
        });

        let config: Config = serde_json::from_value(json).expect("well-formed compiled config");
        assert_eq!(
            config.workspace_path,
            std::path::PathBuf::new(),
            "the JSONB round-trip cannot carry it — that is why it is re-stamped"
        );

        let storage = FsStorage::new("/state/workspaces/abc").expect("storage");
        let config = with_workspace_path(config, &storage);

        let db = config.databases.first().expect("one database");
        let DatabaseType::DuckDB(duck) = &db.database_type else {
            panic!("expected a duckdb database, got {:?}", db.database_type);
        };
        let mirror = duck
            .s3_mirror
            .as_ref()
            .expect("the compiler-injected s3_mirror must survive into the manager");
        assert_eq!(mirror.bucket, "oxy-compile-blobs");
        // The local dataset path is still carried — a node WITH the working copy
        // uses it; only the stateless roles take the mirror arm.
        assert!(matches!(duck.options, DuckDBOptions::Local { .. }));
        assert_eq!(
            config.workspace_path,
            std::path::PathBuf::from("/state/workspaces/abc")
        );
    }

    /// Schema drift against an old revision must fall through to the working
    /// copy rather than fail the build. `load_config` logs and continues on this
    /// error; the assertion here is that it IS an error rather than a partial
    /// `Config` nobody notices.
    #[test]
    fn an_undeserialisable_compiled_config_is_an_error_not_a_partial_config() {
        let json = serde_json::json!({ "databases": "not-an-array" });
        assert!(serde_json::from_value::<Config>(json).is_err());
    }
}
