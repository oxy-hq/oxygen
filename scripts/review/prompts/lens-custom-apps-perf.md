Focus: custom-app serving and data-plane performance

Read first: `crates/api-custom-apps/.claude/skills/oxy-customer-apps-perf/SKILL.md` and `internal-docs/customer-apps-performance.md`.

Check:
- A `/customer-apps/**` serving route without the right `Cache-Control`, or with
  compression that breaks SSE.
- A result cache not keyed `project_id` first, read before the auth gates, or ignoring
  `?refresh`.
- A new per-request read on the custom-app hot path (the filesystem, an uncached query, a
  remote call) that the guardrails rule out.
