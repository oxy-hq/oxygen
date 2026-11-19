Focus: crate boundaries and dependency direction

Read first: `internal-docs/backend-architecture.md`, `internal-docs/domain-boundaries.md`
and `internal-docs/architecture-review-checklist.md`. Cite its R-* ids in `rule`.

Check:
- A dependency that flows the wrong way: a platform crate (`oxy`, `oxy-project`,
  `oxy-shared`) importing an `agentic-*` crate; `agentic-core` importing anything agentic;
  sibling domains (`agentic-analytics`, `agentic-automation`) importing
  each other; `agentic-http` importing a domain crate directly; any crate other than
  `oxy-airlayer-compat` declaring `airlayer`.
- A new crate that should have been a module, or the reverse (the litmus test in
  backend-architecture.md).
- A dependency or feature change without `crates/workspace-hack` updated (`just hakari`).
- A function over about 60 lines or a file over about 400 lines added by this PR, where
  splitting it is straightforward.
