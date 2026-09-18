mod validation;

use async_openai::{
    Client,
    config::{AzureConfig, OpenAIConfig},
    types::{
        chat::{
            ChatCompletionMessageToolCalls, ChatCompletionNamedToolChoice,
            ChatCompletionRequestAssistantMessage, ChatCompletionRequestAssistantMessageContent,
            ChatCompletionRequestAssistantMessageContentPart,
            ChatCompletionRequestDeveloperMessage, ChatCompletionRequestDeveloperMessageContent,
            ChatCompletionRequestDeveloperMessageContentPart, ChatCompletionRequestMessage,
            ChatCompletionRequestSystemMessage, ChatCompletionRequestSystemMessageContent,
            ChatCompletionRequestSystemMessageContentPart, ChatCompletionRequestToolMessage,
            ChatCompletionRequestToolMessageContent, ChatCompletionRequestToolMessageContentPart,
            ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
            ChatCompletionRequestUserMessageContentPart, ChatCompletionTool,
            ChatCompletionToolChoiceOption,
        },
        responses::{
            EasyInputContent, EasyInputMessage, FunctionCallOutput, FunctionCallOutputItemParam,
            FunctionTool, FunctionToolCall, InputContent, InputItem, InputParam, InputTextContent,
            Item, Role, Tool, ToolChoiceFunction, ToolChoiceParam,
        },
    },
};
use std::collections::HashMap;

use oxy_shared::errors::OxyError;

// Re-export config types from oxy-shared for backward compatibility
pub use oxy_shared::{AzureModel, ConfigType, CustomOpenAIConfig};

pub use validation::validate_api_key;

/// Default OpenAI API URL (used by the key-validation probe in `validation.rs`).
/// The `OpenAIModelConfig` schema type + its serde default now live in `oxy-llm`.
pub const OPENAI_API_URL: &str = "https://api.openai.com/v1";

/// Vendor label used in user-facing messages (e.g. validation errors).
pub const VENDOR_LABEL: &str = "OpenAI";

pub type OpenAIClient = Client<ConfigType>;

/// Creates an OpenAI-compatible configuration from model parameters
///
/// # Arguments
/// * `api_key` - The API key (already resolved from secrets)
/// * `api_url` - Optional custom API URL
/// * `azure` - Optional Azure deployment configuration
/// * `custom_headers` - Optional custom headers (already resolved from secrets)
///
/// # Returns
/// A `ConfigType` configured for OpenAI, Azure, or with custom headers
pub fn create_config_from_model(
    api_key: String,
    api_url: Option<String>,
    azure: Option<AzureModel>,
    custom_headers: Option<HashMap<String, String>>,
) -> ConfigType {
    if let Some(azure) = azure {
        // Azure OpenAI configuration
        let mut config = AzureConfig::new()
            .with_api_version(&azure.azure_api_version)
            .with_deployment_id(&azure.azure_deployment_id)
            .with_api_key(api_key);
        if let Some(api_url) = api_url {
            config = config.with_api_base(api_url);
        }
        ConfigType::Azure(config)
    } else if let Some(custom_headers) = custom_headers {
        // OpenAI with custom headers
        let mut config = OpenAIConfig::new().with_api_key(api_key);
        if let Some(api_url) = api_url {
            config = config.with_api_base(api_url);
        }
        let config_with_headers = CustomOpenAIConfig::new(config, custom_headers);
        ConfigType::WithHeaders(config_with_headers)
    } else {
        let mut config = OpenAIConfig::new().with_api_key(api_key);
        if let Some(api_url) = api_url {
            config = config.with_api_base(api_url);
        }
        ConfigType::Default(config)
    }
}

// Streaming Types

/// Represents a chunk of data from an OpenAI streaming response
pub enum StreamChunk {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        args: String,
    },
}

// Message Conversion Functions

/// Converts an OpenAI ChatCompletionRequestSystemMessage to a response InputItem
pub fn convert_system_message(sys_msg: ChatCompletionRequestSystemMessage) -> InputItem {
    let content = match sys_msg.content {
        ChatCompletionRequestSystemMessageContent::Text(text) => EasyInputContent::Text(text),
        ChatCompletionRequestSystemMessageContent::Array(parts) => {
            let content_parts: Vec<InputContent> = parts
                .into_iter()
                .map(|part| match part {
                    ChatCompletionRequestSystemMessageContentPart::Text(text_part) => {
                        InputContent::InputText(InputTextContent {
                            text: text_part.text,
                        })
                    }
                })
                .collect();
            EasyInputContent::ContentList(content_parts)
        }
    };
    InputItem::EasyMessage(EasyInputMessage {
        role: Role::System,
        content,
        r#type: Default::default(),
        phase: None,
    })
}

