//! Looker CLI commands for synchronizing and managing Looker integration metadata

use clap::Parser;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::WorkingCopy;
use oxy::config::model::IntegrationType;
use oxy::config::resolve_local_workspace_path;
use oxy::service::looker_sync::{LookerSyncService, SyncResult};
use oxy::theme::StyledText;
use oxy_looker::{LookerApiClient, LookerAuthConfig};
use oxy_shared::errors::OxyError;
use uuid::Uuid;

/// Looker integration commands for synchronizing and managing metadata
#[derive(Parser, Debug)]
pub struct LookerArgs {
    #[clap(subcommand)]
    pub command: LookerCommand,
}

#[derive(Parser, Debug)]
pub enum LookerCommand {
    /// Synchronize Looker explore metadata from the API
    ///
    /// Fetch metadata from Looker and store it locally for use by agents and automations.
    /// Without arguments, syncs all configured integrations and explores.
    Sync(LookerSyncArgs),
    /// List synchronized Looker explores
    ///
    /// Show which explores have been synchronized for an integration.
    List(LookerListArgs),
    /// Test Looker integration connection
    ///
    /// Verify that the Looker API credentials are valid and the connection works.
    Test(LookerTestArgs),
}

#[derive(Parser, Debug)]
pub struct LookerSyncArgs {
    /// Name of the Looker integration to sync (syncs all if not specified)
    #[clap(long)]
    pub integration: Option<String>,
    /// LookML model to sync (syncs all explores in model if explore not specified)
    #[clap(long)]
    pub model: Option<String>,
    /// Specific explore to sync within the model
    #[clap(long)]
    pub explore: Option<String>,
    /// Force re-sync even if metadata already exists
    #[clap(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Parser, Debug)]
pub struct LookerListArgs {
    /// Name of the Looker integration to list explores for
    #[clap(long)]
    pub integration: Option<String>,
    /// LookML model to list explores for
    #[clap(long)]
    pub model: Option<String>,
}

#[derive(Parser, Debug)]
pub struct LookerTestArgs {
    /// Name of the Looker integration to test
    #[clap(long)]
    pub integration: Option<String>,
}

/// Handle the `oxy looker` subcommand
pub async fn handle_looker_command(args: LookerArgs) -> Result<(), OxyError> {
    match args.command {
        LookerCommand::Sync(sync_args) => handle_looker_sync(sync_args).await,
        LookerCommand::List(list_args) => handle_looker_list(list_args).await,
        LookerCommand::Test(test_args) => handle_looker_test(test_args).await,
    }
}

/// Handle the `oxy looker sync` command
pub async fn handle_looker_sync(args: LookerSyncArgs) -> Result<(), OxyError> {
    let workspace_path = resolve_local_workspace_path()?;

    let project = WorkspaceBuilder::new(Uuid::nil())
        .with_working_copy(&workspace_path, None, oxy::config::OnMissing::Fail)
        .await?
        .build()
        .await
        .map_err(|e| OxyError::from(anyhow::anyhow!("Failed to create project: {e}")))?;

    let config = project.config_manager.clone();

    let looker_integrations: Vec<_> = config
        .get_config()
        .integrations
        .iter()
        .filter_map(|integration| match &integration.integration_type {
            IntegrationType::Looker(looker_integration) => {
                if let Some(ref filter_name) = args.integration
                    && &integration.name != filter_name
                {
                    return None;
                }
                Some((integration.name.clone(), looker_integration.clone()))
            }
            _ => None,
        })
        .collect();

    if looker_integrations.is_empty() {
        if args.integration.is_some() {
            return Err(OxyError::ConfigurationError(format!(
                "Looker integration '{}' not found",
                args.integration.unwrap()
            )));
        }
        println!("{}", "No Looker integrations configured.".warning());
        return Ok(());
    }

    println!(
        "{}",
        format!(
            "🔗 Synchronizing {} Looker integration(s)...",
            looker_integrations.len()
        )
        .info()
    );

    let mut all_results: Vec<SyncResult> = Vec::new();

    for (integration_name, looker_integration) in looker_integrations {
        println!(
            "\n📦 Processing integration: {}",
            integration_name.primary()
        );

        // Resolve API credentials from environment variables
        let client_id = project
            .secrets_manager
            .resolve_secret(&looker_integration.client_id_var)
            .await?
            .ok_or_else(|| {
                OxyError::ConfigurationError(format!(
                    "Looker client ID not found in environment variable: {}",
                    looker_integration.client_id_var
                ))
            })?;

        let client_secret = project
            .secrets_manager
            .resolve_secret(&looker_integration.client_secret_var)
            .await?
            .ok_or_else(|| {
                OxyError::ConfigurationError(format!(
                    "Looker client secret not found in environment variable: {}",
                    looker_integration.client_secret_var
                ))
            })?;

        // Create API client
        let auth_config = LookerAuthConfig {
            base_url: looker_integration.base_url.clone(),
            client_id,
            client_secret,
        };

        let api_client = LookerApiClient::new(auth_config).map_err(|e| {
            OxyError::RuntimeError(format!("Failed to create Looker API client: {}", e))
        })?;

        // Get state directory for metadata storage
        let state_dir = project.config_manager.resolve_state_dir().await?;

        let mut sync_service = LookerSyncService::new(
            api_client,
            state_dir.join(".looker"),
            workspace_path.join("looker"),
            integration_name.clone(),
        );

        // Determine what to sync based on arguments
        if let Some(ref model) = args.model {
            if let Some(ref explore) = args.explore {
                println!(
                    "  🔄 Syncing explore: {}/{}.{}",
                    integration_name, model, explore
                );
                match sync_service.sync_explore(model, explore).await {
                    Ok(()) => {
                        println!(
                            "  {} {}.{}",
                            "✅".success(),
                            model.secondary(),
                            explore.secondary()
                        );
                        all_results.push(SyncResult {
                            integration: integration_name.clone(),
                            model: model.clone(),
                            total_explores: 1,
                            successful: vec![explore.clone()],
                            failed: vec![],
                            skipped: vec![],
                        });
                    }
                    Err(e) => {
                        println!(
                            "  {} {}.{}: {}",
                            "❌".error(),
                            model,
                            explore,
                            e.to_string().error()
                        );
                        all_results.push(SyncResult {
                            integration: integration_name.clone(),
                            model: model.clone(),
                            total_explores: 1,
                            successful: vec![],
                            failed: vec![(explore.clone(), e.to_string())],
                            skipped: vec![],
                        });
                    }
                }
            } else {
                println!("  🔄 Syncing model: {}/{}", integration_name, model);
                match sync_service.sync_model(model).await {
                    Ok(result) => {
                        print_sync_result(&result);
                        all_results.push(result);
                    }
                    Err(e) => {
                        println!(
                            "  {} Failed to sync model {}: {}",
                            "❌".error(),
                            model,
                            e.to_string().error()
                        );
                    }
                }
            }
        } else {
            for explore_config in &looker_integration.explores {
                println!(
                    "  🔄 Syncing explore: {}/{}.{}",
                    integration_name, explore_config.model, explore_config.name
                );

                // Check if already synced and not forcing
                if !args.force
                    && sync_service
                        .is_explore_synchronized(&explore_config.model, &explore_config.name)
                {
                    println!(
                        "  {} {}.{} (already synced, use --force to re-sync)",
                        "⏭️".tertiary(),
                        explore_config.model.secondary(),
                        explore_config.name.secondary()
                    );
                    continue;
                }

                match sync_service
                    .sync_explore(&explore_config.model, &explore_config.name)
                    .await
                {
                    Ok(()) => {
                        println!(
                            "  {} {}.{}",
                            "✅".success(),
                            explore_config.model.secondary(),
                            explore_config.name.secondary()
                        );
                    }
                    Err(e) => {
                        println!(
                            "  {} {}.{}: {}",
                            "❌".error(),
                            explore_config.model,
                            explore_config.name,
                            e.to_string().error()
                        );
                    }
                }
            }
        }
    }

    // Print summary
    println!();
    if all_results.iter().all(|r| r.is_success()) {
        println!(
            "{}",
            "🎉 Looker synchronization completed successfully!".success()
        );
    } else if all_results.iter().any(|r| r.is_partial_success()) {
        println!(
            "{}",
            "⚠️ Looker synchronization completed with some errors.".warning()
        );
    } else if all_results.iter().all(|r| r.is_failure()) {
        println!("{}", "❌ Looker synchronization failed.".error());
    }

    Ok(())
}

/// Handle the `oxy looker list` command
async fn handle_looker_list(args: LookerListArgs) -> Result<(), OxyError> {
    let workspace_path = resolve_local_workspace_path()?;

    let project = WorkspaceBuilder::new(Uuid::nil())
        .with_working_copy(&workspace_path, None, oxy::config::OnMissing::Fail)
        .await?
        .build()
        .await
        .map_err(|e| OxyError::from(anyhow::anyhow!("Failed to create project: {e}")))?;

    let config = project.config_manager.clone();

    // Get state directory for metadata storage
    let state_dir = project.config_manager.resolve_state_dir().await?;

    let looker_integrations: Vec<_> = config
        .get_config()
        .integrations
        .iter()
        .filter_map(|integration| match &integration.integration_type {
            IntegrationType::Looker(looker_integration) => {
                if let Some(ref filter_name) = args.integration
                    && &integration.name != filter_name
                {
                    return None;
                }
                Some((integration.name.clone(), looker_integration.clone()))
            }
            _ => None,
        })
        .collect();

    if looker_integrations.is_empty() {
        if args.integration.is_some() {
            return Err(OxyError::ConfigurationError(format!(
                "Looker integration '{}' not found",
                args.integration.unwrap()
            )));
        }
        println!("{}", "No Looker integrations configured.".warning());
        return Ok(());
    }

    for (integration_name, looker_integration) in looker_integrations {
        println!("\n📦 Integration: {}", integration_name.primary());
        println!("   Base URL: {}", looker_integration.base_url.secondary());

        // Create a minimal storage to list explores
        let storage = oxy_looker::MetadataStorage::new(
            state_dir.join(".looker"),
            workspace_path.join("looker"),
            integration_name.clone(),
        );

        // Get unique models from configured explores or filter by model
        let models_to_list: Vec<String> = if let Some(ref model_filter) = args.model {
            vec![model_filter.clone()]
        } else {
            looker_integration
                .explores
                .iter()
                .map(|e| e.model.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect()
        };

        for model in models_to_list {
            println!("\n   📁 Model: {}", model.secondary());

            match storage.list_base_explores(&model) {
                Ok(explores) => {
                    if explores.is_empty() {
                        println!("      {} No explores synchronized", "ℹ️".tertiary());
                    } else {
                        for explore in explores {
                            println!("      ✅ {}", explore);
                        }
                    }
                }
                Err(_) => {
                    println!("      {} No explores synchronized", "ℹ️".tertiary());
                }
            }
        }
    }

    Ok(())
}

/// Handle the `oxy looker test` command
async fn handle_looker_test(args: LookerTestArgs) -> Result<(), OxyError> {
    let workspace_path = resolve_local_workspace_path()?;

    let project = WorkspaceBuilder::<WorkingCopy>::new(Uuid::nil())
        .with_working_copy(&workspace_path, None, oxy::config::OnMissing::Fail)
        .await?
        .build()
        .await
        .map_err(|e| OxyError::from(anyhow::anyhow!("Failed to create project: {e}")))?;

    let config = project.config_manager.clone();

    let looker_integrations: Vec<_> = config
        .get_config()
        .integrations
        .iter()
        .filter_map(|integration| match &integration.integration_type {
            IntegrationType::Looker(looker_integration) => {
                if let Some(ref filter_name) = args.integration
                    && &integration.name != filter_name
                {
                    return None;
                }
                Some((integration.name.clone(), looker_integration.clone()))
            }
            _ => None,
        })
        .collect();

    if looker_integrations.is_empty() {
        if args.integration.is_some() {
            return Err(OxyError::ConfigurationError(format!(
                "Looker integration '{}' not found",
                args.integration.unwrap()
            )));
        }
        println!("{}", "No Looker integrations configured.".warning());
        return Ok(());
    }

    println!("{}", "🔍 Testing Looker connections...".info());

    for (integration_name, looker_integration) in looker_integrations {
        println!("\n📦 Testing integration: {}", integration_name.primary());

        // Resolve API credentials from environment variables
        let client_id = match project
            .secrets_manager
            .resolve_secret(&looker_integration.client_id_var)
            .await?
        {
            Some(id) => id,
            None => {
                println!(
                    "   {} Client ID not found (env var: {})",
                    "❌".error(),
                    looker_integration.client_id_var
                );
                continue;
            }
        };

        let client_secret = match project
            .secrets_manager
            .resolve_secret(&looker_integration.client_secret_var)
            .await?
        {
            Some(secret) => secret,
            None => {
                println!(
                    "   {} Client secret not found (env var: {})",
                    "❌".error(),
                    looker_integration.client_secret_var
                );
                continue;
            }
        };

        println!("   ✅ Credentials found");
        println!(
            "   🌐 Connecting to {}...",
            looker_integration.base_url.secondary()
        );

        // Create API client and test connection
        let auth_config = LookerAuthConfig {
            base_url: looker_integration.base_url.clone(),
            client_id,
            client_secret,
        };

        let mut api_client: LookerApiClient = match LookerApiClient::new(auth_config) {
            Ok(client) => client,
            Err(e) => {
                let error_msg = e.to_string();
                println!(
                    "   {} Failed to create client: {}",
                    "❌".error(),
                    error_msg.error()
                );
                continue;
            }
        };

        // Test by listing models (this will trigger authentication)
        match api_client.list_models().await {
            Ok(models) => {
                println!("   {} Connection successful!", "✅".success());
                let model_count = models.len();
                println!("   📋 Available models: {}", model_count);
                for model in models.iter().take(5) {
                    let explore_count = model.explores.len();
                    println!(
                        "      - {} ({} explores)",
                        model.name.secondary(),
                        explore_count
                    );
                }
                if model_count > 5 {
                    println!("      ... and {} more", model_count - 5);
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                println!(
                    "   {} Connection failed: {}",
                    "❌".error(),
                    error_msg.error()
                );
            }
        }
    }

    Ok(())
}

fn print_sync_result(result: &SyncResult) {
    println!(
        "   {}",
        format!(
            "Synced {}/{} explores",
            result.successful.len(),
            result.total_explores
        )
        .info()
    );

    for explore in &result.successful {
        println!("      {} {}", "✅".success(), explore);
    }

    for (explore, error) in &result.failed {
        println!("      {} {}: {}", "❌".error(), explore, error.error());
    }

    for explore in &result.skipped {
        println!("      {} {} (skipped)", "⏭️".tertiary(), explore);
    }
}
