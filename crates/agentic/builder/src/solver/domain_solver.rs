use async_trait::async_trait;

use agentic_core::{
    back_target::BackTarget,
    human_input::{ResumeInput, SuspendedRunData},
    orchestrator::{RunContext, SessionMemory},
    solver::DomainSolver,
    state::ProblemState,
    tools::{ToolDef, ToolError},
};

use crate::{
    tools::all_tools,
    types::{
        BuilderAnswer, BuilderDomain, BuilderError, BuilderIntent, BuilderResult, BuilderSolution,
        BuilderSpec,
    },
};

use super::{resuming, solver::BuilderSolver};

#[async_trait]
impl DomainSolver<BuilderDomain> for BuilderSolver {
    const SKIP_STATES: &'static [&'static str] = &["clarifying", "specifying", "executing"];

    async fn clarify(
        &mut self,
        intent: BuilderIntent,
        _ctx: &RunContext<BuilderDomain>,
        _memory: &SessionMemory<BuilderDomain>,
    ) -> Result<BuilderIntent, (BuilderError, BackTarget<BuilderDomain>)> {
        Ok(intent)
    }

    async fn specify_single(
        &mut self,
        intent: BuilderIntent,
        _ctx: &RunContext<BuilderDomain>,
        _memory: &SessionMemory<BuilderDomain>,
    ) -> Result<BuilderSpec, (BuilderError, BackTarget<BuilderDomain>)> {
        Ok(intent.into())
    }

    async fn solve(
        &mut self,
        spec: BuilderSpec,
        ctx: &RunContext<BuilderDomain>,
        _memory: &SessionMemory<BuilderDomain>,
    ) -> Result<BuilderSolution, (BuilderError, BackTarget<BuilderDomain>)> {
        self.solve_impl(spec, ctx).await
    }

    async fn execute(
        &mut self,
        solution: BuilderSolution,
        _ctx: &RunContext<BuilderDomain>,
        _memory: &SessionMemory<BuilderDomain>,
    ) -> Result<BuilderResult, (BuilderError, BackTarget<BuilderDomain>)> {
        Ok(solution.into())
    }

    async fn interpret(
        &mut self,
        result: BuilderResult,
        _ctx: &RunContext<BuilderDomain>,
        _memory: &SessionMemory<BuilderDomain>,
    ) -> Result<BuilderAnswer, (BuilderError, BackTarget<BuilderDomain>)> {
        self.interpret_impl(result).await
    }

    fn should_skip(
        &mut self,
        state: &str,
        data: &ProblemState<BuilderDomain>,
        _run_ctx: &RunContext<BuilderDomain>,
    ) -> Option<ProblemState<BuilderDomain>> {
        match (state, data) {
            ("clarifying", ProblemState::Clarifying(intent)) => {
                Some(ProblemState::Specifying(intent.clone()))
            }
            ("specifying", ProblemState::Specifying(intent)) => {
                Some(ProblemState::Solving(intent.clone().into()))
            }
            ("executing", ProblemState::Executing(solution)) => {
                Some(ProblemState::Interpreting(solution.clone().into()))
            }
            _ => None,
        }
    }

    async fn diagnose(
        &mut self,
        error: BuilderError,
        back: BackTarget<BuilderDomain>,
        _ctx: &RunContext<BuilderDomain>,
    ) -> Result<ProblemState<BuilderDomain>, BuilderError> {
        match back {
            BackTarget::Clarify(intent, _) => Ok(ProblemState::Clarifying(intent)),
            BackTarget::Specify(intent, _) => Ok(ProblemState::Specifying(intent)),
            BackTarget::Solve(spec, _) => Ok(ProblemState::Solving(spec)),
            BackTarget::Execute(solution, _) => Ok(ProblemState::Executing(solution)),
            BackTarget::Interpret(result, _) => Ok(ProblemState::Interpreting(result)),
            BackTarget::Suspend { .. } => Err(error),
        }
    }

    fn tools_for_state(state: &str) -> Vec<ToolDef> {
        match state {
            // Return an empty set here; the actual tool list is built in
            // solve_impl with access to `self.schema_provider`.
            "solving" => {
                let empty = crate::schema_provider::EmptySchemaProvider;
                all_tools(&empty)
            }
            _ => vec![],
        }
    }

    async fn execute_tool(
        &mut self,
        state: &str,
        name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        match state {
            "solving" => super::solver::dispatch_tool(
                name,
                &params,
                &self.project_root,
                &self.event_tx,
                self.test_runner.clone(),
                self.human_input.clone(),
                self.db_provider.as_ref(),
                self.project_validator.as_ref(),
                self.schema_provider.as_ref(),
                self.semantic_compiler.as_ref(),
                self.secrets_provider.as_ref(),
            )
            .await
            .map(|v| v.to_value()),
            _ => Err(ToolError::UnknownTool(format!(
                "no tools for state '{state}'"
            ))),
        }
    }

    fn store_suspension_data(&mut self, data: SuspendedRunData) {
        self.suspension_data = Some(data);
    }

    fn take_suspension_data(&mut self) -> Option<SuspendedRunData> {
        self.suspension_data.take()
    }

    fn set_resume_data(&mut self, data: ResumeInput) {
        self.resume_data = Some(data);
    }

    fn problem_state_from_resume(
        &self,
        data: &SuspendedRunData,
        _memory: &SessionMemory<BuilderDomain>,
    ) -> Option<ProblemState<BuilderDomain>> {
        Some(resuming::problem_state_from_resume(data))
    }
}