/// Converts an OpenAI ChatCompletionRequestUserMessage to a response InputItem
pub fn convert_user_message(user_msg: ChatCompletionRequestUserMessage) -> InputItem {
    let content = match user_msg.content {
        ChatCompletionRequestUserMessageContent::Text(text) => EasyInputContent::Text(text),
        ChatCompletionRequestUserMessageContent::Array(parts) => {
            let content_parts: Vec<InputContent> = parts
                .into_iter()
                .filter_map(|part| match part {
                    ChatCompletionRequestUserMessageContentPart::Text(text_part) => {
                        Some(InputContent::InputText(InputTextContent {
                            text: text_part.text,
                        }))
                    }
                    ChatCompletionRequestUserMessageContentPart::ImageUrl(_img_part) => None,
                    ChatCompletionRequestUserMessageContentPart::InputAudio(_) => None,
                    ChatCompletionRequestUserMessageContentPart::File(_) => None,
                })
                .collect();
            EasyInputContent::ContentList(content_parts)
        }
    };
    InputItem::EasyMessage(EasyInputMessage {
        r#type: Default::default(),
        role: Role::User,
        content,
        phase: None,
    })
}

/// Converts an OpenAI ChatCompletionRequestAssistantMessage to response InputItems
pub fn convert_assistant_message(
    asst_msg: ChatCompletionRequestAssistantMessage,
) -> Vec<InputItem> {
    let mut items = Vec::new();

    let content = match asst_msg.content {
        Some(ChatCompletionRequestAssistantMessageContent::Text(text)) => {
            EasyInputContent::Text(text)
        }
        Some(ChatCompletionRequestAssistantMessageContent::Array(parts)) => {
            let content_parts: Vec<InputContent> = parts
                .into_iter()
                .filter_map(|part| match part {
                    ChatCompletionRequestAssistantMessageContentPart::Text(text_part) => {
                        Some(InputContent::InputText(InputTextContent {
                            text: text_part.text,
                        }))
                    }
                    ChatCompletionRequestAssistantMessageContentPart::Refusal(_) => None,
                })
                .collect();
            EasyInputContent::ContentList(content_parts)
        }
        None => EasyInputContent::Text(String::new()),
    };

    items.push(InputItem::EasyMessage(EasyInputMessage {
        r#type: Default::default(),
        role: Role::Assistant,
        content,
        phase: None,
    }));

    if let Some(tool_calls) = asst_msg.tool_calls {
        for tool_call in tool_calls {
            match tool_call {
                ChatCompletionMessageToolCalls::Function(function_call) => {
                    tracing::debug!(
                        "Converting assistant tool_call to function_call item: id={}, name={}",
                        function_call.id,
                        function_call.function.name
                    );
                    let function_call_item = Item::FunctionCall(FunctionToolCall {
                        id: None,
                        call_id: function_call.id,
                        name: function_call.function.name,
                        arguments: function_call.function.arguments,
                        status: None,
                        namespace: None,
                    });

                    items.push(InputItem::Item(function_call_item));
                    continue;
                }
                ChatCompletionMessageToolCalls::Custom(
                    chat_completion_message_custom_tool_call,
                ) => {
                    tracing::debug!(
                        "Skipping conversion of custom tool call with call_id: {}",
                        chat_completion_message_custom_tool_call.id
                    );
                }
            }
        }
    }
    items
}

/// Converts an OpenAI ChatCompletionRequestDeveloperMessage to a response InputItem
pub fn convert_developer_message(dev_msg: ChatCompletionRequestDeveloperMessage) -> InputItem {
    let content = match dev_msg.content {
        ChatCompletionRequestDeveloperMessageContent::Text(text) => EasyInputContent::Text(text),
        ChatCompletionRequestDeveloperMessageContent::Array(parts) => {
            let content_parts: Vec<InputContent> = parts
                .into_iter()
                .filter_map(|part| match part {
                    ChatCompletionRequestDeveloperMessageContentPart::Text(text_part) => {
                        Some(InputContent::InputText(InputTextContent {
                            text: text_part.text,
                        }))
                    }
                })
                .collect();
            EasyInputContent::ContentList(content_parts)
        }
    };
    InputItem::EasyMessage(EasyInputMessage {
        r#type: Default::default(),
        role: Role::Developer,
        content,
        phase: None,
    })
}

