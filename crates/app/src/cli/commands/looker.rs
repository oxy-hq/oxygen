//! Looker CLI commands for checking a Looker integration

use clap::Parser;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::WorkingCopy;
use oxy::config::model::IntegrationType;
use oxy::config::resolve_local_workspace_path;
use oxy::theme::StyledText;
use oxy_looker::{LookerApiClient, LookerAuthConfig};
use oxy_shared::errors::OxyError;
use uuid::Uuid;

/// Looker integration commands
#[derive(Parser, Debug)]
pub struct LookerArgs {
    #[clap(subcommand)]
    pub command: LookerCommand,
}

#[derive(Parser, Debug)]
pub enum LookerCommand {
    /// Test Looker integration connection
    ///
    /// Verify that the Looker API credentials are valid and the connection works.
    Test(LookerTestArgs),
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
        LookerCommand::Test(test_args) => handle_looker_test(test_args).await,
    }
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
