use std::{collections::HashMap, path::Path, sync::Arc};

use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use tokio::sync::Mutex;

use oxy::{
    adapters::workspace::manager::WorkspaceManager,
    config::constants::EVAL_SOURCE,
    exec_runtime::writer::EventHandler,
    exec_types::{Event, EventKind, ProgressType},
};
use oxy_shared::errors::OxyError;

use crate::builders::EvalLauncher;
use crate::builders::types::{EvalInput, EvalResult};
use oxy::config::WorkingCopy;

/// Global token stats accumulated across all eval runs, shared between
/// the event handler and the calling command.
#[derive(Debug, Default, Clone)]
pub struct TokenStats {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
}

pub type SharedTokenStats = Arc<Mutex<TokenStats>>;

/// General-purpose progress bar handler using indicatif.
/// Used by agent, automation, and eval event handlers.
pub struct PBarsHandler {
    bars: HashMap<String, ProgressBar>,
}

impl Default for PBarsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl PBarsHandler {
    pub fn new() -> Self {
        PBarsHandler {
            bars: HashMap::new(),
        }
    }

    pub fn get_or_create_bar(&mut self, name: &str, total: Option<usize>) -> &mut ProgressBar {
        self.bars.entry(name.to_string()).or_insert_with(|| {
            let pb = match total {
                Some(total) => {
                    let pb = ProgressBar::new(total as u64);
                    pb.set_style(
                        ProgressStyle::with_template(
                            "  {spinner:.green} {msg} [{bar:30.green/white}] {pos}/{len}",
                        )
                        .unwrap()
                        .tick_chars("|/-\\")
                        .progress_chars("## "),
                    );
                    pb
                }
                None => {
                    let pb = ProgressBar::new_spinner();
                    pb.set_style(
                        ProgressStyle::with_template("  {spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("|/-\\"),
                    );
                    pb
                }
            };
            pb.enable_steady_tick(std::time::Duration::from_millis(80));
            pb
        })
    }

    pub fn update_bar(&mut self, name: &str, progress: usize) -> Result<(), OxyError> {
        if let Some(bar) = self.bars.get_mut(name) {
            bar.inc(progress as u64);
        }
        Ok(())
    }

    pub fn remove_bar(&mut self, name: &str) {
        if let Some(bar) = self.bars.remove(name) {
            bar.finish_and_clear();
        }
    }
}

const SPINNER_CHARS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
const TICK_MS: u64 = 80;

enum Phase {
    Idle,
    Generating,
    Evaluating,
}

pub struct EvalEventsHandler {
    quiet: bool,
    phase: Phase,
    progress_bar: Option<ProgressBar>,
    eval_total: u64,
    eval_completed: u64,
    token_stats: SharedTokenStats,
    test_label: Option<String>,
    /// Display labels for each case (name if set, otherwise truncated prompt).
    case_labels: Vec<String>,
    /// Number of runs per case (from test settings).
    runs_per_case: usize,
}

impl EvalEventsHandler {
    pub fn new(quiet: bool, token_stats: SharedTokenStats) -> Self {
        EvalEventsHandler {
            quiet,
            phase: Phase::Idle,
            progress_bar: None,
            eval_total: 0,
            eval_completed: 0,
            token_stats,
            test_label: None,
            case_labels: vec![],
            runs_per_case: 0,
        }
    }

    pub fn with_case_info(mut self, labels: Vec<String>, runs_per_case: usize) -> Self {
        self.case_labels = labels;
        self.runs_per_case = runs_per_case;
        self
    }

    pub fn with_test_label(mut self, label: String) -> Self {
        self.test_label = Some(label);
        self
    }

    fn update_spinner(&mut self) {
        if self.quiet {
            return;
        }
        let msg = self.build_msg();
        match &self.progress_bar {
            Some(pb) => pb.set_message(msg),
            None => {
                let pb = ProgressBar::new_spinner();
                pb.set_style(
                    ProgressStyle::with_template("  {spinner:.green} {msg}")
                        .unwrap()
                        .tick_chars(SPINNER_CHARS),
                );
                pb.set_message(msg);
                pb.enable_steady_tick(std::time::Duration::from_millis(TICK_MS));
                self.progress_bar = Some(pb);
            }
        }
    }