/// Converts an OpenAI ChatCompletionRequestToolMessage to a response InputItem
pub fn convert_tool_message(tool_msg: ChatCompletionRequestToolMessage) -> InputItem {
    tracing::debug!(
        "Converting tool message with call_id: {}",
        tool_msg.tool_call_id
    );

    let output = match tool_msg.content {
        ChatCompletionRequestToolMessageContent::Text(text) => FunctionCallOutput::Text(text),
        ChatCompletionRequestToolMessageContent::Array(parts) => {
            let content_parts: Vec<InputContent> = parts
                .into_iter()
                .filter_map(|part| match part {
                    ChatCompletionRequestToolMessageContentPart::Text(text_part) => {
                        Some(InputContent::InputText(InputTextContent {
                            text: text_part.text,
                        }))
                    }
                })
                .collect();
            FunctionCallOutput::Content(content_parts)
        }
    };

    InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
        call_id: tool_msg.tool_call_id,
        output,
        id: None,
        status: None,
    }))
}

/// Converts ChatCompletionRequestMessages to Response API InputParam
pub fn convert_messages_to_response_input(
    messages: Vec<ChatCompletionRequestMessage>,
) -> Result<InputParam, OxyError> {
    let mut input_items: Vec<InputItem> = Vec::new();

    for msg in messages {
        match msg {
            ChatCompletionRequestMessage::System(sys_msg) => {
                input_items.push(convert_system_message(sys_msg));
            }
            ChatCompletionRequestMessage::User(user_msg) => {
                input_items.push(convert_user_message(user_msg));
            }
            ChatCompletionRequestMessage::Assistant(asst_msg) => {
                input_items.extend(convert_assistant_message(asst_msg));
            }
            ChatCompletionRequestMessage::Developer(dev_msg) => {
                input_items.push(convert_developer_message(dev_msg));
            }
            ChatCompletionRequestMessage::Tool(tool_msg) => {
                input_items.push(convert_tool_message(tool_msg));
            }
            ChatCompletionRequestMessage::Function(func_msg) => {
                tracing::debug!("Converting function message with name: {}", func_msg.name);
            }
        }
    }

    Ok(InputParam::Items(input_items))
}

/// Converts ChatCompletion tool choice to Response API tool choice
pub fn convert_tool_choice(
    chat_tool_choice: &ChatCompletionToolChoiceOption,
) -> Result<ToolChoiceParam, OxyError> {
    use async_openai::types::chat::ToolChoiceOptions as ChatToolChoiceOptions;
    use async_openai::types::responses::ToolChoiceOptions as ResponsesToolChoiceOptions;

    match chat_tool_choice {
        ChatCompletionToolChoiceOption::Mode(mode) => {
            let converted_mode = match mode {
                ChatToolChoiceOptions::None => ResponsesToolChoiceOptions::None,
                ChatToolChoiceOptions::Auto => ResponsesToolChoiceOptions::Auto,
                ChatToolChoiceOptions::Required => ResponsesToolChoiceOptions::Required,
            };
            Ok(ToolChoiceParam::Mode(converted_mode))
        }
        ChatCompletionToolChoiceOption::Function(ChatCompletionNamedToolChoice { function }) => {
            Ok(ToolChoiceParam::Function(ToolChoiceFunction {
                name: function.name.clone(),
            }))
        }
        ChatCompletionToolChoiceOption::Custom(_custom) => {
            // Custom tool choice not currently supported, default to None
            Ok(ToolChoiceParam::Mode(ResponsesToolChoiceOptions::None))
        }
        ChatCompletionToolChoiceOption::AllowedTools(_allowed_tools) => {
            // Allowed tools choice not currently supported, default to Auto
            Ok(ToolChoiceParam::Mode(ResponsesToolChoiceOptions::Auto))
        }
    }
}

pub fn convert_tools(chat_tools: &[ChatCompletionTool]) -> Result<Vec<Tool>, OxyError> {
    chat_tools
        .iter()
        .map(|tool| {
            Ok(Tool::Function(FunctionTool {
                name: tool.function.name.clone(),
                parameters: tool.function.parameters.clone(),
                strict: tool.function.strict,
                description: tool.function.description.clone(),
                defer_loading: None,
            }))
        })
        .collect()
}
