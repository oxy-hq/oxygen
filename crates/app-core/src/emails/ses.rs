use async_trait::async_trait;
use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message as SesMessage};
use oxy_shared::errors::OxyError;

use super::{EmailMessage, EmailProvider};

pub struct SesEmailProvider {
    client: aws_sdk_sesv2::Client,
}

impl SesEmailProvider {
    pub async fn new(aws_region: Option<&str>) -> Self {
        let mut sdk_config_loader = aws_config::from_env();
        if let Some(region) = aws_region {
            sdk_config_loader =
                sdk_config_loader.region(aws_config::Region::new(region.to_string()));
        }
        let sdk_config = sdk_config_loader.load().await;
        Self {
            client: aws_sdk_sesv2::Client::new(&sdk_config),
        }
    }
}

#[async_trait]
impl EmailProvider for SesEmailProvider {
    async fn send(&self, from: &str, to: &str, message: EmailMessage) -> Result<(), OxyError> {
        self.client
            .send_email()
            .from_email_address(from)
            .destination(Destination::builder().to_addresses(to).build())
            .content(
                EmailContent::builder()
                    .simple(
                        SesMessage::builder()
                            .subject(
                                Content::builder()
                                    .data(&message.subject)
                                    .charset("UTF-8")
                                    .build()
                                    .map_err(|e| {
                                        OxyError::ConfigurationError(format!(
                                            "SES subject error: {e}"
                                        ))
                                    })?,
                            )
                            .body(
                                Body::builder()
                                    .html(
                                        Content::builder()
                                            .data(message.html_body)
                                            .charset("UTF-8")
                                            .build()
                                            .map_err(|e| {
                                                OxyError::ConfigurationError(format!(
                                                    "SES html body error: {e}"
                                                ))
                                            })?,
                                    )
                                    .text(
                                        Content::builder()
                                            .data(message.text_body)
                                            .charset("UTF-8")
                                            .build()
                                            .map_err(|e| {
                                                OxyError::ConfigurationError(format!(
                                                    "SES text body error: {e}"
                                                ))
                                            })?,
                                    )
                                    .build(),
                            )
                            .build(),
                    )
                    .build(),
            )
            .send()
            .await
            .map_err(|e| {
                OxyError::ConfigurationError(format!("Failed to send email via SES: {e}"))
            })?;

        Ok(())
    }
}
