//! Revert builder-applied file changes for a run.
//!
//! When the analytics pipeline delegates to the builder agent (e.g. to fix
//! a broken semantics file) the builder auto-applies its file edits. Every
//! applied edit is persisted as a `file_changed` event carrying both the
//! pre-edit (`old_content`) and post-edit (`new_content`) state, so a
//! revert is a pure replay of `old_content` back onto the workspace — no
//! extra storage required.
//!
//! This is the single composition point the HTTP layer calls; it reads the
//! run's events, computes the inverse filesystem operation per file, writes
//! it through the same builder write primitives the original edit used, and
//! records an inverse `file_changed` event so a page reload reflects the
//! revert and the change history stays linear.

use std::collections::BTreeMap;
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::{Value, json};

use crate::PipelineError;
use crate::platform::PlatformContext;

/// One file that was reverted, and what inverse operation was applied.
#[derive(Debug, Serialize)]
pub struct RevertedFile {
    pub file_path: String,
    /// `"restored"` (prior content rewritten), `"deleted"` (a
    /// builder-created file removed), or `"recreated"` (a builder-deleted
    /// file written back).
    pub action: &'static str,
}

/// Unwrap a coordinator `delegation_event` envelope so a builder subrun's
/// events look the same whether read from the child run or bubbled onto the
/// parent (analytics) run.
fn unwrap_event<'a>(event_type: &'a str, payload: &'a Value) -> (&'a str, &'a Value) {
    if event_type == "delegation_event" {
        let inner_type = payload
            .get("inner_event_type")
            .and_then(|v| v.as_str())
            .unwrap_or(event_type);
        let inner = payload.get("inner").unwrap_or(payload);
        (inner_type, inner)
    } else {
        (event_type, payload)
    }
}

/// Revert builder-applied file change(s) for `run_id`.
///
/// `file_paths` empty → revert every file the builder changed in this run.
/// Reverts are applied in the order the files were last changed.
pub async fn revert_builder_file_changes(
    db: &DatabaseConnection,
    platform: &Arc<dyn PlatformContext>,
    run_id: &str,
    file_paths: &[String],
) -> Result<Vec<RevertedFile>, PipelineError> {
    let events = agentic_runtime::crud::get_all_events(db, run_id).await?;

    // Latest applied state per file (`file_changed` is emitted after the
    // edit is written; keep the newest by seq so re-edits collapse).
    let mut latest: BTreeMap<String, (i64, Value)> = BTreeMap::new();
    for ev in &events {
        let (etype, payload) = unwrap_event(&ev.event_type, &ev.payload);
        if etype != "file_changed" {
            continue;
        }
        let Some(fp) = payload.get("file_path").and_then(|v| v.as_str()) else {
            continue;
        };
        let entry = latest
            .entry(fp.to_string())
            .or_insert((ev.seq, payload.clone()));
        if ev.seq > entry.0 {
            *entry = (ev.seq, payload.clone());
        }
    }

    let targets: Vec<String> = if file_paths.is_empty() {
        latest.keys().cloned().collect()
    } else {
        file_paths.to_vec()
    };

    // `.filter(|p| p.is_dir())`, not a bare `Option` check: `workspace_path()`
    // is `Some` whenever the manager DECLARES a working copy, which it does on
    // every node — the slot is full even on a replica that has no volume. Only
    // the filesystem separates "declared" from "present", so without it this
    // guard passes on a worker and the failure resurfaces later as a raw ENOENT
    // from whichever write ran first. Same shape as `pipeline_ref.rs` and
    // `step_executor::load_sql_body`; enforced by
    // `crates/app/tests/platform/workspace_path_needs_is_dir.rs`.
    let workspace_root = platform
        .workspace_path()
        .filter(|p| p.is_dir())
        .ok_or_else(|| {
            PipelineError::Config(
                "revert: this node holds no workspace files, so a builder change cannot be undone"
                    .to_string(),
            )
        })?
        .to_path_buf();
    let mut next_seq = agentic_runtime::crud::get_max_seq(db, run_id).await? + 1;
    let mut reverted = Vec::with_capacity(targets.len());

    for fp in targets {
        let Some((_, payload)) = latest.get(&fp) else {
            return Err(PipelineError::Config(format!(
                "no applied builder change found for '{fp}' in run {run_id}"
            )));
        };
        let old_content = payload
            .get("old_content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_content = payload
            .get("new_content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let was_deletion = payload
            .get("is_deletion")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let description = payload
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Inverse of what the builder did:
        //  - it deleted the file        → write `old_content` back
        //  - it created a new file      → remove it (old_content empty)
        //  - it modified an existing one → restore `old_content`
        let (action, applied_new, applied_is_deletion): (&'static str, &str, bool) = if was_deletion
        {
            agentic_builder::tools::write_file_content(&workspace_root, &fp, old_content)
                .await
                .map_err(PipelineError::Build)?;
            ("recreated", old_content, false)
        } else if old_content.is_empty() {
            agentic_builder::tools::remove_file(&workspace_root, &fp)
                .await
                .map_err(PipelineError::Build)?;
            ("deleted", "", true)
        } else {
            agentic_builder::tools::write_file_content(&workspace_root, &fp, old_content)
                .await
                .map_err(PipelineError::Build)?;
            ("restored", old_content, false)
        };

        // Audit: persist the inverse change so a reload reflects the revert
        // (the `useBuilderActivity` derivation treats the newest
        // `file_changed` per path as the current state).
        let revert_payload = json!({
            "event_type": "file_changed",
            "file_path": fp,
            "description": format!("Reverted: {description}"),
            "new_content": applied_new,
            "old_content": new_content,
            "is_deletion": applied_is_deletion,
        });
        agentic_runtime::crud::insert_event(
            db,
            run_id,
            next_seq,
            "file_changed",
            &revert_payload,
            0,
        )
        .await?;
        next_seq += 1;

        reverted.push(RevertedFile {
            file_path: fp,
            action,
        });
    }

    Ok(reverted)
}
