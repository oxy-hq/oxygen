Focus: route placement on the split fleet (which role serves a route, and whether it may touch disk)

Read first: `crates/app/.claude/skills/oxy-route-classification/SKILL.md`.

Check:
- A route added, moved or renamed under `server/router/` (or an `api-*` crate) that is
  missing from the route declarations `crates/app-core/src/pods/role_manifest.rs` classifies against, or classified wrongly. A handler
  that reads the workspace working copy, `.git` or the state dir (`workspace_path()`,
  `ConfigManager::resolve_*`, `resolve_state_dir()`, `GitClient`) must be `IdeOnly`. A
  persisted-data read must stay `FleetOk`, or high availability hinges on one instance.
- Two routes with the same method and path: that panics when the router is built.
- A handler doing more than parse, call, serialize (the thin-transport rule, about 30
  lines).
