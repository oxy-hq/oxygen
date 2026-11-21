//! Domain-free contracts for a react (reason + act) loop: the tool and engine
//! traits, and the transcript the loop feeds the engine. The loop itself lives
//! in `agentic-runtime`; engines live in `agentic-llm`.

pub mod engine;
pub mod tool;
pub mod transcript;

pub use engine::{
    Decision, DoneHook, Engine, EngineError, EventSink, ToolCallReq, Turn, TurnUsage,
};
pub use tool::{Artifact, Effect, ResultHandle, Tool, ToolCx, ToolFailure, ToolReply, Toolset};
pub use transcript::{Entry, PriorTurn, Transcript};