    /// Build the spinner message. Up to three lines:
    ///
    /// ```text
    ///   ⠙ Running test cases · velocity_test.test.yml
    ///     case 1/4 · What were the top 5 videos…          ← only when case info present
    ///     [████████████░░░░░░░░░░░░░░░░░░░░░░░░░░] 2/12   ← only when total is known
    /// ```
    fn build_msg(&self) -> String {
        let phase = match &self.phase {
            Phase::Generating => "Running test cases",
            Phase::Evaluating => "Judging responses",
            Phase::Idle => "Processing",
        };
        let file_part = match &self.test_label {
            Some(label) => format!(" · {label}"),
            None => String::new(),
        };
        let mut lines = vec![format!("{phase}{file_part}")];

        if self.runs_per_case > 0 && !self.case_labels.is_empty() {
            let num_cases = self.case_labels.len();
            let case_idx = ((self.eval_completed as usize) / self.runs_per_case)
                .min(num_cases.saturating_sub(1));
            let label = &self.case_labels[case_idx];
            lines.push(format!("    case {}/{num_cases} · {label}", case_idx + 1));
        }

        if self.eval_total > 0 {
            const BAR_WIDTH: usize = 40;
            let filled = ((self.eval_completed * BAR_WIDTH as u64) / self.eval_total) as usize;
            let empty = BAR_WIDTH.saturating_sub(filled);
            let pct = (self.eval_completed * 100) / self.eval_total;
            let bar = format!("    {}{}  {}%", "█".repeat(filled), "░".repeat(empty), pct,);
            lines.push(bar);
        }

        lines.join("\n")
    }

    fn finish_progress(&mut self) {
        if let Some(pb) = self.progress_bar.take() {
            pb.finish_and_clear();
        }
    }
}

#[async_trait::async_trait]
impl EventHandler for EvalEventsHandler {
    async fn handle_event(&mut self, event: Event) -> Result<(), OxyError> {
        tracing::debug!("Received event: {:?}", event);

        // Capture token usage from any source (agent runs + judge LLM calls)
        if let EventKind::Usage { usage } = &event.kind {
            let mut stats = self.token_stats.lock().await;
            stats.total_input_tokens += usage.input_tokens as i64;
            stats.total_output_tokens += usage.output_tokens as i64;
            return Ok(());
        }

        match event.source.kind.as_str() {
            EVAL_SOURCE => match event.kind {
                EventKind::Started { .. } => {
                    self.update_spinner();
                }
                EventKind::Finished { .. } => {
                    self.finish_progress();
                }
                EventKind::Progress { progress } => match progress {
                    ProgressType::Started(total) => {
                        self.eval_total = total.unwrap_or(0) as u64;
                        self.eval_completed = 0;
                        self.update_spinner();
                    }
                    ProgressType::Updated(n) => {
                        self.eval_completed += n as u64;
                        self.update_spinner();
                    }
                    ProgressType::Finished => {
                        self.finish_progress();
                    }
                },
                EventKind::Message { message } => {
                    let stripped = strip_ansi_escapes::strip_str(&message);
                    if stripped.contains("Generating outputs") {
                        self.phase = Phase::Generating;
                        self.update_spinner();
                    } else if stripped.contains("Evaluating records") {
                        self.phase = Phase::Evaluating;
                        self.eval_total = 0;
                        self.eval_completed = 0;
                        self.update_spinner();
                    }
                    // Suppress all other messages.
                }
                _ => {}
            },
            _ => {
                tracing::debug!("Non-eval event: {:?}", event);
            }
        }

        Ok(())
    }
}

pub async fn run_eval<P: AsRef<Path>, H: EventHandler + Send + 'static>(
    workspace_manager: WorkspaceManager<WorkingCopy>,
    path: P,
    index: Option<usize>,
    event_handler: H,
) -> Result<Vec<EvalResult>, OxyError> {
    run_eval_with_tag(workspace_manager, path, index, None, event_handler).await
}

pub async fn run_eval_with_tag<P: AsRef<Path>, H: EventHandler + Send + 'static>(
    workspace_manager: WorkspaceManager<WorkingCopy>,
    path: P,
    index: Option<usize>,
    tag: Option<String>,
    event_handler: H,
) -> Result<Vec<EvalResult>, OxyError> {
    let input = EvalInput {
        index,
        target_ref: path.as_ref().to_string_lossy().to_string(),
        tag,
        regrade_from: None,
        test: None,
    };
    launch_eval(workspace_manager, input, event_handler).await
}

/// Grade the outputs a previous run of `path` recorded in `results` (its
/// `--output-json` file) with the test file's current grader; the agent is
/// not run.
pub async fn regrade_eval<P: AsRef<Path>, H: EventHandler + Send + 'static>(
    workspace_manager: WorkspaceManager<WorkingCopy>,
    path: P,
    results: &Path,
    event_handler: H,
) -> Result<Vec<EvalResult>, OxyError> {
    let input = EvalInput {
        index: None,
        target_ref: path.as_ref().to_string_lossy().to_string(),
        tag: None,
        regrade_from: Some(crate::builders::types::RegradeSource::File(
            results.to_path_buf(),
        )),
        test: None,
    };
    launch_eval(workspace_manager, input, event_handler).await
}

async fn launch_eval<H: EventHandler + Send + 'static>(
    workspace_manager: WorkspaceManager<WorkingCopy>,
    input: EvalInput,
    event_handler: H,
) -> Result<Vec<EvalResult>, OxyError> {
    let result = EvalLauncher::new()
        .with_workspace(workspace_manager)
        .await?
        .launch(input, event_handler)
        .await;
    result.and_then(|r| {
        r.into_iter()
            .try_collect::<EvalResult, Vec<EvalResult>, OxyError>()
    })
}
