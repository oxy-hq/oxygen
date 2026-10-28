use std::path::Path;

use agentic_core::{
    human_input::HumanInputProvider,
    tools::{ToolDef, ToolError},
};
use serde_json::{Value, json};

use super::utils::{ApplyAction, safe_path, suspend_or_apply};

pub fn write_file_def() -> ToolDef {
    ToolDef {
        name: "write_file",
        description: "Create a new file or fully overwrite an existing one. \
            Always use this for new files or when replacing the entire file content. \
            The user will see a diff and can accept or reject.",
        parameters: json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file, relative to the project root"
                },
                "content": {
                    "type": "string",
                    "description": "Complete new file content"
                },
                "description": {
                    "type": "string",
                    "description": "Human-readable description of what this change does and why"
                }
            },
            "required": ["file_path", "content", "description"],
            "additionalProperties": false
        }),
        ..Default::default()
    }
}

/// True for a path with the Data App suffix, either spelling.
pub(crate) fn is_data_app_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    lower.ends_with(".app.yml") || lower.ends_with(".app.yaml")
}

pub async fn execute_write_file(
    workspace_root: &Path,
    params: &Value,
    provider: &dyn HumanInputProvider,
) -> Result<Value, ToolError> {
    let file_path = params["file_path"]
        .as_str()
        .ok_or_else(|| ToolError::BadParams("missing 'file_path'".into()))?;
    let content = params["content"]
        .as_str()
        .ok_or_else(|| ToolError::BadParams("missing 'content'".into()))?;
    let description = params["description"]
        .as_str()
        .ok_or_else(|| ToolError::BadParams("missing 'description'".into()))?;

    let abs = safe_path(workspace_root, file_path)?;
    // Data Apps are retired: an existing `.app.yml` may still be rewritten, but
    // a new one is never created.
    if is_data_app_path(file_path) && !abs.exists() {
        return Err(ToolError::Execution(format!(
            "Data apps ('{file_path}') are retired and can no longer be created. \
             A dashboard or app is a custom app: a code-first React app published \
             with `oxyc publish`. Tell the user this instead of writing the file."
        )));
    }
    let old_content = tokio::fs::read_to_string(&abs).await.unwrap_or_default();

    suspend_or_apply(
        workspace_root,
        provider,
        "write_file",
        file_path,
        &old_content,
        content,
        description,
        ApplyAction::WriteContent { content },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::human_input::AutoAcceptInputProvider;

    struct RejectProvider;
    impl HumanInputProvider for RejectProvider {
        fn request_sync(&self, _prompt: &str, _suggestions: &[String]) -> Result<String, ()> {
            Ok("Reject".to_string())
        }
    }

    async fn test_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("write_file_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        dir.canonicalize().unwrap()
    }

    #[tokio::test]
    async fn creates_new_file_on_accept() {
        let dir = test_dir().await;
        let params = json!({
            "file_path": "new.agentic.yml",
            "content": "name: test\nmodel: gpt-4o\n",
            "description": "create test agent"
        });
        let result = execute_write_file(&dir, &params, &AutoAcceptInputProvider).await;
        assert!(result.is_ok());
        let written = tokio::fs::read_to_string(dir.join("new.agentic.yml"))
            .await
            .unwrap();
        assert_eq!(written, "name: test\nmodel: gpt-4o\n");
        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn refuses_to_create_a_data_app() {
        let dir = test_dir().await;
        for name in ["apps/new.app.yml", "NEW.APP.YAML"] {
            let params = json!({
                "file_path": name,
                "content": "tasks: []\n",
                "description": "create a data app"
            });
            let result = execute_write_file(&dir, &params, &AutoAcceptInputProvider).await;
            assert!(
                matches!(&result, Err(ToolError::Execution(m)) if m.contains("custom app")),
                "{name}: {result:?}"
            );
            assert!(!dir.join(name).exists(), "{name} was written");
        }
        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn still_rewrites_an_existing_data_app() {
        let dir = test_dir().await;
        tokio::fs::write(dir.join("old.app.yml"), "tasks: []\n")
            .await
            .unwrap();
        let params = json!({
            "file_path": "old.app.yml",
            "content": "tasks: [x]\n",
            "description": "edit"
        });
        execute_write_file(&dir, &params, &AutoAcceptInputProvider)
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(dir.join("old.app.yml"))
                .await
                .unwrap(),
            "tasks: [x]\n"
        );
        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn does_not_write_on_reject() {
        let dir = test_dir().await;
        let params = json!({
            "file_path": "should_not_exist.yml",
            "content": "content",
            "description": "test"
        });
        let result = execute_write_file(&dir, &params, &RejectProvider).await;
        assert!(result.is_ok());
        assert!(!dir.join("should_not_exist.yml").exists());
        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn overwrites_existing_file_on_accept() {
        let dir = test_dir().await;
        tokio::fs::write(dir.join("existing.yml"), "old content")
            .await
            .unwrap();
        let params = json!({
            "file_path": "existing.yml",
            "content": "new content",
            "description": "overwrite"
        });
        execute_write_file(&dir, &params, &AutoAcceptInputProvider)
            .await
            .unwrap();
        let written = tokio::fs::read_to_string(dir.join("existing.yml"))
            .await
            .unwrap();
        assert_eq!(written, "new content");
        tokio::fs::remove_dir_all(&dir).await.ok();
    }
}
