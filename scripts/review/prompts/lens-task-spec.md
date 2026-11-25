Focus: background work (it has to survive an instance dying)

Read first: `crates/server/.claude/skills/oxy-task-spec-default/SKILL.md` and `internal-docs/task-spec-registry.md`.

Check:
- New long-running or periodic work started with `tokio::spawn` (or a loop) inside an
  HTTP handler or server path, where it should be a `TaskSpec` in `agentic_task_queue`:
  LLM calls, git clones, embedding builds, anything over about 5 seconds, anything that
  must finish.
- Fire-and-forget work whose loss on restart leaves state half-written, or a user waiting
  forever.
- An SSE stream that can end without a terminal event (`done`, `error` or `cancelled`),
  including a failure before its main loop starts.
