# oxy-app-core

**What every HTTP surface needs in order to be a surface — and nothing a single
bounded context owns.** `oxy-app` and every `api-*` surface crate sit on top of
this crate, so an edit here rebuilds all of them. That is the whole reason the
scope is narrow: app-core is the most expensive place in the workspace to put
code that changes.

The rule is S9 in `internal-docs/domain-boundaries.md`, enforced by
`src/charter_tests.rs`.

## What belongs here — four kinds

| Kind | Means | Examples |
| ---- | ----- | -------- |
| `Request` | Request state and context every surface reads | `app_state`, `serve_mode`, `process_role`, `config`, `forwarded`, `origin`, `org_host_dispatch`, `workspace_org` |
| `Authz` | Authorization and refusal plumbing (the decisions stay in `oxy-authz`) | `member_authz`, `gate_response`, `agent_refusal`, `agent_body` |
| `Service` | Cross-cutting services with no owning context | `audit`, `pagination`, `http_cache`, `rate_limit`, `compiled_reader`, `sql_kind` |
| `Seam` | Traits that let a lower crate call up without depending up | `function_seam`, `semantic_pin_seam` |

## Admission test — all four must hold

1. **No owning context.** If the module's honest name starts with a product
   noun (custom apps, notifications, widgets, frontline, billing, usage…), it
   has an owner and belongs in that owner's domain crate, not here.
2. **Stable.** It changes rarely. Code that changes often goes *sideways* into
   an `api-<context>` surface, where an edit skips `oxy-app` entirely.
3. **Shared.** Two or more crates above it use it. A module only one crate uses
   belongs in that crate.
4. **No routes, entities or migrator.** Routes live in a surface, entities in
   `entity`, migrations in `migration`.

"Two surfaces both need it" satisfies (3) only. It is the reason to look for a
domain crate both can depend on, not a reason to land here.

## The guard

`charter_tests.rs` holds two closed lists that together must name every
`pub mod` in `lib.rs`, each exactly once:

- **`CHARTER`** — modules that pass the test, each tagged with its kind. Adding
  one is a deliberate act a reviewer sees: name the kind, and be ready to say
  which of the four tests it passes.
- **`BACKLOG`** — modules that are here but have an owner, each with the crate
  it belongs in. **Shrink-only**: never add a row. A module leaving (moved to
  its owner) deletes its row; the test fails on a stale row.

A new module in neither list fails the test. So does a listed module that no
longer exists.

## Where things go instead

- A custom-app concern → `oxy-custom-apps` (domain) or `oxy-api-custom-apps` (HTTP).
- Org / team / membership logic shared by several contexts → `oxy-tenancy`.
- A vendor client (SES, Slack, …) → its own P2 integration crate.
- A context's HTTP handlers → its `api-<context>` surface, never here or in `oxy-app`
  (S8).
